//! `JsonlAudit`: the audit log of assistant access as JSON lines, one file per writing process.
//!
//! Several `dory-mcp` helpers run at once and none holds the data folder's lock, so no two of them
//! share a file. A writer's file is `<root>/<YYYY-MM-DD>/<actor>-<pid>-<start_ms>.jsonl`, created
//! exclusively, and each entry is one `write_all` of one line. Folders are 0700 and files 0600.
//! The reader merges every file by time and skips a torn last line or a version it does not know.
//! Days are UTC. Only the process that owns the data folder creates the root and deletes old days.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dory_core::domain::{
    Access, Actor, AuditEntry, AuditOutcome, Exposure, RecordingId, Refusal, Timestamp,
    keep_audit_day,
};
use dory_core::ports::{AuditError, AuditRead, AuditSink};
use serde_json::{Value, json};

const VERSION: u64 = 1;
const DAY_MS: i64 = 86_400_000;

pub struct JsonlAudit {
    root: PathBuf,
    writer: Option<Writer>,
}

struct Writer {
    actor: Actor,
    pid: u32,
    started: Timestamp,
    open: Mutex<Option<(i64, File)>>,
}

impl JsonlAudit {
    /// A log that only reads `root`.
    pub fn reader(root: impl Into<PathBuf>) -> JsonlAudit {
        JsonlAudit {
            root: root.into(),
            writer: None,
        }
    }

    /// A log this process appends to as `actor`, starting at `started`.
    pub fn writer(root: impl Into<PathBuf>, actor: Actor, started: Timestamp) -> JsonlAudit {
        JsonlAudit {
            root: root.into(),
            writer: Some(Writer {
                actor,
                pid: std::process::id(),
                started,
                open: Mutex::new(None),
            }),
        }
    }

    /// Creates the root, private to the user. Only the owner of the data folder calls this.
    pub fn create_root(root: &Path) -> std::io::Result<()> {
        DirBuilder::new().recursive(true).mode(0o700).create(root)
    }

    /// Deletes every day folder older than the kept window. Only the owner of the data folder
    /// calls this. Returns how many day folders went.
    pub fn prune(root: &Path, now: Timestamp) -> std::io::Result<usize> {
        let mut removed = 0;
        for (day, path) in days(root)? {
            if !keep_audit_day(Timestamp(day * DAY_MS), now) {
                std::fs::remove_dir_all(path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl AuditSink for JsonlAudit {
    fn append(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        let Some(writer) = &self.writer else {
            return Err(AuditError::Io(std::io::Error::other(
                "a reader cannot append",
            )));
        };
        if !self.root.is_dir() {
            return Err(AuditError::Missing(self.root.clone()));
        }
        let day = entry.at.0.div_euclid(DAY_MS);
        let mut open = writer.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.as_ref().is_none_or(|(d, _)| *d != day) {
            let folder = self.root.join(day_name(day));
            DirBuilder::new()
                .mode(0o700)
                .create(&folder)
                .or_else(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => Ok(()),
                    _ => Err(e),
                })
                .map_err(AuditError::Io)?;
            let path = folder.join(format!(
                "{}-{}-{}.jsonl",
                writer.actor.as_str(),
                writer.pid,
                writer.started.0
            ));
            let file = OpenOptions::new()
                .append(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(AuditError::Io)?;
            *open = Some((day, file));
        }
        let (_, file) = open.as_mut().expect("opened above");
        let mut line = encode(entry).to_string();
        line.push('\n');
        file.write_all(line.as_bytes()).map_err(AuditError::Io)
    }
}

impl AuditRead for JsonlAudit {
    fn since(&self, since: Timestamp, limit: usize) -> Result<Vec<AuditEntry>, AuditError> {
        let mut entries = Vec::new();
        let first_day = since.0.div_euclid(DAY_MS);
        for (day, folder) in days(&self.root).map_err(AuditError::Io)? {
            if day < first_day {
                continue;
            }
            for file in std::fs::read_dir(&folder).map_err(AuditError::Io)? {
                let path = file.map_err(AuditError::Io)?.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let reader = BufReader::new(File::open(&path).map_err(AuditError::Io)?);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if let Some(entry) = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| decode(&v))
                        && entry.at >= since
                    {
                        entries.push(entry);
                    }
                }
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.at));
        entries.truncate(limit);
        Ok(entries)
    }
}

/// Day folders under `root`, as (days since the epoch, path). A missing root has none.
fn days(root: &Path) -> std::io::Result<Vec<(i64, PathBuf)>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        if let Some(day) = entry.file_name().to_str().and_then(parse_day) {
            out.push((day, entry.path()));
        }
    }
    Ok(out)
}

/// `YYYY-MM-DD` for a count of days since 1970-01-01.
fn day_name(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_day(name: &str) -> Option<i64> {
    let mut parts = name.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    let days = days_from_civil(y, m, d);
    (parts.next().is_none() && day_name(days) == name).then_some(days)
}

// Howard Hinnant's civil calendar algorithms, for the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn encode(entry: &AuditEntry) -> Value {
    let access = match &entry.access {
        Access::Search { text, hits } => json!({"kind": "search", "text": text, "hits": hits}),
        Access::Recording(id) => json!({"kind": "recording", "id": id.as_str()}),
        Access::Summary(id) => json!({"kind": "summary", "id": id.as_str()}),
        Access::Transcript { id, turns } => {
            json!({"kind": "transcript", "id": id.as_str(), "from": turns.0, "to": turns.1})
        }
        Access::ActionItems { ids } => json!({
            "kind": "action_items",
            "ids": ids.iter().map(RecordingId::as_str).collect::<Vec<_>>(),
        }),
        Access::ResourceList => json!({"kind": "resource_list"}),
        Access::Prompt { name } => json!({"kind": "prompt", "name": name}),
        Access::Write { what, id } => {
            json!({"kind": "write", "what": what, "id": id.as_ref().map(RecordingId::as_str)})
        }
    };
    let outcome = match &entry.outcome {
        AuditOutcome::Served { items } => json!({"kind": "served", "items": items}),
        AuditOutcome::Refused(why) => {
            let (kind, need) = match why {
                Refusal::ServerOff => ("server_off", None),
                Refusal::NoKey => ("no_key", None),
                Refusal::UnknownKey => ("unknown_key", None),
                Refusal::NotFound => ("not_found", None),
                Refusal::NotShared { need } => ("not_shared", Some(need.as_str())),
                Refusal::WritesOff => ("writes_off", None),
                Refusal::StaleRevision => ("stale_revision", None),
                Refusal::AppNotRunning => ("app_not_running", None),
                Refusal::Declined => ("declined", None),
                Refusal::Unanswered => ("unanswered", None),
                Refusal::NoWindow => ("no_window", None),
                Refusal::Library(_) => ("library", None),
            };
            json!({"kind": "refused", "why": kind, "need": need})
        }
    };
    json!({
        "v": VERSION,
        "at": entry.at.0,
        "actor": entry.actor.as_str(),
        "client": entry.client,
        "reported": entry.reported_name,
        "access": access,
        "outcome": outcome,
    })
}

fn decode(v: &Value) -> Option<AuditEntry> {
    if v.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let id = |v: &Value| {
        v.get("id")?
            .as_str()
            .and_then(|s| RecordingId::parse(s).ok())
    };
    let num = |v: &Value, k: &str| v.get(k)?.as_u64().and_then(|n| u32::try_from(n).ok());
    let a = v.get("access")?;
    let access = match a.get("kind")?.as_str()? {
        "search" => Access::Search {
            text: text(a, "text")?,
            hits: num(a, "hits")?,
        },
        "recording" => Access::Recording(id(a)?),
        "summary" => Access::Summary(id(a)?),
        "transcript" => Access::Transcript {
            id: id(a)?,
            turns: (num(a, "from")?, num(a, "to")?),
        },
        "action_items" => Access::ActionItems {
            ids: a
                .get("ids")?
                .as_array()?
                .iter()
                .map(|s| s.as_str().and_then(|s| RecordingId::parse(s).ok()))
                .collect::<Option<_>>()?,
        },
        "resource_list" => Access::ResourceList,
        "prompt" => Access::Prompt {
            name: text(a, "name")?,
        },
        "write" => Access::Write {
            what: text(a, "what")?,
            id: id(a),
        },
        _ => return None,
    };
    let o = v.get("outcome")?;
    let outcome = match o.get("kind")?.as_str()? {
        "served" => AuditOutcome::Served {
            items: num(o, "items")?,
        },
        "refused" => AuditOutcome::Refused(match o.get("why")?.as_str()? {
            "server_off" => Refusal::ServerOff,
            "no_key" => Refusal::NoKey,
            "unknown_key" => Refusal::UnknownKey,
            "not_found" => Refusal::NotFound,
            "not_shared" => Refusal::NotShared {
                need: Exposure::parse(o.get("need")?.as_str()?)?,
            },
            "writes_off" => Refusal::WritesOff,
            "stale_revision" => Refusal::StaleRevision,
            "app_not_running" => Refusal::AppNotRunning,
            "declined" => Refusal::Declined,
            "unanswered" => Refusal::Unanswered,
            "no_window" => Refusal::NoWindow,
            "library" => Refusal::Library(String::new()),
            _ => return None,
        }),
        _ => return None,
    };
    Some(AuditEntry {
        at: Timestamp(v.get("at")?.as_i64()?),
        actor: match v.get("actor")?.as_str()? {
            "mcp" => Actor::Helper,
            "app" => Actor::App,
            _ => return None,
        },
        client: text(v, "client"),
        reported_name: text(v, "reported"),
        access,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_names_round_trip() {
        assert_eq!(day_name(0), "1970-01-01");
        assert_eq!(day_name(20_409), "2025-11-17");
        assert_eq!(day_name(-1), "1969-12-31");
        assert_eq!(day_name(11_016), "2000-02-29");
        for d in [-800, -1, 0, 59, 60, 11_016, 20_409, 40_000] {
            assert_eq!(parse_day(&day_name(d)), Some(d));
        }
        for bad in [
            "2025-13-01",
            "2025-02-30",
            "2025-1-01",
            "x",
            "2025-01-01-01",
        ] {
            assert_eq!(parse_day(bad), None, "{bad}");
        }
    }
}
