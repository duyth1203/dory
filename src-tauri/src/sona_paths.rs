use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn data_sidecar_bin(app_handle: &AppHandle, name: &str) -> Option<PathBuf> {
    let dir = app_handle.path().app_local_data_dir().ok()?;
    let p = dir.join("sona-sidecars").join(name);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

pub fn resolve_sona_binary(app_handle: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app_handle
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    let binary_name = "sona.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "sona";

    let sidecar_path = resource_dir.join(binary_name);
    if sidecar_path.exists() {
        return Ok(sidecar_path);
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join(binary_name);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    if let Some(p) = data_sidecar_bin(app_handle, binary_name) {
        return Ok(p);
    }

    if let Ok(path) = which::which(binary_name) {
        return Ok(path);
    }

    Err(
        "Sona was not found. Connect to the internet, open Setup and tap Re-check, or reinstall the app. If you built from source, run prebuild before packaging — see project README."
            .to_string(),
    )
}

pub fn resolve_ffmpeg_path(app_handle: &AppHandle) -> Option<PathBuf> {
    let resource_dir = app_handle.path().resource_dir().ok()?;

    #[cfg(target_os = "windows")]
    let binary_name = "ffmpeg.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "ffmpeg";

    let sidecar_path = resource_dir.join(binary_name);
    if sidecar_path.exists() {
        return Some(sidecar_path);
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join(binary_name);
            if path.exists() {
                return Some(path);
            }
        }
    }

    data_sidecar_bin(app_handle, binary_name)
}

pub fn resolve_diarize_path(app_handle: &AppHandle) -> Option<PathBuf> {
    let resource_dir = app_handle.path().resource_dir().ok()?;

    #[cfg(target_os = "windows")]
    let binary_name = "sona-diarize.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "sona-diarize";

    let sidecar_path = resource_dir.join(binary_name);
    if sidecar_path.exists() {
        return Some(sidecar_path);
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let path = exe_dir.join(binary_name);
            if path.exists() {
                return Some(path);
            }
        }
    }

    data_sidecar_bin(app_handle, binary_name)
}
