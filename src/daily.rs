use crate::record::{Header, Record};
use crate::{SCHEMA_MD, SCHEMA_VERSION, VERSION};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn day_path(logs: &Path, date: &str) -> PathBuf {
    logs.join(format!("{date}.jsonl"))
}

pub fn report_path(logs: &Path, date: &str) -> PathBuf {
    logs.join(format!("{date}.report.md"))
}

/// Dates (YYYY-MM-DD) that have a JSONL file in the log folder, oldest first.
pub fn logged_dates(logs: &Path) -> Vec<String> {
    let mut dates: Vec<String> = std::fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let date = name.strip_suffix(".jsonl")?;
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            Some(date.to_string())
        })
        .collect();
    dates.sort();
    dates
}

/// Appends records to one JSONL file per local day.
pub struct DailyLog {
    dir: PathBuf,
    date: String,
    file: Option<File>,
    host: String,
    total_mb: u32,
    own_apps: Vec<String>,
}

impl DailyLog {
    pub fn new(dir: PathBuf, total_mb: u32, own_apps: Vec<String>) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let schema = dir.join("SCHEMA.md");
        if std::fs::read_to_string(&schema).ok().as_deref() != Some(SCHEMA_MD) {
            std::fs::write(&schema, SCHEMA_MD)?;
        }
        Ok(DailyLog {
            dir,
            date: String::new(),
            file: None,
            host: std::env::var("COMPUTERNAME").unwrap_or_default(),
            total_mb,
            own_apps,
        })
    }

    /// Writes one record to the file for the date in `ts`.
    /// Returns the previous date when this record is the first of a new day.
    pub fn append(&mut self, ts: &str, rec: &Record) -> io::Result<Option<String>> {
        let date = &ts[..10];
        let mut finished = None;
        if self.file.is_none() || self.date != date {
            if self.file.is_some() {
                finished = Some(self.date.clone());
            }
            let mut file =
                OpenOptions::new().create(true).append(true).open(day_path(&self.dir, date))?;
            if file.metadata()?.len() == 0 {
                let header = Record::Header(Header {
                    schema: SCHEMA_VERSION,
                    tool: "ramlog".to_string(),
                    version: VERSION.to_string(),
                    host: self.host.clone(),
                    date: date.to_string(),
                    total_mb: self.total_mb,
                    own_apps: self.own_apps.clone(),
                    doc: "SCHEMA.md in this folder describes every field".to_string(),
                });
                write_line(&mut file, &header)?;
            }
            self.file = Some(file);
            self.date = date.to_string();
        }
        if let Some(file) = self.file.as_mut() {
            write_line(file, rec)?;
        }
        Ok(finished)
    }
}

fn write_line(file: &mut File, rec: &Record) -> io::Result<()> {
    let mut line = serde_json::to_string(rec)?;
    line.push('\n');
    // One write call per line keeps lines whole when another reader tails the file.
    file.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Event;

    #[test]
    fn new_day_starts_new_file_and_reports_previous_date() {
        let dir = std::env::temp_dir().join(format!("ramlog-test-{}", std::process::id()));
        let mut log = DailyLog::new(dir.clone(), 16000, vec!["afk".into()]).unwrap();
        let late = "2026-10-16T23:59:50+05:30";
        let early = "2026-10-17T00:00:20+05:30";
        assert_eq!(log.append(late, &Record::Event(Event::new(late, "start"))).unwrap(), None);
        assert_eq!(
            log.append(early, &Record::Event(Event::new(early, "resume"))).unwrap(),
            Some("2026-10-16".to_string())
        );
        let second = std::fs::read_to_string(day_path(&dir, "2026-10-17")).unwrap();
        assert!(second.starts_with(r#"{"t":"header""#));
        assert_eq!(second.lines().count(), 2);
        assert_eq!(logged_dates(&dir), vec!["2026-10-16", "2026-10-17"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
