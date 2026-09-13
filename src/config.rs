use serde::{Deserialize, Serialize};
use toml;

use std::*;

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    compiler_path: String,
    compiler_version: Option<String>,
    compiler_options: Option<String>,
    wibo_path: Option<path::PathBuf>,
    default_gpr_helper_mask: u32,
    default_fpr_helper_mask: u32,
    translation_units: Vec<TUConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TUConfig {
    name: String,
    gpr_helper_mask: u32,
    fpr_helper_mask: u32,
}

static CONFIG: sync::OnceLock<Config> = sync::OnceLock::new();
static WIBO_PATH: sync::OnceLock<path::PathBuf> = sync::OnceLock::new();

pub fn initialize(config_path: &path::Path) {
    let contents = fs::read_to_string(config_path).expect("Failed to read config file");
    let config: Config = toml::from_str(&contents).expect("Failed to parse config file");
    CONFIG.set(config).expect("Failed to set config");
}

pub fn compiler_path() -> path::PathBuf {
    return path::Path::new(&get().compiler_path).to_path_buf();
}

pub fn compiler_version() -> Option<String> {
    return get().compiler_version.clone();
}

pub fn compiler_options() -> Vec<String> {
    let options = get().compiler_options.as_deref().unwrap_or_default();
    shlex::split(options).expect("Failed to parse compiler options")
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
                    "wibo_path has not been configured and WIBO_PATH environment variable is not set",
                )
            })?
    };

    let _ = WIBO_PATH.set(resolved_path);
    Ok(WIBO_PATH.get().expect("wibo path was not cached"))
}

pub fn gpr_helper_mask(tu_name: &str) -> u32 {
    let config = get();
    for tu in &config.translation_units {
        if tu.name == tu_name {
            return tu.gpr_helper_mask;
        }
    }
    config.default_gpr_helper_mask
}

pub fn fpr_helper_mask(tu_name: &str) -> u32 {
    let config = get();
    for tu in &config.translation_units {
        if tu.name == tu_name {
            return tu.fpr_helper_mask;
        }
    }
    config.default_fpr_helper_mask
}

fn get() -> &'static Config {
    CONFIG.get().expect("Config not initialized")
}
