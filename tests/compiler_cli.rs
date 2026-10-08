use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "satansfiddle-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if env::var_os("SATANSFIDDLE_TEST_KEEP_ARTIFACTS").is_some() {
            eprintln!("compiler test artifacts: {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn cli_command(input: &Path, output: &Path, config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_satansfiddle"));
    command
        .arg("--config")
        .arg(config)
        .arg("--output")
        .arg(output)
        .arg(input);
    command
}

fn cli(input: &Path, output: &Path, config: &Path) -> Output {
    cli_command(input, output, config)
        .output()
        .expect("could not launch satansfiddle")
}

fn optimization(compiler: &Path) -> &'static str {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(fs::read(compiler).unwrap()));
    if hash == "6375fd27814a1cb7eddbc229ecbd15b57d83562f95be23804f0a06ad4814e2df" {
        "-O2"
    } else {
        "-O3,p"
    }
}

fn compiler_config(compiler: &Path, fp: &str) -> String {
    let mut table = toml::Table::new();
    table.insert(
        "compiler_path".into(),
        toml::Value::String(compiler.to_string_lossy().into_owned()),
    );
    table.insert(
        "compiler_options".into(),
        toml::Value::String(format!(
            "{} -c -Cpp_exceptions off -RTTI off -strings readonly",
            optimization(compiler)
        )),
    );
    if let Some(wibo) = env::var_os("SATANSFIDDLE_TEST_WIBO") {
        table.insert(
            "wibo_path".into(),
            toml::Value::String(wibo.to_string_lossy().into_owned()),
        );
    }
    table.extend(toml::from_str::<toml::Table>(fp).unwrap());
    serde_json::to_string_pretty(&table).unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "CLI failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unknown_compiler_is_rejected() {
    let work = Workspace::new();
    let compiler = work.0.join("unrecognized.exe");
    fs::write(&compiler, b"not a supported compiler").unwrap();
    let config = work.0.join("config.json");
    fs::write(&config, compiler_config(&compiler, "")).unwrap();
    let input = work.0.join("sample.cpp");
    fs::write(&input, "int value() { return 1; }\n").unwrap();
    let output = work.0.join("sample.o");
    let result = cli(&input, &output, &config);
    assert!(!result.status.success());
    assert!(!output.exists());
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(
        diagnostics.to_lowercase().contains("compiler"),
        "missing compiler rejection: {diagnostics}"
    );
}

/// Requires both genuine compiler executables; explicitly ignored in generic CI.
#[test]
#[ignore = "requires SATANSFIDDLE_TEST_COMPILER_233 and SATANSFIDDLE_TEST_COMPILER_300 plus debug-symbol wibo"]
fn real_compilers_repeatability_float_policies_and_failure() {
    for variable in [
        "SATANSFIDDLE_TEST_COMPILER_233",
        "SATANSFIDDLE_TEST_COMPILER_300",
    ] {
        let compiler = PathBuf::from(
            env::var_os(variable).unwrap_or_else(|| panic!("set {variable} to run this test")),
        );
        assert!(compiler.is_file(), "{variable} is not a file");
        let work = Workspace::new();
        let input = work.0.join("floating.cpp");
        fs::write(&input, include_str!("floating/floating.cpp")).unwrap();
        let config = work.0.join("config.json");
        fs::write(&config, compiler_config(&compiler, "")).unwrap();
        let first = work.0.join("first.o");
        let second = work.0.join("second.o");
        assert_success(&cli(&input, &first, &config));
        assert_success(&cli(&input, &second, &config));
        let bytes = fs::read(&first).unwrap();
        assert_eq!(&bytes[..4], b"\x7fELF");
        assert_eq!(
            bytes,
            fs::read(&second).unwrap(),
            "default output changed across processes for {variable}"
        );

        // Isolate the two policies so a passing test proves both hooks affect code.
        fs::write(
            &config,
            compiler_config(&compiler, "[floating_point]\ndefault_evaluate_first=true"),
        )
        .unwrap();
        let ordering = work.0.join("ordering.o");
        assert_success(&cli(&input, &ordering, &config));
        assert_ne!(
            function_bytes(&bytes, "constant_arguments__FPf"),
            function_bytes(&fs::read(&ordering).unwrap(), "constant_arguments__FPf"),
            "evaluate-first policy did not change constant argument ordering for {variable}"
        );

        if variable == "SATANSFIDDLE_TEST_COMPILER_233" {
            fs::write(
                &config,
                compiler_config(&compiler, "[floating_point]\ndefault_literal_reload=false"),
            )
            .unwrap();
            let reuse = work.0.join("reuse.o");
            assert_success(&cli(&input, &reuse, &config));
            let reload_code = function_bytes(&bytes, "literals_across_store__FPff");
            let reuse_code =
                function_bytes(&fs::read(&reuse).unwrap(), "literals_across_store__FPff");
            assert!(
                lwc1_count(&reload_code) > lwc1_count(&reuse_code),
                "literal reload policy did not add a pool load across the store for {variable}"
            );
        } else {
            // This image has no validated affected alias path; requests fail.
            for literal_request in [
                "[floating_point]\ndefault_literal_reload=false",
                "[floating_point]\ndefault_literal_reload=true",
                "[[floating_point.literal_overrides]]\ntranslation_unit='floating.cpp'\nvalue_type='binary32'\nvalue_bits='0x3c23d70a'\nreload=false",
            ] {
                fs::write(&config, compiler_config(&compiler, literal_request)).unwrap();
                assert!(
                    !cli(&input, &work.0.join("unsupported-literal.o"), &config)
                        .status
                        .success(),
                    "unsupported literal request was silently accepted for {variable}"
                );
            }
        }

        // The identity applies to every 1.25f constant in this function.
        let expression = "[[floating_point.expression_overrides]]\ntranslation_unit='floating.cpp'\nfunction='constant_arguments__FPf'\nvalue_type='binary32'\nvalue_bits='0x3fa00000'\nevaluate_first=true\n[[floating_point.expression_overrides]]\ntranslation_unit='floating.cpp'\nfunction='repeated_constant_arguments__FPf'\nvalue_type='binary32'\nvalue_bits='0x3fa00000'\nevaluate_first=true";
        fs::write(&config, compiler_config(&compiler, expression)).unwrap();
        let selected = work.0.join("selected.o");
        assert_success(&cli(&input, &selected, &config));
        assert_ne!(
            function_bytes(&bytes, "constant_arguments__FPf"),
            function_bytes(&fs::read(&selected).unwrap(), "constant_arguments__FPf"),
            "exact expression override did not change code for {variable}"
        );
        assert_eq!(
            function_bytes(
                &fs::read(&selected).unwrap(),
                "repeated_constant_arguments__FPf"
            ),
            function_bytes(
                &fs::read(&ordering).unwrap(),
                "repeated_constant_arguments__FPf"
            ),
            "bit selector did not affect all identical constants for {variable}"
        );
        assert_ne!(
            function_bytes(&bytes, "repeated_constant_arguments__FPf"),
            function_bytes(
                &fs::read(&selected).unwrap(),
                "repeated_constant_arguments__FPf"
            ),
            "repeated constant fixture did not exercise evaluate-first for {variable}"
        );
        let propagated = "[[floating_point.expression_overrides]]\ntranslation_unit='floating.cpp'\nfunction='propagated_constant_arguments__FPf'\nvalue_type='binary32'\nvalue_bits='0x3fa00000'\nevaluate_first=true";
        fs::write(&config, compiler_config(&compiler, propagated)).unwrap();
        let propagated_output = work.0.join("propagated.o");
        assert_success(&cli(&input, &propagated_output, &config));
        assert_ne!(
            function_bytes(&bytes, "propagated_constant_arguments__FPf"),
            function_bytes(
                &fs::read(&propagated_output).unwrap(),
                "propagated_constant_arguments__FPf"
            ),
            "propagated literal escaped the consumer policy for {variable}"
        );

        let broad = "[[floating_point.expression_overrides]]\ntranslation_unit='floating.cpp'\nfunction='split_callee_arguments__FPf'\nvalue_type='binary32'\nvalue_bits='0x3fa00000'\nevaluate_first=false\n";
        let scoped = broad.replace(
            "evaluate_first=false",
            "evaluate_first=true\ncallee='consume__Fffd'",
        );
        let mut scoped_code = Vec::new();
        for (index, policy) in [format!("{broad}{scoped}"), format!("{scoped}{broad}")]
            .iter()
            .enumerate()
        {
            fs::write(&config, compiler_config(&compiler, policy)).unwrap();
            let selected_callee = work.0.join(format!("callee-{index}.o"));
            assert_success(&cli(&input, &selected_callee, &config));
            let code = function_bytes(
                &fs::read(&selected_callee).unwrap(),
                "split_callee_arguments__FPf",
            );
            assert_ne!(
                code,
                function_bytes(&bytes, "split_callee_arguments__FPf"),
                "callee selector had no effect for {variable}"
            );
            assert_ne!(
                code,
                function_bytes(&fs::read(&ordering).unwrap(), "split_callee_arguments__FPf"),
                "callee selector leaked to another target for {variable}"
            );
            if index == 0 {
                scoped_code = code;
            } else {
                assert_eq!(
                    scoped_code, code,
                    "callee precedence depended on row order for {variable}"
                );
            }
        }
        fs::write(
            &config,
            compiler_config(
                &compiler,
                &scoped.replace("consume__Fffd", "missing_callee__Fffd"),
            ),
        )
        .unwrap();
        assert!(
            !cli(&input, &work.0.join("missing-callee.o"), &config)
                .status
                .success(),
            "unmatched callee selector silently succeeded for {variable}"
        );

        fs::write(&config, compiler_config(&compiler, expression)).unwrap();
        let generated = work.0.join("generated.cpp");
        fs::copy(&input, &generated).unwrap();
        let canonical_output = work.0.join("canonical.o");
        assert_success(
            &cli_command(&generated, &canonical_output, &config)
                .arg("--translation-unit")
                .arg("original/path/floating.cpp")
                .output()
                .unwrap(),
        );
        assert_eq!(
            function_bytes(&fs::read(&selected).unwrap(), "constant_arguments__FPf"),
            function_bytes(
                &fs::read(&canonical_output).unwrap(),
                "constant_arguments__FPf"
            ),
            "canonical TU selector was lost for generated input on {variable}"
        );
        fs::write(
            &config,
            compiler_config(
                &compiler,
                &expression.replace("constant_arguments__FPf", "missing_function__Fv"),
            ),
        )
        .unwrap();
        assert!(
            !cli(&input, &work.0.join("stale.o"), &config)
                .status
                .success(),
            "unconsumed expression override was silently accepted for {variable}"
        );
        let pooled_expression = "[[floating_point.expression_overrides]]\ntranslation_unit='floating.cpp'\nfunction='pooled_constant_arguments__FPf'\nvalue_type='binary32'\nvalue_bits='0x3c23d70a'\nevaluate_first=false";
        for flag in [false, true] {
            let policy = pooled_expression
                .replace("evaluate_first=false", &format!("evaluate_first={flag}"));
            fs::write(&config, compiler_config(&compiler, &policy)).unwrap();
            let pooled_selected = work.0.join(format!("pooled-selected-{flag}.o"));
            let result = cli_command(&input, &pooled_selected, &config)
                .env("SATANSFIDDLE_VERIFY", "1")
                .output()
                .unwrap();
            assert_success(&result);
            let diagnostic = format!(
                "expression floating.cpp/pooled_constant_arguments__FPf Binary32 0x3c23d70a evaluate_first={}",
                u8::from(flag)
            );
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(&diagnostic),
                "missing terminal annotation byte readback for {variable}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        // Named call targets also select pooled constants and optimizer-created
        // wrappers; no source occurrence count is part of the identity.
        let pooled_callee = pooled_expression.replace(
            "evaluate_first=false",
            "evaluate_first=true\ncallee='consume__Fffd'",
        );
        let integral_callee = pooled_callee
            .replace(
                "pooled_constant_arguments__FPf",
                "integral_constant_arguments__FPf",
            )
            .replace("0x3c23d70a", "0x43808000");
        let lowered_callees = format!("{pooled_callee}\n{integral_callee}");
        fs::write(&config, compiler_config(&compiler, &lowered_callees)).unwrap();
        let result = cli_command(&input, &work.0.join("pooled-callee.o"), &config)
            .env("SATANSFIDDLE_VERIFY", "1")
            .output()
            .unwrap();
        assert_success(&result);
        assert!(String::from_utf8_lossy(&result.stderr).contains("call-expression floating.cpp/pooled_constant_arguments__FPf callee=consume__Fffd Binary32 0x3c23d70a evaluate_first=1"), "pooled callee selector was not applied at its consumer for {variable}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("call-expression floating.cpp/integral_constant_arguments__FPf callee=consume__Fffd Binary32 0x43808000 evaluate_first=1"), "integral float callee selector was not applied at its consumer for {variable}");
        if variable == "SATANSFIDDLE_TEST_COMPILER_300" {
            // O2 takes 3.0's full pooled-constant annotation path as well.
            let mut o2_config: serde_json::Value =
                serde_json::from_str(&compiler_config(&compiler, "")).unwrap();
            o2_config["compiler_options"] =
                "-O2 -c -Cpp_exceptions off -RTTI off -strings readonly".into();
            fs::write(&config, serde_json::to_string(&o2_config).unwrap()).unwrap();
            let o2_baseline = work.0.join("o2-baseline.o");
            let o2_repeat = work.0.join("o2-repeat.o");
            assert_success(&cli(&input, &o2_baseline, &config));
            assert_success(&cli(&input, &o2_repeat, &config));
            assert_eq!(
                fs::read(&o2_baseline).unwrap(),
                fs::read(&o2_repeat).unwrap(),
                "3.0 pooled-literal default output changed between processes"
            );
            for flag in [false, true] {
                let policy = pooled_expression
                    .replace("evaluate_first=false", &format!("evaluate_first={flag}"));
                let selected_o2: serde_json::Value =
                    serde_json::from_str(&compiler_config(&compiler, &policy)).unwrap();
                o2_config["floating_point"] = selected_o2["floating_point"].clone();
                fs::write(&config, serde_json::to_string(&o2_config).unwrap()).unwrap();
                let o2_selected = work.0.join(format!("o2-selected-{flag}.o"));
                let result = cli_command(&input, &o2_selected, &config)
                    .env("SATANSFIDDLE_VERIFY", "1")
                    .output()
                    .unwrap();
                assert_success(&result);
                let diagnostic = format!(
                    "expression floating.cpp/pooled_constant_arguments__FPf Binary32 0x3c23d70a evaluate_first={}",
                    u8::from(flag)
                );
                assert!(
                    String::from_utf8_lossy(&result.stderr).contains(&diagnostic),
                    "3.0 O2 selected byte did not survive annotation: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            let selected_o2: serde_json::Value =
                serde_json::from_str(&compiler_config(&compiler, &lowered_callees)).unwrap();
            o2_config["floating_point"] = selected_o2["floating_point"].clone();
            fs::write(&config, serde_json::to_string(&o2_config).unwrap()).unwrap();
            let result = cli_command(&input, &work.0.join("o2-pooled-callee.o"), &config)
                .env("SATANSFIDDLE_VERIFY", "1")
                .output()
                .unwrap();
            assert_success(&result);
            assert!(String::from_utf8_lossy(&result.stderr).contains("call-expression floating.cpp/pooled_constant_arguments__FPf callee=consume__Fffd Binary32 0x3c23d70a evaluate_first=1"), "3.0 O2 pooled callee selector was not applied at its consumer");
            assert!(String::from_utf8_lossy(&result.stderr).contains("call-expression floating.cpp/integral_constant_arguments__FPf callee=consume__Fffd Binary32 0x43808000 evaluate_first=1"), "3.0 O2 integral float callee selector was not applied at its consumer");
        }
        fs::write(&config, compiler_config(&compiler, "")).unwrap();

        // Integer arithmetic is unchanged from the ordinary compiler baseline.
        let integer_input = work.0.join("integer.cpp");
        fs::write(&integer_input, include_str!("compile_sample/sample.cpp")).unwrap();
        let native_integer = work.0.join("integer-native.o");
        let hooked_integer = work.0.join("integer-hooked.o");
        let direct =
            Command::new(env::var_os("SATANSFIDDLE_TEST_WIBO").unwrap_or_else(|| "wibo".into()))
                .arg(&compiler)
                .args([
                    optimization(&compiler),
                    "-c",
                    "-Cpp_exceptions",
                    "off",
                    "-RTTI",
                    "off",
                    "-strings",
                    "readonly",
                ])
                .arg("-o")
                .arg(&native_integer)
                .arg(&integer_input)
                .output()
                .unwrap();
        assert_success(&direct);
        assert_success(&cli(&integer_input, &hooked_integer, &config));
        assert_eq!(
            function_bytes(&fs::read(&native_integer).unwrap(), "add__Fii"),
            function_bytes(&fs::read(&hooked_integer).unwrap(), "add__Fii"),
            "ordinary integer code changed for {variable}"
        );

        let include_directory = work.0.join("include-only-from-environment");
        fs::create_dir(&include_directory).unwrap();
        fs::write(
            include_directory.join("environment_only.hpp"),
            "#define ENVIRONMENT_HEADER_VALUE 123\n",
        )
        .unwrap();
        let header_input = work.0.join("header.cpp");
        fs::write(&header_input, "#include <environment_only.hpp>\nint header_value() { return ENVIRONMENT_HEADER_VALUE; }\n").unwrap();
        let header_output = work.0.join("header.o");
        assert!(
            !cli_command(&header_input, &header_output, &config)
                .env_remove("MWCIncludes")
                .output()
                .unwrap()
                .status
                .success(),
            "header fixture did not require environment search path for {variable}"
        );
        assert_success(
            &cli_command(&header_input, &header_output, &config)
                .env("MWCIncludes", &include_directory)
                .output()
                .unwrap(),
        );
        assert!(!function_bytes(&fs::read(&header_output).unwrap(), "header_value__Fv").is_empty());

        let bad_input = work.0.join("invalid.cpp");
        fs::write(&bad_input, "this is invalid C++ !\n").unwrap();
        let failed = work.0.join("failed.o");
        let failure = cli(&bad_input, &failed, &config);
        assert!(
            !failure.status.success(),
            "compiler diagnostics were treated as success for {variable}"
        );
        assert!(!failed.exists());
        let sentinel = b"existing output must survive a compiler failure";
        fs::write(&failed, sentinel).unwrap();
        assert!(!cli(&bad_input, &failed, &config).status.success());
        assert_eq!(fs::read(&failed).unwrap(), sentinel);
        assert!(
            !cli(&work.0.join("missing.cpp"), &failed, &config)
                .status
                .success()
        );
        assert_eq!(fs::read(&failed).unwrap(), sentinel);
    }
}

// Read named function bytes directly from MWCC's ELF32 little-endian object.
// Relocation fields remain unlinked; comparing one named function avoids data
// symbol numbering and unrelated sections obscuring the scheduling evidence.
fn function_bytes(elf: &[u8], name: &str) -> Vec<u8> {
    assert_eq!(&elf[..6], b"\x7fELF\x01\x01");
    fn u16_at(data: &[u8], off: usize) -> usize {
        u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize
    }
    fn u32_at(data: &[u8], off: usize) -> usize {
        u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize
    }
    let headers = u32_at(elf, 32);
    let stride = u16_at(elf, 46);
    let count = u16_at(elf, 48);
    let section = |index: usize| &elf[headers + index * stride..headers + (index + 1) * stride];
    for index in 0..count {
        let header = section(index);
        if u32_at(header, 4) != 2 {
            continue;
        } // SHT_SYMTAB
        let strings_header = section(u32_at(header, 24));
        let strings_start = u32_at(strings_header, 16);
        let start = u32_at(header, 16);
        let size = u32_at(header, 20);
        let entry_size = u32_at(header, 36);
        for entry in elf[start..start + size].chunks_exact(entry_size) {
            let string_start = strings_start + u32_at(entry, 0);
            let end = elf[string_start..].iter().position(|b| *b == 0).unwrap();
            if &elf[string_start..string_start + end] != name.as_bytes() {
                continue;
            }
            let code_header = section(u16_at(entry, 14));
            let code_start = u32_at(code_header, 16) + u32_at(entry, 4);
            return elf[code_start..code_start + u32_at(entry, 8)].to_vec();
        }
    }
    panic!("function symbol {name} missing from ELF");
}

fn lwc1_count(code: &[u8]) -> usize {
    code.chunks_exact(4)
        .filter(|word| u32::from_le_bytes((*word).try_into().unwrap()) >> 26 == 0x31)
        .count()
}
