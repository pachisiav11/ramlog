use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// One line of a daily JSONL file. The "t" field names the kind.
#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Record {
    Header(Header),
    Sample(Sample),
    Event(Event),
}

#[derive(Serialize, Deserialize)]
pub struct Header {
    pub schema: u32,
    pub tool: String,
    pub version: String,
    pub host: String,
    pub date: String,
    pub total_mb: u32,
    pub own_apps: Vec<String>,
    pub doc: String,
}

#[derive(Serialize, Deserialize)]
pub struct Sample {
    pub ts: String,
    pub int_s: u32,
    pub idle_s: u32,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_exe: Option<String>,
    pub sys: Sys,
    pub apps: Vec<App>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Sys {
    pub total_mb: u32,
    pub avail_mb: u32,
    pub used_pct: u32,
    pub commit_mb: u32,
    pub commit_limit_mb: u32,
    pub pagefile_mb: u32,
    pub compressed_mb: u32,
    pub cache_mb: u32,
    pub kernel_paged_mb: u32,
    pub kernel_nonpaged_mb: u32,
    pub procs: u32,
}

#[derive(Serialize, Deserialize)]
pub struct App {
    pub app: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub own: bool,
    pub n: u32,
    pub mem_mb: u32,
    pub commit_mb: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, u32>,
}

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub ts: String,
    pub ev: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl Event {
    pub fn new(ts: &str, ev: &str) -> Self {
        Event { ts: ts.to_string(), ev: ev.to_string(), data: Map::new() }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.data.insert(key.to_string(), value.into());
        self
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Local time with UTC offset, for example 2026-10-16T22:15:30+05:30.
pub fn now_ts() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
