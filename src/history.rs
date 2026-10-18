//! Append-only log of announced alerts in JSON Lines: one object per line,
//! easy to grep today and to load into a history view later.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::alerts::Alert;
use crate::detect::FindingKind;

/// Past this size the log is moved to `*.1.jsonl` and started afresh.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub struct History {
    path: PathBuf,
}

#[derive(Serialize)]
struct Record<'a> {
    unix_time: u64,
    kind: FindingKind,
    pid: u32,
    name: &'a str,
    summary: &'a str,
    path: Option<&'a str>,
    command_line: Option<&'a str>,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn append(&self, alert: &Alert) -> io::Result<()> {
        self.rotate_if_large()?;
        let record = Record {
            unix_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            kind: alert.finding.kind,
            pid: alert.finding.key.pid,
            name: &alert.name,
            summary: &alert.finding.summary,
            path: alert.details.path.as_deref(),
            command_line: alert.details.command_line.as_deref(),
        };
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(&line)
    }

    fn rotate_if_large(&self) -> io::Result<()> {
        match fs::metadata(&self.path) {
            Ok(meta) if meta.len() > MAX_BYTES => {
                fs::rename(&self.path, self.path.with_extension("1.jsonl"))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Finding;
    use crate::sampler::ProcessKey;
    use crate::win::process::ProcessDetails;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("spike-detector-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn alert() -> Alert {
        Alert {
            finding: Finding {
                key: ProcessKey {
                    pid: 42,
                    create_time: 1,
                },
                kind: FindingKind::Orphan,
                summary: "running without its parent for 31 min".into(),
            },
            name: "node.exe".into(),
            details: ProcessDetails {
                path: Some(r"C:\Program Files\nodejs\node.exe".into()),
                command_line: None,
            },
        }
    }

    #[test]
    fn appends_one_json_object_per_line() {
        let dir = temp_dir("append");
        let history = History::new(dir.join("history.jsonl"));
        history.append(&alert()).unwrap();
        history.append(&alert()).unwrap();

        let text = fs::read_to_string(dir.join("history.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["kind"], "orphan");
        assert_eq!(lines[0]["pid"], 42);
        assert_eq!(lines[0]["path"], r"C:\Program Files\nodejs\node.exe");
        assert!(lines[0]["command_line"].is_null());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rotates_when_too_large() {
        let dir = temp_dir("rotate");
        let path = dir.join("history.jsonl");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        History::new(path.clone()).append(&alert()).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert!(dir.join("history.1.jsonl").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
