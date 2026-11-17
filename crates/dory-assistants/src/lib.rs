//! The control channel between `dory-mcp` and the running app: a Unix socket in the data folder.
//!
//! The app, which holds the data folder's lock, serves `<data>/run/control.sock`. A helper
//! connects, writes one JSON line with its key and the change it wants, and reads one JSON line
//! back once the app decided. The app checks that the peer runs as the same user and drops
//! anything else. The wire format lives only here.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use dory_core::domain::{
    ClientKey, Exposure, RecordingId, Refusal, RemoteCommand, RemoteOutcome, RemoteRequest,
    Revision,
};
use dory_core::ports::Remote;
use serde_json::{Value, json};

const VERSION: u64 = 1;
const MAX_LINE: u64 = 8 * 1024;
const READ_DEADLINE: Duration = Duration::from_secs(2);
/// A helper waits a little longer than the app waits for the user.
const ANSWER_DEADLINE: Duration = Duration::from_secs(100);
/// `sun_path` on macOS holds 104 bytes including the terminating zero.
const MAX_SOCKET_PATH: usize = 103;

/// Where the socket lives for a data folder.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("run").join("control.sock")
}

#[derive(Debug)]
pub enum ControlError {
    PathTooLong(PathBuf),
    Io(std::io::Error),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::PathTooLong(p) => write!(
                f,
                "the control socket path {} is longer than macOS allows, so assistants cannot ask for changes",
                p.display()
            ),
            ControlError::Io(e) => write!(f, "cannot serve the control socket: {e}"),
        }
    }
}

impl std::error::Error for ControlError {}

type Handler = Arc<dyn Fn(RemoteRequest) -> RemoteOutcome + Send + Sync>;

/// Serves the socket until dropped, then removes it.
pub struct ControlServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Binds the socket for `data_dir`. Only the process holding the data folder's lock calls
    /// this, so any socket file already there is stale and is replaced.
    pub fn serve(data_dir: &Path, handler: Handler) -> Result<ControlServer, ControlError> {
        let path = socket_path(data_dir);
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(ControlError::PathTooLong(path));
        }
        let run = path.parent().expect("socket path has a parent");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(run)
            .map_err(ControlError::Io)?;
        std::fs::set_permissions(run, std::fs::Permissions::from_mode(0o700))
            .map_err(ControlError::Io)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ControlError::Io(e)),
        }
        let listener = UnixListener::bind(&path).map_err(ControlError::Io)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(ControlError::Io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("dory-control".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let Ok(stream) = stream else { continue };
                        let handler = Arc::clone(&handler);
                        let _ = std::thread::Builder::new()
                            .name("dory-control-request".into())
                            .spawn(move || serve_one(stream, handler.as_ref()));
                    }
                })
                .map_err(ControlError::Io)?
        };
        Ok(ControlServer {
            path,
            stop,
            accept: Some(accept),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn same_user(stream: &UnixStream) -> bool {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the descriptor is open for the stream's lifetime and both out-pointers are valid.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    // SAFETY: geteuid has no preconditions.
    ok && uid == unsafe { libc::geteuid() }
}

fn serve_one(stream: UnixStream, handler: &(dyn Fn(RemoteRequest) -> RemoteOutcome + Send + Sync)) {
    if !same_user(&stream) {
        eprintln!("control: dropped a connection from another user");
        return;
    }
    let _ = stream.set_read_timeout(Some(READ_DEADLINE));
    let mut line = String::new();
    let read = BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line);
    let outcome = match read {
        Ok(n) if n > 0 && line.ends_with('\n') => match decode_request(&line) {
            Some(request) => handler(request),
            None => Err(Refusal::Library("the request was not understood".into())),
        },
        _ => return,
    };
    let mut reply = encode_outcome(&outcome).to_string();
    reply.push('\n');
    let _ = (&stream).write_all(reply.as_bytes());
}

/// The helper's side of the channel.
pub struct SocketRemote {
    path: PathBuf,
}

impl SocketRemote {
    pub fn new(data_dir: &Path) -> SocketRemote {
        SocketRemote {
            path: socket_path(data_dir),
        }
    }
}

impl Remote for SocketRemote {
    fn send(&self, request: &RemoteRequest) -> RemoteOutcome {
        let mut stream = UnixStream::connect(&self.path).map_err(|_| Refusal::AppNotRunning)?;
        let mut line = encode_request(request).to_string();
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|_| Refusal::AppNotRunning)?;
        let _ = stream.set_read_timeout(Some(ANSWER_DEADLINE));
        let mut reply = String::new();
        match BufReader::new(stream.take(MAX_LINE)).read_line(&mut reply) {
            Ok(n) if n > 0 => decode_outcome(&reply).unwrap_or_else(|| {
                Err(Refusal::Library("Dory's answer was not understood".into()))
            }),
            Ok(_) => Err(Refusal::AppNotRunning),
            Err(_) => Err(Refusal::Unanswered),
        }
    }
}

fn encode_request(r: &RemoteRequest) -> Value {
    let mut v = json!({
        "v": VERSION,
        "key": r.key.as_ref().map(ClientKey::as_str),
        "reported": r.reported_name,
    });
    match &r.command {
        RemoteCommand::SetTitle { id, title } => {
            v["op"] = json!("set_title");
            v["id"] = json!(id.as_str());
            v["title"] = json!(title);
        }
        RemoteCommand::RenameSpeaker {
            id,
            speaker,
            revision,
            name,
        } => {
            v["op"] = json!("rename_speaker");
            v["id"] = json!(id.as_str());
            v["speaker"] = json!(speaker);
            v["revision"] = json!(revision.0);
            v["name"] = json!(name);
        }
        RemoteCommand::Resummarize { id, instruction } => {
            v["op"] = json!("resummarize");
            v["id"] = json!(id.as_str());
            v["instruction"] = json!(instruction);
        }
    }
    v
}

fn decode_request(line: &str) -> Option<RemoteRequest> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let id = RecordingId::parse(v.get("id")?.as_str()?).ok()?;
    let command = match v.get("op")?.as_str()? {
        "set_title" => RemoteCommand::SetTitle {
            id,
            title: text("title"),
        },
        "rename_speaker" => RemoteCommand::RenameSpeaker {
            id,
            speaker: text("speaker")?,
            revision: Revision(u32::try_from(v.get("revision")?.as_u64()?).ok()?),
            name: text("name"),
        },
        "resummarize" => RemoteCommand::Resummarize {
            id,
            instruction: text("instruction")?,
        },
        _ => return None,
    };
    Some(RemoteRequest {
        key: text("key").and_then(|k| ClientKey::parse(&k)),
        reported_name: text("reported"),
        command,
    })
}

fn refusal_code(why: &Refusal) -> (&'static str, Option<&'static str>) {
    match why {
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
    }
}

fn encode_outcome(outcome: &RemoteOutcome) -> Value {
    match outcome {
        Ok(()) => json!({"v": VERSION, "ok": true}),
        Err(why) => {
            let (code, need) = refusal_code(why);
            let detail = match why {
                Refusal::Library(d) => Some(d.as_str()),
                _ => None,
            };
            json!({"v": VERSION, "ok": false, "why": code, "need": need, "detail": detail})
        }
    }
}

fn decode_outcome(line: &str) -> Option<RemoteOutcome> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("v")?.as_u64()? != VERSION {
        return None;
    }
    if v.get("ok")?.as_bool()? {
        return Some(Ok(()));
    }
    Some(Err(match v.get("why")?.as_str()? {
        "server_off" => Refusal::ServerOff,
        "no_key" => Refusal::NoKey,
        "unknown_key" => Refusal::UnknownKey,
        "not_found" => Refusal::NotFound,
        "not_shared" => Refusal::NotShared {
            need: Exposure::parse(v.get("need")?.as_str()?)?,
        },
        "writes_off" => Refusal::WritesOff,
        "stale_revision" => Refusal::StaleRevision,
        "app_not_running" => Refusal::AppNotRunning,
        "declined" => Refusal::Declined,
        "unanswered" => Refusal::Unanswered,
        "no_window" => Refusal::NoWindow,
        "library" => Refusal::Library(
            v.get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_outcomes_round_trip() {
        let requests = [
            RemoteRequest {
                key: Some(ClientKey::from_bytes([5; 32])),
                reported_name: Some("claude-code".into()),
                command: RemoteCommand::SetTitle {
                    id: RecordingId::parse("a").unwrap(),
                    title: Some("Q3 planning".into()),
                },
            },
            RemoteRequest {
                key: None,
                reported_name: None,
                command: RemoteCommand::RenameSpeaker {
                    id: RecordingId::parse("a").unwrap(),
                    speaker: "Speaker 2".into(),
                    revision: Revision(3),
                    name: None,
                },
            },
        ];
        for r in requests {
            assert_eq!(decode_request(&encode_request(&r).to_string()), Some(r));
        }
        let outcomes = [
            Ok(()),
            Err(Refusal::Declined),
            Err(Refusal::NotShared {
                need: Exposure::Summaries,
            }),
            Err(Refusal::Library("disk".into())),
        ];
        for o in outcomes {
            assert_eq!(decode_outcome(&encode_outcome(&o).to_string()), Some(o));
        }
        assert_eq!(decode_request("{\"v\":2}"), None);
    }
}
