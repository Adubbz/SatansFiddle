mod compiler;
mod config;
mod hooks;

#[cfg(test)]
#[path = "../tests/compile_sample/mod.rs"]
mod compile_sample;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use lldb::*;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{self, Path, PathBuf};

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Configuration file to use
    #[arg(short, long, value_name = "config")]
    config: PathBuf,

    /// C++ source file to compile
    #[arg(value_name = "input")]
    input: PathBuf,

    /// Object file to write
    #[arg(short, long, value_name = "output")]
    output: PathBuf,

    /// Logical source name for builds that compile a generated temporary file
    #[arg(long, value_name = "name")]
    translation_unit: Option<String>,
}

/// Keep LLDB alive until all target, process and breakpoint handles drop.
struct DebuggerSession(SBDebugger);

impl DebuggerSession {
    fn new() -> Self {
        SBDebugger::initialize();
        let debugger = SBDebugger::create(false);
        debugger.set_async_mode(false);
        Self(debugger)
    }
}

impl Drop for DebuggerSession {
    fn drop(&mut self) {
        SBDebugger::destroy(&self.0);
        SBDebugger::terminate();
    }
}

/// A rejected profile or failed callback must not leave a stopped compiler behind.
struct CompilerProcess(SBProcess);

impl Drop for CompilerProcess {
    fn drop(&mut self) {
        if !matches!(self.0.state(), ProcessState::Exited | ProcessState::Invalid) {
            let _ = self.0.kill();
        }
    }
}

/// Publish an object only after both the compiler and its hooks succeed.
struct PendingObject(PathBuf);

impl PendingObject {
    fn new(output: &Path) -> Result<Self> {
        let output = path::absolute(output)?;
        let parent = output
            .parent()
            .context("output path has no parent directory")?;
        for serial in 0..100 {
            let candidate = parent.join(format!(".satansfiddle-{}-{serial}.o", std::process::id()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(_) => return Ok(Self(candidate)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("cannot create a temporary output object"),
            }
        }
        bail!("cannot allocate a temporary output object")
    }

    fn publish(&self, output: &Path) -> Result<()> {
        let mut magic = [0; 4];
        fs::File::open(&self.0)?
            .read_exact(&mut magic)
            .context("compiler succeeded without producing an object")?;
        ensure!(&magic == b"\x7fELF", "compiler output is not an ELF object");
        fs::rename(&self.0, output)
            .with_context(|| format!("cannot publish object to {}", output.display()))
    }
}

impl Drop for PendingObject {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn main() {
    let args = Args::parse();
    if let Err(error) = compile(
        &args.input,
        &args.output,
        &args.config,
        args.translation_unit.as_deref(),
    ) {
        eprintln!("satansfiddle: {error:#}");
        std::process::exit(1);
    }
}

pub fn compile(
    input: &Path,
    output: &Path,
    config: &Path,
    translation_unit: Option<&str>,
) -> Result<()> {
    config::initialize(config)?;
    config::set_translation_unit(translation_unit)?;
    hooks::load_compiler(&config::compiler_path())?;
    let wibo_path = config::wibo_path()?;
    let pending = PendingObject::new(output)?;
    let launch_info = configure_launch(input, &pending.0)?;

    let session = DebuggerSession::new();
    let target = session
        .0
        .create_target(
            Some(wibo_path),
            Some("x86_64-pc-linux-gnu"),
            Some("host"),
            true,
        )
        .context("cannot create the wibo debugger target")?;
    // Current wibo exposes the mapped image before resolving its imports.
    // Older builds expose the file loader instead; those need a step-out.
    let mut loader = target.breakpoint_create_by_regex(r"wibo::Executable::resolveImports");
    let mut finish_loader = false;
    if loader.num_locations() == 0 {
        ensure!(
            target.breakpoint_delete(loader.id()),
            "cannot remove unresolved loader breakpoint"
        );
        loader = target.breakpoint_create_by_regex(r"^\(anonymous namespace\)::loadPEFromSource");
        finish_loader = true;
    }
    ensure!(
        loader.num_locations() > 0,
        "wibo has no supported PE loader debug symbol; use an unstripped wibo build"
    );

    let owned_process = CompilerProcess(target.launch(&launch_info).context("cannot launch wibo")?);
    let process = &owned_process.0;
    drain_output(process)?;
    ensure!(
        process.state() == ProcessState::Stopped,
        "wibo did not stop at its PE loader (state {:?}, exit status {})",
        process.state(),
        process.exit_status()
    );
    let thread = process.selected_thread();
    ensure!(
        thread.stop_reason() == StopReason::Breakpoint
            && (0..thread.stop_reason_data_count())
                .step_by(2)
                .any(|index| thread.stop_reason_data_at_index(index) == loader.id() as u64),
        "wibo stopped before loading the compiler: {}",
        thread.stop_description()
    );
    ensure!(
        target.breakpoint_delete(loader.id()),
        "cannot remove the PE loader breakpoint"
    );

    if finish_loader {
        // This older loader has not mapped the PE at its entry breakpoint.
        thread
            .step_out()
            .context("cannot finish loading the compiler")?;
        drain_output(process)?;
        ensure!(
            process.state() == ProcessState::Stopped
                && process.selected_thread().stop_reason() == StopReason::PlanComplete,
            "wibo failed while loading the compiler: {:?}: {}",
            process.state(),
            process.selected_thread().stop_description()
        );
    }
    hooks::configure_hooks(&target, process)?;

    // Callbacks resume automatically unless a hook fails. Any other stop,
    // including a compiler crash, must be reported as a failed compilation.
    let resumed = process.resume();
    drain_output(process)?;
    hooks::check_error()?;
    resumed.context("cannot resume the compiler")?;
    if process.state() != ProcessState::Exited {
        bail!(
            "compiler stopped unexpectedly: {:?}: {}",
            process.state(),
            process.selected_thread().stop_description()
        );
    }
    ensure!(
        process.exit_status() == 0,
        "compiler exited with status {}",
        process.exit_status()
    );
    hooks::check_complete()?;
    pending.publish(output)
}

fn drain_output(process: &SBProcess) -> Result<()> {
    let mut buffer = [0u8; 8192];
    let mut stdout = io::stdout().lock();
    loop {
        let count = process.read_stdout(&mut buffer);
        if count == 0 {
            break;
        }
        stdout.write_all(&buffer[..count])?;
    }
    stdout.flush()?;
    let mut stderr = io::stderr().lock();
    loop {
        let count = process.read_stderr(&mut buffer);
        if count == 0 {
            break;
        }
        stderr.write_all(&buffer[..count])?;
    }
    stderr.flush()?;
    Ok(())
}

fn configure_launch(input: &Path, output: &Path) -> Result<SBLaunchInfo> {
    let compiler_path = config::compiler_path();
    let compiler_path = compiler_path
        .to_str()
        .context("compiler path is not UTF-8")?;
    let compiler_options = config::compiler_options()?;
    let output_path = path::absolute(output)?;
    let input_path =
        fs::canonicalize(input).with_context(|| format!("cannot open {}", input.display()))?;
    let mut arguments = Vec::with_capacity(compiler_options.len() + 4);
    arguments.push(compiler_path.to_owned());
    arguments.extend(compiler_options);
    arguments.extend([
        "-o".to_owned(),
        output_path
            .to_str()
            .context("output path is not UTF-8")?
            .to_owned(),
        input_path
            .to_str()
            .context("input path is not UTF-8")?
            .to_owned(),
    ]);
    ensure!(
        arguments.iter().all(|arg| !arg.contains('\0')),
        "compiler arguments contain a NUL byte"
    );
    let mut launch = SBLaunchInfo::new();
    // MWCC is a fixed-base PE; disabling Linux ASLR is unnecessary and fails
    // under containers that disallow the personality syscall.
    launch.set_launch_flags(launch.launch_flags() & !LaunchFlag::DisableASLR);
    launch.set_arguments(arguments.iter().map(String::as_str), false);
    // SBLaunchInfo starts with an empty environment. MWCC relies on caller
    // settings such as MWCIncludes, and wibo must see the same host settings.
    let environment = SBEnvironment::new();
    for (name, value) in std::env::vars_os() {
        ensure!(
            environment.set(&name, &value, true),
            "cannot inherit compiler environment"
        );
    }
    launch.set_environment(&environment, false);
    Ok(launch)
}
