use super::*;

const BASE: &str = "compiler_path = '/tmp/mwcc.exe'\n";

fn parse(extra: &str) -> Result<Config> {
    Config::parse(&format!("{BASE}{extra}"))
}

#[test]
fn legacy_config_and_defaults() {
    let old = Config::parse(include_str!("../compile_sample/config.toml")).unwrap();
    assert_eq!(old.translation_units[0].gpr_helper_mask, 0x30);
    assert!(!old.floating_point.default_evaluate_first);
    assert!(old.floating_point.default_literal_reload);
    let minimal = parse("").unwrap();
    assert_eq!(minimal.default_fpr_helper_mask, 0);
    assert!(minimal.translation_units.is_empty());
}

#[test]
fn example_uses_typed_schema() {
    Config::parse(include_str!("../../config.example.toml")).unwrap();
}

#[test]
fn selectors_preserve_float_bit_identity_and_normalize_paths() {
    let config = parse(
        r#"
[[floating_point.expression_overrides]]
translation_unit = 'src\test.cpp'
function = 'zero__Fv'
value_type = 'binary32'
value_bits = '0x80000000'
evaluate_first = true
[[floating_point.literal_overrides]]
translation_unit = 'src/test.cpp'
value_type = 'binary64'
value_bits = '0x7ff8000000000001'
reload = false
"#,
    )
    .unwrap();
    assert_eq!(
        config.floating_point.expression_overrides[0].translation_unit,
        "test.cpp"
    );
    assert_eq!(
        config.floating_point.expression_overrides[0].value_bits,
        0x80000000
    );
    assert_eq!(
        config.floating_point.literal_overrides[0].value_bits,
        0x7ff8000000000001
    );
}

#[test]
fn unknown_fields_versions_and_broken_options_fail() {
    for extra in [
        "complier_options = ''",
        "compiler_version = '9.9'",
        "compiler_options = '\"unterminated'",
        "[floating_point]\nmode = 'guess'",
        "[[floating_point.expression_overrides]]\nordinal=1",
    ] {
        assert!(parse(extra).is_err(), "accepted {extra}");
    }
}

const EXPRESSION: &str = r#"
[[floating_point.expression_overrides]]
translation_unit = 'test.cpp'
function = 'float__Fv'
value_type = 'binary32'
value_bits = '0x3f800000'
evaluate_first = false
"#;

#[test]
fn widths_flags_and_precision_are_validated() {
    for (from, to) in [
        ("0x3f800000", "0x100000000"),
        ("0x3f800000", "0x10000000000000000"),
        ("0x3f800000", "3f800000"),
        ("0x3f800000", "0xnope"),
        ("evaluate_first = false", "evaluate_first = 2"),
        ("binary32", "single"),
    ] {
        assert!(
            parse(&EXPRESSION.replace(from, to)).is_err(),
            "accepted {to}"
        );
    }
}

#[test]
fn duplicate_selectors_fail_instead_of_depending_on_order() {
    assert!(parse(&format!("{EXPRESSION}{EXPRESSION}")).is_err());
    assert!(parse("[[translation_units]]\nname='a/x.cpp'\ngpr_helper_mask=0\nfpr_helper_mask=0\n[[translation_units]]\nname='x.cpp'\ngpr_helper_mask=1\nfpr_helper_mask=1").is_err());
    let literal = "[[floating_point.literal_overrides]]\ntranslation_unit='x.cpp'\nvalue_type='binary64'\nvalue_bits='0x1'\nreload=true\n";
    assert!(parse(&format!("{literal}{literal}")).is_err());
}

#[test]
fn compiler_options_keep_quoted_values() {
    let config =
        parse(r#"compiler_options = '-pragma "divbyzerocheck on" -i "folder with spaces"'"#)
            .unwrap();
    assert_eq!(
        shlex::split(config.compiler_options.as_deref().unwrap()).unwrap(),
        ["-pragma", "divbyzerocheck on", "-i", "folder with spaces"]
    );
}

#[test]
fn existing_config_keys_parse_in_json_and_toml() {
    let json = Config::parse(include_str!("../../config.example.json")).unwrap();
    let toml = Config::parse(include_str!("../../config.example.toml")).unwrap();
    assert_eq!(json.default_gpr_helper_mask, toml.default_gpr_helper_mask);
    assert_eq!(
        json.translation_units[0].fpr_helper_mask,
        toml.translation_units[0].fpr_helper_mask
    );
    assert_eq!(
        json.floating_point.default_literal_reload,
        toml.floating_point.default_literal_reload
    );
    assert!(!json.floating_point.default_evaluate_first);
}

#[test]
fn json_rejects_unknown_occurrence_and_wrong_widths() {
    for extra in [
        "\"ordinal\": 1,",
        "\"occurrence\": 1,",
        "\"evaluate_first\": 2,",
    ] {
        let text = format!(
            r#"{{"compiler_path":"compiler.exe","floating_point":{{"expression_overrides":[{{{extra}"translation_unit":"x.cpp","function":"f","value_type":"binary32","value_bits":"0x1","evaluate_first":false}}]}}}}"#
        );
        assert!(Config::parse(&text).is_err(), "accepted {text}");
    }
    assert!(Config::parse(r#"{"compiler_path":"compiler.exe","floating_point":{"expression_overrides":[{"translation_unit":"x.cpp","function":"f","value_type":"binary32","value_bits":"0x100000000","evaluate_first":false}]}}"#).is_err());
}

#[test]
fn literal_policy_presence_is_preserved_for_capability_checks() {
    assert!(
        !parse("")
            .unwrap()
            .floating_point
            .default_literal_reload_explicit
    );
    assert!(
        !parse("[floating_point]")
            .unwrap()
            .floating_point
            .default_literal_reload_explicit
    );
    for value in [true, false] {
        let config = parse(&format!("[floating_point]\ndefault_literal_reload={value}")).unwrap();
        assert!(config.floating_point.default_literal_reload_explicit);
        assert_eq!(config.floating_point.default_literal_reload, value);
    }
    assert!(parse("[floating_point]\ndefault_literal_reload_explicit=true").is_err());
}

#[test]
fn translation_unit_override_rejects_empty_names_and_normalizes_paths() {
    assert!(set_translation_unit(Some("path/")).is_err());
    set_translation_unit(Some(r"directory\sample.cpp")).unwrap();
    assert_eq!(translation_unit_override(), Some("sample.cpp"));
    assert!(set_translation_unit(None).is_err());
}

#[test]
fn json_roundtrip_preserves_implicit_literal_policy() {
    for extra in ["", "[floating_point]\ndefault_literal_reload=false"] {
        let original = parse(extra).unwrap();
        let encoded = serde_json::to_string(&original).unwrap();
        let decoded = Config::parse(&encoded).unwrap();
        assert_eq!(
            original.floating_point.default_literal_reload_explicit,
            decoded.floating_point.default_literal_reload_explicit
        );
        assert_eq!(
            original.floating_point.default_literal_reload,
            decoded.floating_point.default_literal_reload
        );
    }
}

#[test]
fn callee_scopes_coexist_with_global_identity_and_reject_duplicates() {
    let first = EXPRESSION.replace(
        "evaluate_first = false",
        "callee='first__Ff'\nevaluate_first = false",
    );
    let second = EXPRESSION.replace(
        "evaluate_first = false",
        "callee='second__Ff'\nevaluate_first = true",
    );
    let config = parse(&format!("{EXPRESSION}{first}{second}")).unwrap();
    assert_eq!(config.floating_point.expression_overrides.len(), 3);
    assert!(
        config.floating_point.expression_overrides[0]
            .callee
            .is_none()
    );
    assert_eq!(
        config.floating_point.expression_overrides[1]
            .callee
            .as_deref(),
        Some("first__Ff")
    );
    assert!(parse(&format!("{first}{first}")).is_err());
    assert!(
        parse(&EXPRESSION.replace("evaluate_first = false", "callee=''\nevaluate_first=false"))
            .is_err()
    );
    let encoded = serde_json::to_string(&parse(EXPRESSION).unwrap()).unwrap();
    assert!(!encoded.contains("callee"));
}
