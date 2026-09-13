use super::compile;
use std::{fs, path::Path};

#[test]
fn test_basic_elf() {
    let test_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/compile_sample");
    let input_path = test_dir.join("sample.cpp");
    let output_path = test_dir.join("sample.o");
    let config_path = test_dir.join("config.toml");

    let _ = fs::remove_file(&output_path);
    compile(&input_path, &output_path, &config_path).unwrap();

    assert!(output_path.is_file());
    assert_eq!(&fs::read(&output_path).unwrap()[..4], b"\x7fELF");
    fs::remove_file(output_path).unwrap();
}
