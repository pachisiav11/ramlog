use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "ramlog.toml";

const BUILTIN_HELPERS: &[&str] = &[
    "msedgewebview2",
    "conhost",
    "openconsole",
    "crashpad_handler",
    "node",
    "python",
    "pythonw",
    "py",
    "uv",
    "cmd",
    "powershell",
    "pwsh",
    "bash",
    "sh",
    "git",
];

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub active_interval_s: u32,
    pub idle_interval_s: u32,
    pub idle_after_s: u32,
    pub min_app_mb: u32,
    pub own_apps: Vec<String>,
    pub helpers: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            active_interval_s: 30,
            idle_interval_s: 120,
            idle_after_s: 300,
            min_app_mb: 5,
            own_apps: Vec::new(),
            helpers: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(root: &Path) -> Result<Config, String> {
        let path = root.join(CONFIG_FILE);
        let mut cfg: Config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => Config::default(),
        };
        cfg.active_interval_s = cfg.active_interval_s.max(5);
        cfg.idle_interval_s = cfg.idle_interval_s.max(cfg.active_interval_s);
        cfg.own_apps = cfg.own_apps.iter().map(|s| norm(s)).collect();
        cfg.helpers = cfg.helpers.iter().map(|s| norm(s)).collect();
        cfg.helpers.extend(BUILTIN_HELPERS.iter().map(|s| s.to_string()));
        Ok(cfg)
    }

    pub fn is_own(&self, name: &str) -> bool {
        self.own_apps.iter().any(|a| a == name)
    }

    pub fn is_helper(&self, name: &str) -> bool {
        !self.is_own(name) && self.helpers.iter().any(|h| h == name)
    }
}

/// Lowercase exe name without ".exe", the form used everywhere in the logs.
pub fn norm(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => lower,
    }
}

/// The project root: the nearest folder above the running exe that holds ramlog.toml.
pub fn root() -> PathBuf {
    let start = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    start
        .ancestors()
        .find(|dir| dir.join(CONFIG_FILE).is_file())
        .map(Path::to_path_buf)
        .unwrap_or(start)
}

pub fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}
