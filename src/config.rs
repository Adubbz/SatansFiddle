use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, env, fs, io, path, sync};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    compiler_path: String,
    compiler_version: Option<String>,
    compiler_options: Option<String>,
    wibo_path: Option<path::PathBuf>,
    #[serde(default)]
    default_gpr_helper_mask: u32,
    #[serde(default)]
    default_fpr_helper_mask: u32,
    #[serde(default)]
    translation_units: Vec<TUConfig>,
    #[serde(default)]
    floating_point: FloatingPointConfig,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TUConfig {
    name: String,
    gpr_helper_mask: u32,
    fpr_helper_mask: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Binary32,
    Binary64,
}

#[derive(Debug, Deserialize)]
#[serde(from = "FloatingPointInput")]
pub struct FloatingPointConfig {
    pub default_evaluate_first: bool,
    pub default_literal_reload: bool,
    pub default_literal_reload_explicit: bool,
    pub expression_overrides: Vec<ExpressionOverride>,
    pub literal_overrides: Vec<LiteralOverride>,
}

impl Serialize for FloatingPointConfig {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut fields = serializer.serialize_map(None)?;
        fields.serialize_entry("default_evaluate_first", &self.default_evaluate_first)?;
        if self.default_literal_reload_explicit {
            fields.serialize_entry("default_literal_reload", &self.default_literal_reload)?;
        }
        fields.serialize_entry("expression_overrides", &self.expression_overrides)?;
        fields.serialize_entry("literal_overrides", &self.literal_overrides)?;
        fields.end()
    }
}

impl Default for FloatingPointConfig {
    fn default() -> Self {
        Self {
            default_evaluate_first: false,
            default_literal_reload: true,
            default_literal_reload_explicit: false,
            expression_overrides: Vec::new(),
            literal_overrides: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct FloatingPointInput {
    default_evaluate_first: bool,
    default_literal_reload: Option<bool>,
    expression_overrides: Vec<ExpressionOverride>,
    literal_overrides: Vec<LiteralOverride>,
}

impl From<FloatingPointInput> for FloatingPointConfig {
    fn from(input: FloatingPointInput) -> Self {
        Self {
            default_evaluate_first: input.default_evaluate_first,
            default_literal_reload: input.default_literal_reload.unwrap_or(true),
            default_literal_reload_explicit: input.default_literal_reload.is_some(),
            expression_overrides: input.expression_overrides,
            literal_overrides: input.literal_overrides,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionOverride {
    pub translation_unit: String,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callee: Option<String>,
    pub value_type: ValueType,
    #[serde(with = "hex_bits")]
    pub value_bits: u64,
    pub evaluate_first: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiteralOverride {
    pub translation_unit: String,
    pub value_type: ValueType,
    #[serde(with = "hex_bits")]
    pub value_bits: u64,
    pub reload: bool,
}

mod hex_bits {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        let digits = value.strip_prefix("0x").ok_or_else(|| {
            D::Error::custom("value_bits must be a hexadecimal string starting with 0x")
        })?;
        if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(D::Error::custom(
                "value_bits must contain 1 to 16 hexadecimal digits",
            ));
        }
        u64::from_str_radix(digits, 16).map_err(D::Error::custom)
    }

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("0x{value:016x}"))
    }
}

/// MWCC reports either slash style; selectors identify the source basename.
pub fn translation_unit_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn validate_name(name: &str, label: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "{label} must not be empty");
    ensure!(!name.contains('\0'), "{label} must not contain NUL");
    Ok(())
}

fn validate_bits(value_type: ValueType, bits: u64) -> Result<()> {
    ensure!(
        value_type != ValueType::Binary32 || bits <= u32::MAX as u64,
        "binary32 value_bits exceeds 32 bits"
    );
    Ok(())
}

impl Config {
    fn parse(contents: &str) -> Result<Self> {
        let mut config: Self = if contents.trim_start().starts_with('{') {
            serde_json::from_str(contents).context("Failed to parse JSON config")?
        } else {
            toml::from_str(contents).context("Failed to parse TOML config")?
        };
        validate_name(&config.compiler_path, "compiler_path")?;
        if let Some(version) = &config.compiler_version {
            ensure!(
                matches!(version.as_str(), "2.3.3" | "3.0-011126"),
                "Unsupported compiler_version: {version}"
            );
        }
        shlex::split(config.compiler_options.as_deref().unwrap_or_default())
            .context("Invalid shell quoting in compiler_options")?;
        let mut tus = HashSet::new();
        for tu in &mut config.translation_units {
            tu.name = translation_unit_name(&tu.name).to_owned();
            validate_name(&tu.name, "translation unit name")?;
            ensure!(
                tus.insert(tu.name.clone()),
                "Duplicate translation-unit override: {}",
                tu.name
            );
        }
        let mut expressions = HashSet::new();
        for row in &mut config.floating_point.expression_overrides {
            row.translation_unit = translation_unit_name(&row.translation_unit).to_owned();
            validate_name(&row.translation_unit, "expression translation_unit")?;
            validate_name(&row.function, "expression function")?;
            if let Some(callee) = &row.callee {
                validate_name(callee, "expression callee")?;
            }
            validate_bits(row.value_type, row.value_bits)?;
            ensure!(
                expressions.insert((
                    row.translation_unit.clone(),
                    row.function.clone(),
                    row.callee.clone(),
                    row.value_type,
                    row.value_bits
                )),
                "Duplicate expression selector for {} / {}",
                row.translation_unit,
                row.function
            );
        }
        let mut literals = HashSet::new();
        for row in &mut config.floating_point.literal_overrides {
            row.translation_unit = translation_unit_name(&row.translation_unit).to_owned();
            validate_name(&row.translation_unit, "literal translation_unit")?;
            validate_bits(row.value_type, row.value_bits)?;
            ensure!(
                literals.insert((row.translation_unit.clone(), row.value_type, row.value_bits)),
                "Duplicate literal selector for {}",
                row.translation_unit
            );
        }
        Ok(config)
    }
}

static CONFIG: sync::OnceLock<Config> = sync::OnceLock::new();
static TRANSLATION_UNIT_OVERRIDE: sync::OnceLock<Option<String>> = sync::OnceLock::new();
static WIBO_PATH: sync::OnceLock<path::PathBuf> = sync::OnceLock::new();

pub fn set_translation_unit(name: Option<&str>) -> Result<()> {
    let normalized = name.map(translation_unit_name);
    if let Some(name) = normalized {
        validate_name(name, "translation-unit override")?;
    }
    TRANSLATION_UNIT_OVERRIDE
        .set(normalized.map(str::to_owned))
        .map_err(|_| anyhow::anyhow!("Translation-unit override is already initialized"))
}

pub fn translation_unit_override() -> Option<&'static str> {
    TRANSLATION_UNIT_OVERRIDE
        .get()
        .and_then(|name| name.as_deref())
}

pub fn initialize(config_path: &path::Path) -> Result<()> {
    let contents = fs::read_to_string(config_path).context("Failed to read config file")?;
    let config = Config::parse(&contents)?;
    if CONFIG.set(config).is_err() {
        bail!("Configuration is already initialized; run each compilation in a separate process");
    }
    Ok(())
}

pub fn compiler_path() -> path::PathBuf {
    path::PathBuf::from(&get().compiler_path)
}

pub fn compiler_version() -> Option<String> {
    get().compiler_version.clone()
}

pub fn compiler_options() -> Result<Vec<String>> {
    shlex::split(get().compiler_options.as_deref().unwrap_or_default())
        .context("Invalid shell quoting in compiler_options")
}

pub fn floating_point() -> &'static FloatingPointConfig {
    &get().floating_point
}

pub fn wibo_path() -> io::Result<&'static path::Path> {
    if let Some(path) = WIBO_PATH.get() {
        return Ok(path);
    }
    let resolved_path = if let Some(path) = &get().wibo_path {
        path.clone()
    } else if let Some(path) = env::var_os("WIBO_PATH") {
        path.into()
    } else {
        env::split_paths(&env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join("wibo"))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Configure wibo_path, WIBO_PATH or PATH",
                )
            })?
    };
    let _ = WIBO_PATH.set(resolved_path);
    Ok(WIBO_PATH.get().expect("wibo path was not cached"))
}

pub fn gpr_helper_mask(tu_name: &str) -> u32 {
    let config = get();
    config
        .translation_units
        .iter()
        .find(|tu| tu.name == translation_unit_name(tu_name))
        .map_or(config.default_gpr_helper_mask, |tu| tu.gpr_helper_mask)
}

pub fn fpr_helper_mask(tu_name: &str) -> u32 {
    let config = get();
    config
        .translation_units
        .iter()
        .find(|tu| tu.name == translation_unit_name(tu_name))
        .map_or(config.default_fpr_helper_mask, |tu| tu.fpr_helper_mask)
}

fn get() -> &'static Config {
    CONFIG.get().expect("Config not initialized")
}

#[cfg(test)]
#[path = "../tests/config/mod.rs"]
mod tests;
