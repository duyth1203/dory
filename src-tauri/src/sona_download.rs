//! When Tauri `externalBin` assets are missing (dev without prebuild, or an incomplete install),
//! download the same GitHub release artifacts as `scripts/pre_build.py` into
//! `{app_local_data}/sona-sidecars/` and resolve paths there.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use flate2::read::GzDecoder;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use zip::ZipArchive;

static DOWNLOAD_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

const USER_AGENT: &str = "dory/1 (+https://github.com/your-username/dory)";

pub fn sona_version_tag() -> &'static str {
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../.sona-version")).trim()
}

pub fn data_sidecar_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("sona-sidecars"))
}

fn host_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// (github asset filename, inner sona name in archive or None if raw binary, inner ffmpeg name)
fn sona_asset(triple: &str) -> Option<(&'static str, Option<&'static str>, Option<&'static str>)> {
    match triple {
        "aarch64-apple-darwin" => Some((
            "sona-darwin-arm64-with-ffmpeg.tar.gz",
            Some("sona"),
            Some("ffmpeg"),
        )),
        "x86_64-apple-darwin" => Some((
            "sona-darwin-amd64-with-ffmpeg.tar.gz",
            Some("sona"),
            Some("ffmpeg"),
        )),
        "x86_64-unknown-linux-gnu" => Some(("sona-linux-amd64", None, None)),
        "aarch64-unknown-linux-gnu" => Some(("sona-linux-arm64", None, None)),
        "x86_64-pc-windows-msvc" => Some((
            "sona-windows-amd64-with-ffmpeg.zip",
            Some("sona.exe"),
            Some("ffmpeg.exe"),
        )),
        _ => None,
    }
}

fn diarize_release_filename(triple: &str) -> Option<&'static str> {
    match triple {
        "aarch64-apple-darwin" => Some("sona-diarize-darwin-arm64"),
        "x86_64-unknown-linux-gnu" => Some("sona-diarize-linux-amd64"),
        "aarch64-unknown-linux-gnu" => Some("sona-diarize-linux-arm64"),
        "x86_64-pc-windows-msvc" => Some("sona-diarize-windows-amd64.exe"),
        _ => None,
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

async fn fetch_url_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    res.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

fn extract_tar_gz_sona_ffmpeg(
    data: &[u8],
    sona_out: &Path,
    ffmpeg_out: &Path,
    sona_member: &str,
    ffmpeg_member: &str,
) -> Result<(), String> {
    let dec = GzDecoder::new(Cursor::new(data));
    let mut arch = tar::Archive::new(dec);
    let mut sona_bytes: Option<Vec<u8>> = None;
    let mut ffmpeg_bytes: Option<Vec<u8>> = None;
    for entry in arch.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == sona_member {
            let mut v = Vec::new();
            std::io::copy(&mut entry, &mut v).map_err(|e| e.to_string())?;
            sona_bytes = Some(v);
        } else if name == ffmpeg_member {
            let mut v = Vec::new();
            std::io::copy(&mut entry, &mut v).map_err(|e| e.to_string())?;
            ffmpeg_bytes = Some(v);
        }
        if sona_bytes.is_some() && ffmpeg_bytes.is_some() {
            break;
        }
    }
    let sb = sona_bytes.ok_or_else(|| "archive missing sona member".to_string())?;
    let fb = ffmpeg_bytes.ok_or_else(|| "archive missing ffmpeg member".to_string())?;
    fs::write(sona_out, sb).map_err(|e| e.to_string())?;
    fs::write(ffmpeg_out, fb).map_err(|e| e.to_string())?;
    set_executable(sona_out)?;
    set_executable(ffmpeg_out)?;
    Ok(())
}

fn extract_zip_sona_ffmpeg(
    data: &[u8],
    sona_out: &Path,
    ffmpeg_out: &Path,
    sona_member: &str,
    ffmpeg_member: &str,
) -> Result<(), String> {
    let cursor = Cursor::new(data);
    let mut zip = ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut got_sona = false;
    let mut got_ff = false;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = Path::new(file.name())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name == sona_member {
            let mut out = fs::File::create(sona_out).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
            got_sona = true;
        } else if name == ffmpeg_member {
            let mut out = fs::File::create(ffmpeg_out).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
            got_ff = true;
        }
        if got_sona && got_ff {
            break;
        }
    }
    if !got_sona || !got_ff {
        return Err("zip missing sona or ffmpeg".to_string());
    }
    set_executable(sona_out)?;
    set_executable(ffmpeg_out)?;
    Ok(())
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn write_diarize_stub_intel_mac(dest: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("mnt_diarize_stub.c");
    fs::write(
        &tmp,
        r#"#include <stdio.h>
int main(void) {
    fprintf(stderr, "sona-diarize is not supported on this platform\n");
    return 1;
}
"#,
    )
    .map_err(|e| e.to_string())?;
    let dest_str = dest.to_str().ok_or_else(|| "path utf8".to_string())?;
    let status = std::process::Command::new("cc")
        .args(["-o", dest_str])
        .arg(&tmp)
        .status()
        .map_err(|e| format!("cc not found (needed for diarize stub on Intel Mac): {e}"))?;
    let _ = fs::remove_file(&tmp);
    if !status.success() {
        return Err("cc failed to build diarize stub".to_string());
    }
    set_executable(dest)?;
    Ok(())
}

async fn fetch_diarize_binary(_app: &AppHandle, triple: &str, dest: &Path) -> Result<(), String> {
    if let Some(asset) = diarize_release_filename(triple) {
        let tag = sona_version_tag();
        let url = format!(
            "https://github.com/thewh1teagle/sona/releases/download/{tag}/{asset}"
        );
        tracing::info!("downloading sona-diarize: {}", url);
        let data = fetch_url_bytes(&url).await?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(dest, data).map_err(|e| e.to_string())?;
        let _ = set_executable(dest);
        return Ok(());
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        return write_diarize_stub_intel_mac(dest);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(
            dest,
            "@echo off\r\necho sona-diarize is not supported on this platform\r\nexit /b 1\r\n",
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let _ = dest;
    Ok(())
}

async fn install_sona_from_release(app: &AppHandle, triple: &str) -> Result<(), String> {
    let (asset_name, sona_inner, ffmpeg_inner) =
        sona_asset(triple).ok_or_else(|| format!("Unsupported platform for Sona: {triple}"))?;

    let dir = data_sidecar_dir(app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    let (sona_name, ff_name, dz_name) = ("sona.exe", "ffmpeg.exe", "sona-diarize.exe");
    #[cfg(not(target_os = "windows"))]
    let (sona_name, ff_name, dz_name) = ("sona", "ffmpeg", "sona-diarize");

    let sona_path = dir.join(sona_name);
    let ff_path = dir.join(ff_name);
    let dz_path = dir.join(dz_name);

    let tag = sona_version_tag();
    let url = format!(
        "https://github.com/thewh1teagle/sona/releases/download/{tag}/{asset_name}"
    );
    tracing::info!("downloading Sona release asset: {}", url);
    let data = fetch_url_bytes(&url).await?;

    match (sona_inner, ffmpeg_inner) {
        (None, None) => {
            fs::write(&sona_path, data).map_err(|e| e.to_string())?;
            set_executable(&sona_path)?;
        }
        (Some(s), Some(f)) if asset_name.ends_with(".tar.gz") => {
            extract_tar_gz_sona_ffmpeg(&data, &sona_path, &ff_path, s, f)?;
        }
        (Some(s), Some(f)) if asset_name.ends_with(".zip") => {
            extract_zip_sona_ffmpeg(&data, &sona_path, &ff_path, s, f)?;
        }
        _ => {
            return Err(format!("Unexpected sona asset layout: {asset_name}"));
        }
    }

    if !dz_path.exists() {
        let _ = fetch_diarize_binary(app, triple, &dz_path).await;
    }

    Ok(())
}

/// If bundled `sona` is missing, download release artifacts into app data (needs network).
pub async fn ensure_sona_sidecars_present(app: &AppHandle) -> Result<(), String> {
    if crate::sona_paths::resolve_sona_binary(app).is_ok() {
        return Ok(());
    }

    let mtx = DOWNLOAD_MUTEX.get_or_init(|| Mutex::new(()));
    let _guard = mtx.lock().await;
    if crate::sona_paths::resolve_sona_binary(app).is_ok() {
        return Ok(());
    }

    let triple = host_triple()
        .ok_or_else(|| "This OS/CPU is not supported for automatic Sona download.".to_string())?;

    install_sona_from_release(app, triple).await?;
    let _ = app.emit("sona-sidecars-ready", ());
    Ok(())
}
