use crate::VERSION;
use crate::daily::{day_path, report_path};
use crate::record::{Record, Sample};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Windows core processes, left out of the "heavy in the background" list.
const CORE: &[&str] = &[
    "(other)", "system", "registry", "secure system", "svchost", "dwm", "csrss", "lsass", "smss",
    "wininit", "winlogon", "services", "lsaiso",
];

#[derive(Default)]
struct AppStats {
    own: bool,
    secs: u64,
    mem_secs: u64,
    n_secs: u64,
    peak: u32,
    peak_ts: String,
    mems: Vec<u32>,
}

struct Moment {
    avail: u32,
    ts: String,
    fg: String,
    top: String,
}

#[derive(Default)]
struct Day {
    samples: u32,
    first: String,
    last: String,
    secs: u64,
    active_secs: u64,
    total_mb: u32,
    avail: Vec<(u32, u32)>,
    low_1g: u64,
    low_512: u64,
    commit_peak: u32,
    commit_limit: u32,
    pagefile_peak: u32,
    compressed_peak: u32,
    compressed_secs: u64,
    events: BTreeMap<String, u32>,
    gap_secs: u64,
    apps: HashMap<String, AppStats>,
    focus: HashMap<String, u64>,
    moments: Vec<Moment>,
    skipped: u32,
}

impl Day {
    fn add(&mut self, s: Sample) {
        let w = s.int_s as u64;
        self.samples += 1;
        if self.first.is_empty() {
            self.first = s.ts.clone();
        }
        self.last = s.ts.clone();
        self.secs += w;
        let sys = &s.sys;
        self.total_mb = sys.total_mb;
        self.avail.push((sys.avail_mb, s.int_s));
        if sys.avail_mb < 1024 {
            self.low_1g += w;
        }
        if sys.avail_mb < 512 {
            self.low_512 += w;
        }
        self.commit_peak = self.commit_peak.max(sys.commit_mb);
        self.commit_limit = self.commit_limit.max(sys.commit_limit_mb);
        self.pagefile_peak = self.pagefile_peak.max(sys.pagefile_mb);
        self.compressed_peak = self.compressed_peak.max(sys.compressed_mb);
        self.compressed_secs += sys.compressed_mb as u64 * w;

        let fg = s.fg.clone().unwrap_or_default();
        if s.active {
            self.active_secs += w;
            if !fg.is_empty() {
                *self.focus.entry(fg.clone()).or_default() += w;
            }
        }
        for a in &s.apps {
            let st = self.apps.entry(a.app.clone()).or_default();
            st.own |= a.own;
            st.secs += w;
            st.mem_secs += a.mem_mb as u64 * w;
            st.n_secs += a.n as u64 * w;
            if a.mem_mb > st.peak {
                st.peak = a.mem_mb;
                st.peak_ts = s.ts.clone();
            }
            st.mems.push(a.mem_mb);
        }
        let top = s
            .apps
            .iter()
            .filter(|a| a.app != "(other)")
            .take(5)
            .map(|a| format!("{} {}", a.app, a.mem_mb))
            .collect::<Vec<_>>()
            .join(", ");
        self.moments.push(Moment { avail: sys.avail_mb, ts: s.ts, fg, top });
    }
}

/// Builds the report for `date` and saves it next to the day's JSONL file.
pub fn write(logs: &Path, date: &str) -> io::Result<(PathBuf, String)> {
    let text = build(&day_path(logs, date), date)?;
    let out = report_path(logs, date);
    std::fs::write(&out, &text)?;
    Ok((out, text))
}

pub fn build(path: &Path, date: &str) -> io::Result<String> {
    let mut day = Day::default();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Sample(s)) => day.add(s),
            Ok(Record::Event(e)) => {
                if e.ev == "gap" {
                    day.gap_secs += e.data.get("secs").and_then(|v| v.as_u64()).unwrap_or(0);
                }
                *day.events.entry(e.ev).or_default() += 1;
            }
            Ok(Record::Header(_)) => {}
            // A line can be cut short while the logger is writing it.
            Err(_) => day.skipped += 1,
        }
    }
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(render(&day, date, &file))
}

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

fn hours(secs: u64) -> String {
    if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{:.1} h", secs as f64 / 3600.0)
    }
}

fn hhmm(ts: &str) -> &str {
    ts.get(11..16).unwrap_or(ts)
}

fn secs_of_day(ts: &str) -> i64 {
    let part = |r: std::ops::Range<usize>| ts.get(r).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    part(11..13) * 3600 + part(14..16) * 60 + part(17..19)
}

fn pct(part: u64, whole: u64) -> u64 {
    if whole == 0 { 0 } else { part * 100 / whole }
}

fn render(d: &Day, date: &str, file: &str) -> String {
    let mut o = String::new();
    macro_rules! w {
        ($($arg:tt)*) => { let _ = writeln!(o, $($arg)*); };
    }

    w!("# RAM report: {date}\n");
    w!(
        "Made by ramlog {VERSION} from `{file}`. \"Memory\" is the private working set \
         (the Task Manager Memory column). Averages are weighted by time. Field meanings: `SCHEMA.md`.\n"
    );
    if d.samples == 0 {
        w!("No samples on this day.");
        return o;
    }

    w!("## Coverage\n");
    w!("| Item | Value |\n|---|---|");
    w!("| Samples | {} |", d.samples);
    w!("| First / last sample | {} / {} |", hhmm(&d.first), hhmm(&d.last));
    w!(
        "| Time covered | {} (active {}, idle {}) |",
        hours(d.secs),
        hours(d.active_secs),
        hours(d.secs - d.active_secs)
    );
    w!("| Gaps inside the day (sleep, logger off) | {} |", hours(d.gap_secs));
    let events = d.events.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
    w!("| Events | {} |", if events.is_empty() { "none".to_string() } else { events });
    if d.skipped > 0 {
        w!("| Unreadable lines skipped | {} |", d.skipped);
    }

    let mut avail = d.avail.clone();
    avail.sort_unstable();
    let weight: u64 = avail.iter().map(|(_, w)| *w as u64).sum();
    let avg_avail = avail.iter().map(|(a, w)| *a as u64 * *w as u64).sum::<u64>() / weight.max(1);
    let mut acc = 0u64;
    let median = avail
        .iter()
        .find(|(_, w)| {
            acc += *w as u64;
            acc * 2 >= weight
        })
        .map_or(0, |(a, _)| *a as u64);
    let min_avail = avail.first().map_or(0, |(a, _)| *a as u64);

    w!("\n## Memory pressure\n");
    w!("| Metric | Value |\n|---|---|");
    w!("| Installed RAM (usable) | {} |", gb(d.total_mb as u64));
    w!(
        "| Available RAM: average / median / lowest | {} / {} / {} |",
        gb(avg_avail),
        gb(median),
        gb(min_avail)
    );
    w!("| Time with less than 1 GB available | {} ({}%) |", hours(d.low_1g), pct(d.low_1g, d.secs));
    w!("| Time with less than 512 MB available | {} ({}%) |", hours(d.low_512), pct(d.low_512, d.secs));
    w!(
        "| Commit: peak / limit | {} / {} |",
        gb(d.commit_peak as u64),
        gb(d.commit_limit as u64)
    );
    w!("| Page file in use: peak | {} |", gb(d.pagefile_peak as u64));
    w!(
        "| Compressed memory: average / peak | {} / {} |",
        gb(d.compressed_secs / d.secs.max(1)),
        gb(d.compressed_peak as u64)
    );

    let mut ranked: Vec<(&String, &AppStats)> = d.apps.iter().collect();
    ranked.sort_by(|a, b| b.1.mem_secs.cmp(&a.1.mem_secs).then_with(|| a.0.cmp(b.0)));

    w!("\n## Top apps by average memory\n");
    w!("\"Avg (day)\" spreads the app's memory over all covered time, so it shows the total load. \"Avg (running)\" counts only the time the app ran.\n");
    app_table(&mut o, d, ranked.iter().take(25).copied());

    w!("\n## Your apps\n");
    let own: Vec<_> = ranked.iter().filter(|(_, s)| s.own).copied().collect();
    if own.is_empty() {
        w!("None of your apps ran on this day.");
    } else {
        app_table(&mut o, d, own.into_iter());
    }

    w!("\n## Heavy in the background\n");
    w!("Apps that ran 1 h or more, used 150 MB or more on average, and had focus less than 5% of the time they ran. Windows core processes are left out.\n");
    let background: Vec<_> = ranked
        .iter()
        .filter(|(name, s)| {
            let focus = d.focus.get(*name).copied().unwrap_or(0);
            !CORE.contains(&name.as_str())
                && s.secs >= 3600
                && s.mem_secs / s.secs >= 150
                && focus * 20 < s.secs
        })
        .copied()
        .collect();
    if background.is_empty() {
        w!("None.");
    } else {
        app_table(&mut o, d, background.into_iter());
    }

    w!("\n## Growth during the day\n");
    w!("Average of the app's first 10% of samples against its last 10%. A rise can be a leak or a normal cache; check the raw samples.\n");
    let mut growth: Vec<(&String, u64, u64)> = ranked
        .iter()
        .filter(|(_, s)| s.mems.len() >= 20 && s.secs >= 3600)
        .filter_map(|(name, s)| {
            let k = (s.mems.len() / 10).max(3);
            let first = s.mems[..k].iter().map(|m| *m as u64).sum::<u64>() / k as u64;
            let last = s.mems[s.mems.len() - k..].iter().map(|m| *m as u64).sum::<u64>() / k as u64;
            (last >= first + 100 && last * 10 >= first * 13).then_some((*name, first, last))
        })
        .collect();
    growth.sort_by(|a, b| (b.2 - b.1).cmp(&(a.2 - a.1)));
    if growth.is_empty() {
        w!("No app grew by 100 MB and 30% or more.");
    } else {
        w!("| App | First 10% avg MB | Last 10% avg MB | Change MB |\n|---|---:|---:|---:|");
        for (name, first, last) in growth.iter().take(10) {
            w!("| {name} | {first} | {last} | +{} |", last - first);
        }
    }

    w!("\n## Lowest-RAM moments\n");
    w!("The 5 samples with the least available RAM, at least 30 minutes apart.\n");
    let mut moments: Vec<&Moment> = d.moments.iter().collect();
    moments.sort_by_key(|m| m.avail);
    let mut picked: Vec<&Moment> = Vec::new();
    for m in moments {
        if picked.len() == 5 {
            break;
        }
        if picked.iter().all(|p| (secs_of_day(&p.ts) - secs_of_day(&m.ts)).abs() >= 1800) {
            picked.push(m);
        }
    }
    w!("| Time | Available | Focus app | Top 5 apps (MB) |\n|---|---:|---|---|");
    for m in picked {
        let fg = if m.fg.is_empty() { "-" } else { &m.fg };
        w!("| {} | {} MB | {} | {} |", hhmm(&m.ts), m.avail, fg, m.top);
    }

    w!("\n## Focus time\n");
    w!("Time each app had focus while you were active.\n");
    let mut focus: Vec<(&String, &u64)> = d.focus.iter().collect();
    focus.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    w!("| App | Focus | Avg MB (running) |\n|---|---:|---:|");
    for (name, secs) in focus.into_iter().take(10) {
        let avg = d.apps.get(name).map_or(0, |s| s.mem_secs / s.secs.max(1));
        w!("| {name} | {} | {avg} |", hours(*secs));
    }
    o
}

fn app_table<'a>(o: &mut String, d: &Day, rows: impl Iterator<Item = (&'a String, &'a AppStats)>) {
    let _ = writeln!(
        o,
        "| App | Own | Avg MB (day) | Avg MB (running) | Peak MB | Peak at | Running | Procs | Focus |\n\
         |---|---|---:|---:|---:|---|---:|---:|---:|"
    );
    for (name, s) in rows {
        let focus = d.focus.get(name).copied().unwrap_or(0);
        let _ = writeln!(
            o,
            "| {name} | {} | {} | {} | {} | {} | {} | {:.1} | {} |",
            if s.own { "yes" } else { "" },
            s.mem_secs / d.secs.max(1),
            s.mem_secs / s.secs.max(1),
            s.peak,
            hhmm(&s.peak_ts),
            hours(s.secs),
            s.n_secs as f64 / s.secs.max(1) as f64,
            hours(focus),
        );
    }
}
