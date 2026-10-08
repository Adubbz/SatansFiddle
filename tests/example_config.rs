#[test]
fn example_config_parses() {
    toml::from_str::<toml::Table>(include_str!("../config.example.toml")).unwrap();
}

#[test]
fn json_example_is_valid_json() {
    serde_json::from_str::<serde_json::Value>(include_str!("../config.example.json")).unwrap();
}
