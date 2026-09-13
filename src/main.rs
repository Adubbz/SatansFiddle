mod config;
mod hooks;

#[cfg(test)]
#[path = "../tests/compile_sample/mod.rs"]
mod compile_sample;

use anyhow::Result;
use clap::Parser;
use lldb::*;
use std::path::{Path, PathBuf};
use std::*;

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
}

fn hook(target: &SBTarget, address: Address, callback: fn(&SBProcess, &SBThread, &SBBreakpointLocation) -> bool) {
    let breakpoint = target.breakpoint_create_by_load_address(address);
    assert_eq!(
        breakpoint.num_resolved_locations(),
        1,
        "Breakpoint at {:#x} was not resolved",
        address
    );
    breakpoint.set_callback(callback);
}

fn configure_hooks(target: &SBTarget, _process: &SBProcess) {
    let compiler = hooks::get_compiler();
    hook(target, compiler.func_compile_translation_unit_addr, hooks::compile_translation_unit);
}

fn main() {
    let args = Args::parse();
    compile(args.input.as_path(), args.output.as_path(), args.config.as_path()).expect("Failed to compile source");
}

pub fn compile(input: &Path, output: &Path, config: &Path) -> Result<()> {
    config::initialize(config);

    // Derive and load the compiler settings
    hooks::load_compiler(config::compiler_path().as_path());
    let wibo_path = config::wibo_path()?;

    SBDebugger::initialize();

    let debugger = SBDebugger::create(false);
    debugger.set_async_mode(false);

    let target = debugger
        .create_target(
            Some(wibo_path),
            Some("x86_64-pc-linux-gnu"),
            Some("host"),
            true,
        )
        .expect("Failed to create target");

    let loader_breakpoint = target.breakpoint_create_by_regex(r"^\(anonymous namespace\)::loadPEFromSource");
    let launch_info = configure_launch(input, output)?;

    // Launch wibo
    let process = target.launch(&launch_info)?;

    // Ensure that wibo has hit our breakpoint before continuing
    assert_eq!(
        process.state(),
        ProcessState::Stopped,
        "wibo did not stop in loadPEFromSource"
    );

    // Delete the breakpoint so that we don't hit it again
    assert!(target.breakpoint_delete(loader_breakpoint.id()));
    
    // Run until the PE has been loaded.
    process.selected_thread().step_out()?;

    configure_hooks(&target, &process);

    // Resume the process which has been paused whilst configuring hooks
    process.resume().expect("Failed to resume wibo");

    let mut buffer: [u8; 4096] = [0; 4096];
    let n = process.read_stdout(&mut buffer);

    print!("{}", String::from_utf8_lossy(&buffer[..n]));
    let _ = process.kill();

    SBDebugger::terminate();
    return Ok(());
}

fn configure_launch(input: &Path, output: &Path) -> Result<SBLaunchInfo> {
    // Configure wibo to launch MWCC
    let compiler_path = config::compiler_path();
    let compiler_path_str = compiler_path.to_str().expect("Failed to convert compiler path to string");
    let compiler_options = config::compiler_options();
    
    let output_path = path::absolute(output)?.to_str().expect("Failed to convert absolute path to string").to_owned();
    let input_path = fs::canonicalize(input)?.to_str().expect("Failed to convert canonical path to string").to_owned();

    let mut launch_info = SBLaunchInfo::new();
    let mut compiler_arguments = Vec::with_capacity(compiler_options.len() + 4);
    compiler_arguments.push(compiler_path_str.to_owned());
    compiler_arguments.extend(compiler_options);
    compiler_arguments.extend([
        "-o".to_owned(),
        output_path,
        input_path,
    ]);
    launch_info.set_arguments(compiler_arguments.iter().map(String::as_str), false);

    return Ok(launch_info);
}
