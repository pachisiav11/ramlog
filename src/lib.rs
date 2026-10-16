pub mod config;
pub mod daily;
pub mod record;
pub mod report;
pub mod snapshot;
pub mod win;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_MD: &str = include_str!("../SCHEMA.md");
pub const DAEMON_WINDOW_CLASS: &str = "ramlogd.hidden";
pub const MB: u64 = 1024 * 1024;
