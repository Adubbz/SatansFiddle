#[test]
fn example_config_parses() {
    toml::from_str::<toml::Table>(include_str!("../config.example.toml")).unwrap();
}
