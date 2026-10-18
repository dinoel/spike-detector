//! Log of announced alerts in JSON Lines: one object per line, easy to grep
//! and read back by the history window.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::alerts::Alert;
use crate::detect::FindingKind;

/// Past this size the log is moved to `*.1.jsonl` and started afresh.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub struct History {
    path: PathBuf,
}

/// One announced alert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub unix_time: u64,
    pub kind: FindingKind,
    pub pid: u32,
    pub name: String,
    pub summary: String,
    pub path: Option<String>,
    pub command_line: Option<String>,
}

impl History {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn append(&self, alert: &Alert) -> io::Result<()> {
        self.rotate_if_large()?;
        let entry = Entry {
            unix_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            kind: alert.finding.kind,
            pid: alert.finding.key.pid,
            name: alert.name.clone(),
            summary: alert.finding.summary.clone(),
            path: alert.details.path.clone(),
            command_line: alert.details.command_line.clone(),
        };
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(&line)
    }

    /// Every logged alert, newest first, including the rotated-out file.
    /// Lines that don't parse (a write cut short, say) are skipped.
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for path in [self.rotated_path(), self.path.clone()] {
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            entries.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str(line).ok()),
            );
        }
        entries.reverse();
        Ok(entries)
    }

    fn rotated_path(&self) -> PathBuf {
        self.path.with_extension("1.jsonl")
    }

    fn rotate_if_large(&self) -> io::Result<()> {
        match fs::metadata(&self.path) {
            Ok(meta) if meta.len() > MAX_BYTES => fs::rename(&self.path, self.rotated_path()),
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

    fn alert(pid: u32) -> Alert {
        Alert {
            finding: Finding {
                key: ProcessKey {
                    pid,
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
        history.append(&alert(42)).unwrap();
        history.append(&alert(42)).unwrap();

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
        History::new(path.clone()).append(&alert(42)).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert!(dir.join("history.1.jsonl").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_back_newest_first_across_rotation() {
        let dir = temp_dir("read");
        let history = History::new(dir.join("history.jsonl"));
        assert!(history.entries().unwrap().is_empty());

        history.append(&alert(1)).unwrap();
        fs::rename(dir.join("history.jsonl"), dir.join("history.1.jsonl")).unwrap();
        history.append(&alert(2)).unwrap();
        // A torn last line must not hide the rest.
        OpenOptions::new()
            .append(true)
            .open(dir.join("history.jsonl"))
            .unwrap()
            .write_all(b"{\"unix_time\":")
            .unwrap();

        let pids: Vec<u32> = history.entries().unwrap().iter().map(|e| e.pid).collect();
        assert_eq!(pids, [2, 1]);
        fs::remove_dir_all(dir).unwrap();
    }
}
