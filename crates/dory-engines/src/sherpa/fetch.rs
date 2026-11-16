//! Fetching a model: resumable download into `.partial/`, SHA-256 check, extraction into a staging
//! directory, then one rename into place. The model directory exists only once it is complete, so
//! every crash leaves a state the next call finishes.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dory_core::ports::{CancelToken, ProvisionError, ProvisionSink};
use sha2::{Digest, Sha256};

use super::catalog::{Layout, Model};

const PARTIAL_DIR: &str = ".partial";
const CHUNK: usize = 256 * 1024;
/// Bounds every socket read, so a stalled server still lets cancel return within a second.
const POLL: Duration = Duration::from_millis(500);
/// Give up after this long with no new bytes.
const STALL: Duration = Duration::from_secs(30);
/// Give up after this many responses in a row that delivered nothing.
const EMPTY_RESPONSES: u32 = 5;

pub(super) fn model_dir(root: &Path, model: &Model) -> PathBuf {
    root.join(model.name)
}

fn partial_path(root: &Path, model: &Model) -> PathBuf {
    root.join(PARTIAL_DIR).join(model.archive.file)
}

pub(super) fn is_installed(dir: &Path, layout: Layout) -> bool {
    layout.files().iter().all(|f| dir.join(f).is_file())
}

/// Bytes still to download, for `NeedsProvision`.
pub(super) fn remaining(root: &Path, model: &Model) -> u64 {
    let have = fs::metadata(partial_path(root, model)).map_or(0, |m| m.len());
    model.archive.bytes.saturating_sub(have)
}

pub(super) fn provision(
    root: &Path,
    base_url: &str,
    model: &Model,
    sink: &mut dyn ProvisionSink,
    cancel: &CancelToken,
) -> Result<(), ProvisionError> {
    let target = model_dir(root, model);
    let partial = partial_path(root, model);
    let partial_dir = root.join(PARTIAL_DIR);
    fs::create_dir_all(&partial_dir).map_err(io_error)?;
    let lock =
        File::create(partial_dir.join(format!("{}.lock", model.archive.file))).map_err(io_error)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Err(ProvisionError::Other(format!(
                "{} is already being downloaded",
                model.display_name
            )));
        }
        Err(fs::TryLockError::Error(e)) => return Err(io_error(e)),
    }
    if is_installed(&target, model.layout) {
        let _ = fs::remove_file(&partial);
        return Ok(());
    }
    remove_stale_staging(root, model)?;

    let url = format!("{}/{}", base_url.trim_end_matches('/'), model.archive.file);
    download(&url, &partial, model.archive.bytes, sink, cancel)?;
    verify(&partial, model, sink, cancel)?;
    unpack(&partial, root, &target, model, sink, cancel)?;
    let _ = fs::remove_file(&partial);
    Ok(())
}

fn staging_prefix(model: &Model) -> String {
    format!(".extract-{}-", model.name)
}

fn remove_stale_staging(root: &Path, model: &Model) -> Result<(), ProvisionError> {
    let prefix = staging_prefix(model);
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            fs::remove_dir_all(entry.path()).map_err(io_error)?;
        }
    }
    Ok(())
}

fn download(
    url: &str,
    partial: &Path,
    total: u64,
    sink: &mut dyn ProvisionSink,
    cancel: &CancelToken,
) -> Result<(), ProvisionError> {
    sink.step("Downloading");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(POLL))
        .build()
        .into();
    let mut last_progress = Instant::now();
    let mut empty = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(ProvisionError::Cancelled);
        }
        let mut have = fs::metadata(partial).map_or(0, |m| m.len());
        if have > total {
            fs::remove_file(partial).map_err(io_error)?;
            have = 0;
        }
        sink.bytes(have, Some(total));
        if have == total {
            return Ok(());
        }
        if last_progress.elapsed() > STALL || empty >= EMPTY_RESPONSES {
            return Err(ProvisionError::Network(format!(
                "download of {url} stalled"
            )));
        }
        let response = match agent
            .get(url)
            .header("Range", format!("bytes={have}-"))
            .call()
        {
            Ok(response) => response,
            Err(ureq::Error::Timeout(_)) => continue,
            Err(e) => return Err(ProvisionError::Network(e.to_string())),
        };
        let mut file = match response.status().as_u16() {
            206 => OpenOptions::new().append(true).create(true).open(partial),
            200 => File::create(partial),
            416 if have > 0 => {
                fs::remove_file(partial).map_err(io_error)?;
                continue;
            }
            status => return Err(ProvisionError::Network(format!("HTTP {status} from {url}"))),
        }
        .map_err(io_error)?;
        let mut body = response.into_body().into_reader();
        let mut buf = vec![0; CHUNK];
        let mut got = 0;
        let mut done = fs::metadata(partial).map_or(0, |m| m.len());
        loop {
            if cancel.is_cancelled() {
                return Err(ProvisionError::Cancelled);
            }
            match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let n = n.min((total - done) as usize);
                    file.write_all(&buf[..n])
                        .map_err(|e| disk_error(e, total - done))?;
                    got += n;
                    done += n as u64;
                    last_progress = Instant::now();
                    sink.bytes(done, Some(total));
                    if done == total {
                        break;
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // A timeout, reset, or early close: the next request resumes from what was kept.
                Err(_) => break,
            }
        }
        empty = if got == 0 { empty + 1 } else { 0 };
    }
}

fn verify(
    partial: &Path,
    model: &Model,
    sink: &mut dyn ProvisionSink,
    cancel: &CancelToken,
) -> Result<(), ProvisionError> {
    sink.step("Verifying");
    let mut file = File::open(partial).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; CHUNK];
    let mut done = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(ProvisionError::Cancelled);
        }
        let n = file.read(&mut buf).map_err(io_error)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        sink.bytes(done, Some(model.archive.bytes));
    }
    if hex(&hasher.finalize()) == model.archive.sha256 {
        Ok(())
    } else {
        let _ = fs::remove_file(partial);
        Err(ProvisionError::ChecksumMismatch)
    }
}

fn unpack(
    archive_path: &Path,
    root: &Path,
    target: &Path,
    model: &Model,
    sink: &mut dyn ProvisionSink,
    cancel: &CancelToken,
) -> Result<(), ProvisionError> {
    sink.step("Unpacking");
    let staging = tempfile::Builder::new()
        .prefix(&staging_prefix(model))
        .tempdir_in(root)
        .map_err(io_error)?;
    let read = Rc::new(Cell::new(0));
    let file = File::open(archive_path).map_err(io_error)?;
    let counted = Counted {
        inner: BufReader::new(file),
        read: Rc::clone(&read),
    };
    let mut archive = tar::Archive::new(bzip2::read::BzDecoder::new(counted));
    let wanted = model.layout.files();
    let mut buf = vec![0; CHUNK];
    for entry in archive.entries().map_err(io_error)? {
        let mut entry = entry.map_err(io_error)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(io_error)?.into_owned();
        let Some(rel) = path
            .strip_prefix(model.archive.root)
            .ok()
            .and_then(Path::to_str)
            .filter(|rel| wanted.contains(rel))
        else {
            continue;
        };
        let dest = staging.path().join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut out = File::create(&dest).map_err(io_error)?;
        loop {
            if cancel.is_cancelled() {
                return Err(ProvisionError::Cancelled);
            }
            let n = entry.read(&mut buf).map_err(io_error)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| disk_error(e, model.disk_bytes))?;
            sink.bytes(read.get(), Some(model.archive.bytes));
        }
    }
    if let Some(missing) = wanted.iter().find(|f| !staging.path().join(f).is_file()) {
        return Err(ProvisionError::Other(format!(
            "{} has no {missing}",
            model.archive.file
        )));
    }
    if target.exists() {
        fs::remove_dir_all(target).map_err(io_error)?;
    }
    fs::rename(staging.path(), target).map_err(io_error)?;
    let _ = staging.keep();
    Ok(())
}

struct Counted<R> {
    inner: R,
    read: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn disk_error(e: io::Error, needed: u64) -> ProvisionError {
    if e.kind() == ErrorKind::StorageFull {
        ProvisionError::DiskFull { needed }
    } else {
        io_error(e)
    }
}

fn io_error(e: io::Error) -> ProvisionError {
    ProvisionError::Other(e.to_string())
}
