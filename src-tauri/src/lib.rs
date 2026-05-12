use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

mod sona;
mod sona_download;
mod sona_paths;

/// Pipeline job aborted because the user cancelled the job (`cancel_pipeline_jobs`).
const PIPELINE_CANCELLED: &str = "pipeline_cancelled_by_user";

/// ONNX Sortformer model (same as Vibe) — speaker diarization up to 4 speakers.
const DIARIZE_MODEL_FILENAME: &str = "diar_streaming_sortformer_4spk-v2.1.onnx";
const DIARIZE_MODEL_URL: &str = "https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.1.onnx";

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct TranscriptionSettings {
    pub sona_model_path: String,
    pub diarize_enabled: bool,
    pub gpu_device: Option<i32>,
}

#[derive(Default)]
pub struct SonaState {
    pub process: Option<sona::SonaProcess>,
    pub loaded_model_path: Option<String>,
    pub loaded_gpu_device: Option<i32>,
}

/// GUI-launched macOS apps use a minimal PATH (no shell profile). Prepend common install dirs
/// so Homebrew (`brew install ffmpeg`) and pip user bins (`~/.local/bin`) are found.
fn apply_user_tool_path(cmd: &mut tokio::process::Command) {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut prefixes: Vec<String> = Vec::new();
    if cfg!(target_os = "macos") {
        if Path::new("/opt/homebrew/bin").is_dir() {
            prefixes.push("/opt/homebrew/bin".into());
        }
        if Path::new("/usr/local/bin").is_dir() {
            prefixes.push("/usr/local/bin".into());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = format!("{home}/.local/bin");
        if Path::new(&p).is_dir() {
            prefixes.push(p);
        }
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let mut seen = HashSet::<String>::new();
    let mut ordered: Vec<String> = Vec::new();
    for pref in prefixes {
        if seen.insert(pref.clone()) {
            ordered.push(pref);
        }
    }
    for segment in path.split(sep).filter(|s| !s.is_empty()) {
        let s = segment.to_string();
        if seen.insert(s.clone()) {
            ordered.push(s);
        }
    }
    cmd.env("PATH", ordered.join(&sep.to_string()));
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub filename: String,
    pub path: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub duration_secs: f64,
    pub created_at: u64,
    pub transcript: Option<String>,
    pub summary: Option<String>,
    /// queued | transcribing | summarizing | done | failed — omit when not yet in pipeline
    #[serde(default)]
    pub pipeline_status: Option<String>,
    #[serde(default)]
    pub pipeline_error: Option<String>,
    #[serde(default)]
    pub pipeline_priority: bool,
}

#[derive(Debug, Serialize)]
pub struct EnqueuePipelineOutcome {
    pub enqueued: u32,
    pub skipped_complete: u32,
}

#[derive(Debug, Deserialize)]
pub struct EnqueuePipelineJobsArgs {
    pub ids: Vec<String>,
    #[serde(default)]
    pub overwrite_complete: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct PipelineQueueSnapshot {
    pub active_id: Option<String>,
    pub queued_ids: Vec<String>,
}

#[tauri::command]
async fn get_pipeline_queue_snapshot(app: AppHandle) -> Result<PipelineQueueSnapshot, String> {
    let coord = app.state::<Arc<PipelineCoordinator>>();
    let queued_ids: Vec<String> = {
        let q = coord.queue.lock().await;
        q.iter().cloned().collect()
    };
    let active_id = {
        let a = coord.active.lock().await;
        a.as_ref().map(|(id, _)| id.clone())
    };
    Ok(PipelineQueueSnapshot {
        active_id,
        queued_ids,
    })
}

#[tauri::command]
async fn get_recordings_dir(app: AppHandle) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let recordings_dir = data_dir.join("recordings");
    std::fs::create_dir_all(&recordings_dir).map_err(|e| e.to_string())?;
    Ok(recordings_dir.to_string_lossy().to_string())
}

#[tauri::command]
async fn save_recording(
    app: AppHandle,
    filename: String,
    data: Vec<u8>,
) -> Result<String, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let recordings_dir = data_dir.join("recordings");
    std::fs::create_dir_all(&recordings_dir).map_err(|e| e.to_string())?;
    let file_path = recordings_dir.join(&filename);
    std::fs::write(&file_path, data).map_err(|e| e.to_string())?;
    Ok(file_path.to_string_lossy().to_string())
}

async fn list_recordings_impl(app: &AppHandle) -> Result<Vec<Recording>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let recordings_dir = data_dir.join("recordings");
    let meta_dir = data_dir.join("metadata");
    std::fs::create_dir_all(&recordings_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&meta_dir).map_err(|e| e.to_string())?;

    let mut recordings: Vec<Recording> = Vec::new();

    let entries = std::fs::read_dir(&recordings_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("webm")
            || path.extension().and_then(|e| e.to_str()) == Some("wav")
        {
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            let meta_path = meta_dir.join(format!("{}.json", id));
            let (
                transcript,
                summary,
                display_name,
                duration_secs,
                created_at,
                pipeline_status,
                pipeline_error,
                pipeline_priority,
            ) = if let Ok(meta_bytes) = std::fs::read(&meta_path) {
                    if let Ok(meta) =
                        serde_json::from_slice::<serde_json::Value>(&meta_bytes)
                    {
                        (
                            meta["transcript"].as_str().map(String::from),
                            meta["summary"].as_str().map(String::from),
                            meta["display_name"].as_str().map(String::from),
                            meta["duration_secs"].as_f64().unwrap_or(0.0),
                            meta["created_at"].as_u64().unwrap_or(0),
                            meta["pipeline_status"].as_str().map(String::from),
                            meta["pipeline_error"].as_str().map(String::from),
                            meta["pipeline_priority"].as_bool().unwrap_or(false),
                        )
                    } else {
                        (
                            None,
                            None,
                            None,
                            0.0,
                            0,
                            None,
                            None,
                            false,
                        )
                    }
                } else {
                    let created_at = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.created().ok())
                        .and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH).ok()
                        })
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (None, None, None, 0.0, created_at, None, None, false)
                };

            recordings.push(Recording {
                id,
                filename,
                path: path.to_string_lossy().to_string(),
                display_name,
                duration_secs,
                created_at,
                transcript,
                summary,
                pipeline_status,
                pipeline_error,
                pipeline_priority,
            });
        }
    }

    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(recordings)
}

#[tauri::command]
async fn list_recordings(app: AppHandle) -> Result<Vec<Recording>, String> {
    list_recordings_impl(&app).await
}

#[tauri::command]
async fn save_metadata(
    app: AppHandle,
    id: String,
    transcript: Option<String>,
    summary: Option<String>,
    duration_secs: f64,
    created_at: u64,
    pipeline_status: Option<String>,
    pipeline_error: Option<String>,
    pipeline_priority: Option<bool>,
) -> Result<(), String> {
    let meta_dir = app_data_dir(&app)?.join("metadata");
    std::fs::create_dir_all(&meta_dir).map_err(|e| e.to_string())?;
    let meta_path = meta_dir.join(format!("{}.json", id));

    let mut meta: serde_json::Value = if meta_path.exists() {
        let bytes = std::fs::read(&meta_path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    meta["id"] = serde_json::json!(id);
    // Only patch transcript/summary when a value is provided so `None` means "leave existing"
    // (frontend sends null for those keys on new recordings; missing keys behave the same).
    if let Some(t) = transcript {
        meta["transcript"] = serde_json::json!(t);
    }
    if let Some(s) = summary {
        meta["summary"] = serde_json::json!(s);
    }
    meta["duration_secs"] = serde_json::json!(duration_secs);
    meta["created_at"] = serde_json::json!(created_at);

    if let Some(ref p) = pipeline_status {
        meta["pipeline_status"] = serde_json::json!(p);
    }
    if let Some(ref e) = pipeline_error {
        meta["pipeline_error"] = serde_json::json!(e);
    }
    if let Some(pr) = pipeline_priority {
        meta["pipeline_priority"] = serde_json::json!(pr);
    }

    std::fs::write(
        meta_path,
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Set or clear the user-visible recording label stored in metadata (`display_name`).
/// Passing an empty string (after trim) removes the custom name so the UI falls back to the default.
#[tauri::command]
async fn set_recording_display_name(app: AppHandle, id: String, display_name: String) -> Result<(), String> {
    let meta_path = recording_meta_path(&app, &id)?;
    let mut meta: serde_json::Value = if meta_path.exists() {
        let bytes = std::fs::read(&meta_path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    meta["id"] = serde_json::json!(id);

    let s = display_name.trim().chars().take(200).collect::<String>();
    if s.is_empty() {
        if let Some(o) = meta.as_object_mut() {
            o.remove("display_name");
        }
    } else {
        meta["display_name"] = serde_json::json!(s);
    }

    std::fs::write(
        meta_path,
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn diarize_model_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("models").join(DIARIZE_MODEL_FILENAME))
}

fn recording_meta_path(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    let meta_dir = app_data_dir(app)?.join("metadata");
    Ok(meta_dir.join(format!("{id}.json")))
}

#[tauri::command]
async fn delete_recording(app: AppHandle, id: String) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let recordings_dir = data_dir.join("recordings");
    let meta_dir = data_dir.join("metadata");

    for ext in &["webm", "wav"] {
        let path = recordings_dir.join(format!("{}.{}", id, ext));
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    let meta_path = meta_dir.join(format!("{}.json", id));
    if meta_path.exists() {
        std::fs::remove_file(meta_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Debug)]
struct PipelineCoordinator {
    queue: Mutex<VecDeque<String>>,
    notify: Notify,
    exclusive: Mutex<()>,
    /// Currently running pipeline job (if any), with its cancellation handle.
    active: Mutex<Option<(String, CancellationToken)>>,
}

impl PipelineCoordinator {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            exclusive: Mutex::new(()),
            active: Mutex::new(None),
        }
    }

    /// Returns `true` if the id was newly appended (not already in the queue).
    async fn enqueue_unique_back(&self, id: String) -> bool {
        let mut q = self.queue.lock().await;
        if !q.iter().any(|x| x == &id) {
            q.push_back(id);
            true
        } else {
            false
        }
    }

    async fn promote_to_front(&self, id: &str) {
        let mut q = self.queue.lock().await;
        if let Some(pos) = q.iter().position(|x| x == id) {
            let item = q.remove(pos).unwrap();
            q.push_front(item);
        }
    }

    /// Removes matching IDs from the queue and cancels the active job when its ID matches.
    /// Returns `(removed_from_queue, active_cancelled_id)`.
    async fn cancel_matching(&self, ids: &HashSet<String>) -> (Vec<String>, Option<String>) {
        let mut dequeued: Vec<String> = Vec::new();
        {
            let mut q = self.queue.lock().await;
            q.retain(|x| {
                if ids.contains(x) {
                    dequeued.push(x.clone());
                    false
                } else {
                    true
                }
            });
        }
        let mut active_hit: Option<String> = None;
        {
            let a = self.active.lock().await;
            if let Some((ref rid, ref tok)) = *a {
                if ids.contains(rid) {
                    tok.cancel();
                    active_hit = Some(rid.clone());
                }
            }
        }
        (dequeued, active_hit)
    }

    fn wake(&self) {
        self.notify.notify_one();
    }
}

#[derive(Debug, Serialize, Clone)]
struct PipelineJobEvent {
    id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn audio_path_for_id(app: &AppHandle, id: &str) -> Result<String, String> {
    let dir = app_data_dir(app)?.join("recordings");
    for ext in ["webm", "wav"] {
        let p = dir.join(format!("{id}.{ext}"));
        if p.exists() {
            return Ok(p.to_string_lossy().to_string());
        }
    }
    Err(format!("No audio file for id {id}"))
}

async fn patch_recording_meta(app: &AppHandle, id: &str, patch: serde_json::Value) -> Result<(), String> {
    let path = recording_meta_path(app, id)?;
    let mut meta: serde_json::Value = if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({ "id": id }))
    } else {
        serde_json::json!({ "id": id })
    };
    if let Some(obj) = patch.as_object() {
        for (k, v) in obj {
            meta[k] = v.clone();
        }
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn emit_pipeline_event(app: &AppHandle, id: &str, status: &str, error: Option<String>) {
    let _ = app.emit(
        "pipeline-job-update",
        PipelineJobEvent {
            id: id.to_string(),
            status: status.to_string(),
            error,
        },
    );
}

async fn pipeline_cancelled_cleanup(app: &AppHandle, id: &str) -> Result<(), String> {
    patch_recording_meta(
        app,
        id,
        serde_json::json!({ "pipeline_status": "idle", "pipeline_error": null }),
    )
    .await?;
    emit_pipeline_event(app, id, "cancelled", None);
    Ok(())
}

fn diarization_platform_supported() -> bool {
    !(cfg!(all(target_os = "macos", target_arch = "x86_64")))
}

fn speaker_line(speaker: Option<i32>, text: &str) -> String {
    let prefix = speaker
        .map(|s| format!("[Speaker {}] ", s + 1))
        .unwrap_or_default();
    format!("{}{}", prefix, text.trim())
}

fn transcription_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("transcription_settings.json"))
}

/// Ensures the path points to a plausible on-disk Whisper GGUF / legacy .bin before persisting settings.
fn validate_sona_model_path(model_path: &str) -> Result<(), String> {
    use std::io::Read;

    let p = Path::new(model_path);
    if !p.is_file() {
        return Err(format!(
            "No file at that path. Check spelling and that the file is still there:\n{}",
            model_path
        ));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("Cannot read that file: {e}"))?;
    let len = meta.len();
    if len == 0 {
        return Err("That file is empty.".to_string());
    }

    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "gguf" | "bin" => {}
            other => {
                return Err(format!(
                    "Use a Whisper model whose name ends with .gguf or .bin (not .{other})."
                ));
            }
        }
    }

    let mut file = std::fs::File::open(p).map_err(|e| format!("Cannot open model file: {e}"))?;
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return Err(
            "Could not read the start of the file — it may be empty or permissions may block access."
                .to_string(),
        );
    }
    if magic == *b"GGUF" {
        return Ok(());
    }
    if p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
    {
        return Err(
            "This file is named .gguf but does not contain a GGUF header — it may be corrupt or not a model."
                .to_string(),
        );
    }

    const MIN_WHISPER_BYTES: u64 = 10 * 1024 * 1024;
    if len < MIN_WHISPER_BYTES {
        return Err(format!(
            "File is not a GGUF model (missing GGUF header) and is smaller than a typical Whisper .bin (expect at least {} MB).",
            MIN_WHISPER_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

async fn read_transcription_settings(app: &AppHandle) -> Result<TranscriptionSettings, String> {
    let path = transcription_settings_path(app)?;
    if !path.exists() {
        return Ok(TranscriptionSettings::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(serde_json::from_slice(&bytes).unwrap_or_else(|_| TranscriptionSettings::default()))
}

async fn resolve_sona_with_optional_download(app: &AppHandle) -> Result<PathBuf, String> {
    let dl_err = sona_download::ensure_sona_sidecars_present(app).await.err();
    match sona_paths::resolve_sona_binary(app) {
        Ok(p) => Ok(p),
        Err(e) => Err(match dl_err {
            Some(d) => format!("{d} — {e}"),
            None => e,
        }),
    }
}

#[tauri::command]
async fn check_transcription_ready(app: AppHandle) -> CheckResult {
    if let Err(e) = resolve_sona_with_optional_download(&app).await {
        return CheckResult {
            available: false,
            version: None,
            error: Some(e),
        };
    }

    let settings = match read_transcription_settings(&app).await {
        Ok(s) => s,
        Err(e) => {
            return CheckResult {
                available: false,
                version: None,
                error: Some(e),
            };
        }
    };

    let model_path = settings.sona_model_path.trim();
    if model_path.is_empty() {
        return CheckResult {
            available: false,
            version: None,
            error: Some(
                "Choose a Sona-compatible Whisper GGUF file — see guide below.".to_string(),
            ),
        };
    }

    if !Path::new(model_path).is_file() {
        return CheckResult {
            available: false,
            version: None,
            error: Some(format!("Model file not found: {model_path}")),
        };
    }

    if settings.diarize_enabled && diarization_platform_supported() {
        match app_data_dir(&app).map(|d| d.join("models").join(DIARIZE_MODEL_FILENAME)) {
            Ok(path) if path.is_file() => {}
            Ok(path) => {
                return CheckResult {
                    available: false,
                    version: None,
                    error: Some(format!(
                        "Speaker diarization is on but the ONNX model is missing. Download it or disable diarization. Expected: {}",
                        path.display()
                    )),
                };
            }
            Err(e) => {
                return CheckResult {
                    available: false,
                    version: None,
                    error: Some(e),
                };
            }
        }
    }

    CheckResult {
        available: true,
        version: Some(model_path.to_string()),
        error: None,
    }
}

#[tauri::command]
async fn get_transcription_settings(app: AppHandle) -> Result<TranscriptionSettings, String> {
    read_transcription_settings(&app).await
}

#[tauri::command]
async fn set_transcription_settings(
    app: AppHandle,
    mut settings: TranscriptionSettings,
) -> Result<(), String> {
    settings.sona_model_path = settings.sona_model_path.trim().to_string();
    if settings.sona_model_path.is_empty() {
        return Err(
            "Paste the full path to your Whisper model file (.gguf or .bin), then save.".to_string(),
        );
    }
    validate_sona_model_path(&settings.sona_model_path)?;
    let path = transcription_settings_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_models_dir(app: AppHandle) -> Result<String, String> {
    let dir = app_data_dir(&app)?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn is_diarization_platform_supported() -> bool {
    diarization_platform_supported()
}

#[derive(Debug, Serialize)]
struct DiarizeModelInfo {
    path: String,
    present: bool,
    size_bytes: Option<u64>,
}

#[tauri::command]
fn get_diarize_model_info(app: AppHandle) -> Result<DiarizeModelInfo, String> {
    let path = diarize_model_path(&app)?;
    let path_str = path.to_string_lossy().to_string();
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => Ok(DiarizeModelInfo {
            path: path_str,
            present: true,
            size_bytes: Some(meta.len()),
        }),
        _ => Ok(DiarizeModelInfo {
            path: path_str,
            present: false,
            size_bytes: None,
        }),
    }
}

/// Reject tiny downloads (error pages, broken proxies) — real asset is tens of MB.
const MIN_DIARIZE_ONNX_BYTES: u64 = 512 * 1024;

#[tauri::command]
async fn download_diarize_model(app: AppHandle) -> Result<String, String> {
    let dest = diarize_model_path(&app)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(DIARIZE_MODEL_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Download failed: HTTP {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&dest).map_err(|e| e.to_string())?;
    let len = meta.len();
    if len < MIN_DIARIZE_ONNX_BYTES {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "Downloaded file is only {len} bytes (expected a large ONNX model). Removed bad file — check network and try again."
        ));
    }
    Ok(dest.to_string_lossy().to_string())
}

async fn ensure_sona_model_loaded(
    app: &AppHandle,
    sona_state: &Mutex<SonaState>,
    model_path: &str,
    gpu_device: Option<i32>,
) -> Result<(), String> {
    resolve_sona_with_optional_download(app).await?;
    let spawn_fresh = || -> Result<sona::SonaProcess, String> {
        let binary = sona_paths::resolve_sona_binary(app)?;
        let ffmpeg = sona_paths::resolve_ffmpeg_path(app);
        let diarize = sona_paths::resolve_diarize_path(app);
        sona::SonaProcess::spawn(&binary, ffmpeg.as_deref(), diarize.as_deref())
            .map_err(|e| format!("{:#}", e))
    };

    let mut guard = sona_state.lock().await;

    let alive = guard
        .process
        .as_mut()
        .map(|p| p.is_alive())
        .unwrap_or(false);
    if !alive {
        if let Some(mut old) = guard.process.take() {
            old.kill();
        }
        guard.process = Some(spawn_fresh()?);
        guard.loaded_model_path = None;
    }

    if guard.loaded_model_path.as_deref() == Some(model_path) {
        return Ok(());
    }

    let load_result = {
        let proc = guard
            .process
            .as_mut()
            .ok_or_else(|| "sona process missing".to_string())?;
        proc.load_model(model_path, gpu_device, false).await
    };

    match load_result {
        Ok(()) => {
            guard.loaded_model_path = Some(model_path.to_string());
            guard.loaded_gpu_device = gpu_device;
            Ok(())
        }
        Err(e) => {
            tracing::warn!("sona load_model failed (retrying on CPU): {:#}", e);
            if let Some(mut old) = guard.process.take() {
                old.kill();
            }
            let mut proc = spawn_fresh()?;
            proc.load_model(model_path, gpu_device, true)
                .await
                .map_err(|e2| format!("{:#}", e2))?;
            guard.process = Some(proc);
            guard.loaded_model_path = Some(model_path.to_string());
            guard.loaded_gpu_device = gpu_device;
            Ok(())
        }
    }
}

async fn convert_to_wav(webm_path: &str, ffmpeg_bin: Option<&Path>) -> Result<String, String> {
    let stem = PathBuf::from(webm_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();
    let wav_path = std::env::temp_dir()
        .join(format!("mnt_{}_converted.wav", stem));
    let wav_str = wav_path.to_string_lossy().to_string();

    let result = if let Some(bin) = ffmpeg_bin {
        tokio::process::Command::new(bin)
            .args([
                "-y",
                "-i",
                webm_path,
                "-ar",
                "16000",
                "-ac",
                "1",
                "-c:a",
                "pcm_s16le",
                &wav_str,
            ])
            .output()
            .await
    } else {
        let mut ff = tokio::process::Command::new("ffmpeg");
        apply_user_tool_path(&mut ff);
        ff.args([
            "-y",
            "-i",
            webm_path,
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            &wav_str,
        ])
        .output()
        .await
    };

    match result {
        Ok(out) if out.status.success() => Ok(wav_str),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            Err(format!(
                "ffmpeg conversion failed: {}\n\nInstall ffmpeg or use a bundled app build with ffmpeg.",
                stderr
            ))
        }
        Err(e) => Err(format!(
            "ffmpeg not found ({}). Install ffmpeg or use a desktop build that bundles ffmpeg.",
            e
        )),
    }
}

async fn run_sona_transcription(
    app: AppHandle,
    audio_path: String,
    cancel: CancellationToken,
    sona_state: Arc<Mutex<SonaState>>,
) -> Result<String, String> {
    use futures_util::StreamExt;

    let settings = read_transcription_settings(&app).await?;
    let model_path = settings.sona_model_path.trim();
    if model_path.is_empty() {
        return Err(
            "No transcription model path set. Open Setup → Transcription and choose a Sona-compatible GGUF model."
                .to_string(),
        );
    }
    if !Path::new(model_path).is_file() {
        return Err(format!("Transcription model file not found: {model_path}"));
    }

    ensure_sona_model_loaded(&app, &sona_state, model_path, settings.gpu_device).await?;

    let bundled_ffmpeg = sona_paths::resolve_ffmpeg_path(&app);
    let ffmpeg_ref = bundled_ffmpeg.as_deref();

    let (transcribe_path, wav_tmp) = match convert_to_wav(&audio_path, ffmpeg_ref).await {
        Ok(wav) => (wav.clone(), Some(wav)),
        Err(e) => {
            eprintln!("ffmpeg conversion skipped: {}", e);
            (audio_path.clone(), None)
        }
    };

    let models_base = app_data_dir(&app)?.join("models");
    let diarize_model_path = if settings.diarize_enabled && diarization_platform_supported() {
        let p = models_base.join(DIARIZE_MODEL_FILENAME);
        if p.is_file() {
            Some(p.to_string_lossy().to_string())
        } else {
            if let Some(ref w) = wav_tmp {
                let _ = std::fs::remove_file(w);
            }
            return Err(format!(
                "Speaker diarization is enabled but the model file is missing. Download it in Setup or place it at {}",
                p.display()
            ));
        }
    } else {
        None
    };

    let opts = sona::SonaTranscribeOptions {
        path: transcribe_path.clone(),
        language: Some("en".to_string()),
        translate: false,
        prompt: None,
        diarize_model: diarize_model_path,
    };

    let (client, base_url) = {
        let mut guard = sona_state.lock().await;
        let proc = guard
            .process
            .as_mut()
            .ok_or_else(|| "sona process missing".to_string())?;
        (proc.client(), proc.base_url())
    };

    let stream = sona::SonaProcess::transcribe_stream(&client, &base_url, &opts)
        .await
        .map_err(|e| {
            if let Some(api) = e.downcast_ref::<sona::SonaApiError>() {
                format!("{}: {}", api.code, api.message)
            } else {
                format!("{:#}", e)
            }
        })?;

    tokio::pin!(stream);

    let mut lines: Vec<String> = Vec::new();

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if let Some(ref w) = wav_tmp {
                    let _ = std::fs::remove_file(w);
                }
                return Err(PIPELINE_CANCELLED.to_string());
            }
            ev = stream.next() => {
                let Some(event_result) = ev else { break };
                match event_result {
                    Ok(sona::SonaEvent::Segment { text, speaker, .. }) => {
                        let line = speaker_line(speaker, &text);
                        let _ = app.emit("transcription-chunk", &line);
                        lines.push(line);
                    }
                    Ok(sona::SonaEvent::Error { code, message }) => {
                        if let Some(ref w) = wav_tmp {
                            let _ = std::fs::remove_file(w);
                        }
                        let c = code.unwrap_or_else(|| "transcription_error".to_string());
                        return Err(format!("{c}: {message}"));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if let Some(ref w) = wav_tmp {
                            let _ = std::fs::remove_file(w);
                        }
                        return Err(format!("{:#}", e));
                    }
                }
            }
        }
    }

    if let Some(ref w) = wav_tmp {
        let _ = std::fs::remove_file(w);
    }

    Ok(lines.join("\n"))
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaResponse {
    response: String,
}

async fn run_summarize_internal(transcript: String) -> Result<String, String> {
    let client = reqwest::Client::new();
    let prompt = format!(
        "You are a helpful assistant that summarizes meeting transcripts.\n\n\
         Please provide a concise summary of the following meeting transcript. \
         Include:\n- Key topics discussed\n- Action items\n- Decisions made\n\n\
         Transcript:\n{}\n\nSummary:",
        transcript
    );

    let request = OllamaRequest {
        model: "gemma4:e4b".to_string(),
        prompt,
        stream: false,
    };

    let response = client
        .post("http://localhost:11434/api/generate")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama: {}. Make sure Ollama is running (`ollama serve`) and the model is pulled (`ollama pull gemma4:e4b`)", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Ollama returned error: {}",
            response.status()
        ));
    }

    let ollama_response: OllamaResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

    Ok(ollama_response.response.trim().to_string())
}

async fn run_summarize_internal_cancellable(
    transcript: String,
    cancel: CancellationToken,
    cancelled_msg: &'static str,
) -> Result<String, String> {
    let mut task = tokio::spawn(async move { run_summarize_internal(transcript).await });
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            task.abort();
            let _ = task.await;
            Err(cancelled_msg.to_string())
        }
        res = &mut task => match res {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(e),
            Err(join_err) => {
                if join_err.is_cancelled() {
                    Err(cancelled_msg.to_string())
                } else {
                    Err(join_err.to_string())
                }
            }
        },
    }
}

async fn run_pipeline_job_inner(
    app: &AppHandle,
    id: &str,
    cancel: CancellationToken,
) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err(PIPELINE_CANCELLED.to_string());
    }

    let sona_state = Arc::clone(&*app.state::<Arc<Mutex<SonaState>>>());

    let audio_path = audio_path_for_id(app, id)?;
    let path = recording_meta_path(app, id)?;
    let meta: serde_json::Value = if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
    } else {
        return Err(format!("Missing metadata for {id}"));
    };

    let has_t = meta["transcript"]
        .as_str()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_s = meta["summary"]
        .as_str()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    if has_t && has_s {
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "pipeline_status": "done", "pipeline_error": null }),
        )
        .await?;
        emit_pipeline_event(app, id, "done", None);
        return Ok(());
    }

    if !has_t {
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "pipeline_status": "transcribing", "pipeline_error": null }),
        )
        .await?;
        emit_pipeline_event(app, id, "transcribing", None);
        let transcript =
            run_sona_transcription(app.clone(), audio_path, cancel.clone(), sona_state).await?;
        if cancel.is_cancelled() {
            return Err(PIPELINE_CANCELLED.to_string());
        }
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "transcript": transcript, "pipeline_status": "summarizing" }),
        )
        .await?;
        emit_pipeline_event(app, id, "summarizing", None);
    }

    if cancel.is_cancelled() {
        return Err(PIPELINE_CANCELLED.to_string());
    }

    let meta2: serde_json::Value = {
        let bytes = std::fs::read(recording_meta_path(app, id)?).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
    };
    let transcript = meta2["transcript"]
        .as_str()
        .ok_or_else(|| "Transcript missing after transcription step".to_string())?
        .to_string();

    let need_summary = !meta2["summary"]
        .as_str()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if need_summary {
        // Transcribe-first jobs already move to summarizing after Sona; summarize-only reruns
        // (e.g. after cancel) stay on "queued" until here — patch so UI and queue snapshot match work.
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "pipeline_status": "summarizing", "pipeline_error": null }),
        )
        .await?;
        emit_pipeline_event(app, id, "summarizing", None);
        let summary =
            run_summarize_internal_cancellable(transcript, cancel.clone(), PIPELINE_CANCELLED).await?;
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "summary": summary, "pipeline_status": "done", "pipeline_error": null }),
        )
        .await?;
    } else {
        patch_recording_meta(
            app,
            id,
            serde_json::json!({ "pipeline_status": "done", "pipeline_error": null }),
        )
        .await?;
    }
    emit_pipeline_event(app, id, "done", None);
    Ok(())
}

async fn run_pipeline_job(
    app: &AppHandle,
    id: &str,
    cancel: CancellationToken,
) -> Result<(), String> {
    let result = run_pipeline_job_inner(app, id, cancel).await;
    match result {
        Ok(()) => Ok(()),
        Err(e) if e == PIPELINE_CANCELLED => pipeline_cancelled_cleanup(app, id).await,
        Err(e) => {
            let _ = patch_recording_meta(
                app,
                id,
                serde_json::json!({ "pipeline_status": "failed", "pipeline_error": e }),
            )
            .await;
            emit_pipeline_event(app, id, "failed", Some(e.clone()));
            Err(e)
        }
    }
}

async fn pipeline_worker_loop(app: AppHandle, coord: Arc<PipelineCoordinator>) {
    loop {
        let id = {
            let mut q = coord.queue.lock().await;
            q.pop_front()
        };
        if let Some(id) = id {
            let cancel = CancellationToken::new();
            {
                let mut a = coord.active.lock().await;
                *a = Some((id.clone(), cancel.clone()));
            }
            let _guard = coord.exclusive.lock().await;
            let _ = run_pipeline_job(&app, &id, cancel).await;
            drop(_guard);
            {
                let mut a = coord.active.lock().await;
                if let Some((ref rid, _)) = *a {
                    if rid == &id {
                        *a = None;
                    }
                }
            }
            let _ = app.emit(
                "pipeline-job-update",
                PipelineJobEvent {
                    id: id.clone(),
                    status: "updated".into(),
                    error: None,
                },
            );
        } else {
            coord.notify.notified().await;
        }
    }
}

async fn recover_pipeline_queue(app: AppHandle, coord: Arc<PipelineCoordinator>) {
    let Ok(list) = list_recordings_impl(&app).await else {
        return;
    };
    for rec in list {
        let has_t = rec
            .transcript
            .as_ref()
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        let has_s = rec
            .summary
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_t && has_s {
            continue;
        }
        let st = rec.pipeline_status.as_deref().unwrap_or("");
        if st == "failed" || st == "idle" {
            continue;
        }
        if st == "transcribing" || st == "summarizing" {
            let _ = patch_recording_meta(
                &app,
                &rec.id,
                serde_json::json!({ "pipeline_status": "queued" }),
            )
            .await;
        }
        let _ = coord.enqueue_unique_back(rec.id.clone()).await;
    }
    coord.wake();
}

#[tauri::command]
async fn enqueue_pipeline_jobs(
    app: AppHandle,
    args: EnqueuePipelineJobsArgs,
) -> Result<EnqueuePipelineOutcome, String> {
    let EnqueuePipelineJobsArgs {
        ids,
        overwrite_complete,
    } = args;
    let coord = app.state::<Arc<PipelineCoordinator>>();
    let list = list_recordings_impl(&app).await?;
    let mut enqueued: u32 = 0;
    let mut skipped_complete: u32 = 0;
    let mut needs_wake = false;

    for id in ids {
        let rec = list
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("Recording {id} not found"))?;
        let has_t = rec
            .transcript
            .as_ref()
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        let has_s = rec
            .summary
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if has_t && has_s && !overwrite_complete {
            skipped_complete += 1;
            continue;
        }
        let patch = if has_t && has_s && overwrite_complete {
            serde_json::json!({
                "transcript": null,
                "summary": null,
                "pipeline_status": "queued",
                "pipeline_error": null,
            })
        } else {
            serde_json::json!({ "pipeline_status": "queued", "pipeline_error": null })
        };
        patch_recording_meta(&app, &id, patch).await?;
        if coord.enqueue_unique_back(id).await {
            enqueued += 1;
            needs_wake = true;
        }
    }
    if needs_wake {
        coord.wake();
    }
    Ok(EnqueuePipelineOutcome {
        enqueued,
        skipped_complete,
    })
}

#[tauri::command]
async fn cancel_pipeline_jobs(app: AppHandle, ids: Vec<String>) -> Result<Vec<String>, String> {
    let coord = app.state::<Arc<PipelineCoordinator>>();
    let id_set: HashSet<String> = ids.into_iter().collect();

    let (dequeued, active_cancelled) = coord.cancel_matching(&id_set).await;

    let mut affected: HashSet<String> = dequeued.iter().cloned().collect();
    if let Some(ref id) = active_cancelled {
        affected.insert(id.clone());
    }

    for id in &dequeued {
        patch_recording_meta(
            &app,
            id,
            serde_json::json!({ "pipeline_status": "idle", "pipeline_error": null }),
        )
        .await?;
        emit_pipeline_event(&app, id, "cancelled", None);
    }

    Ok(affected.into_iter().collect())
}

#[tauri::command]
async fn set_pipeline_job_priority(app: AppHandle, id: String, priority: bool) -> Result<(), String> {
    let coord = app.state::<Arc<PipelineCoordinator>>();
    patch_recording_meta(
        &app,
        &id,
        serde_json::json!({ "pipeline_priority": priority }),
    )
    .await?;
    if priority {
        coord.promote_to_front(&id).await;
        coord.wake();
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[tauri::command]
async fn check_sona(app: AppHandle) -> CheckResult {
    match resolve_sona_with_optional_download(&app).await {
        Ok(path) => CheckResult {
            available: true,
            version: Some(path.to_string_lossy().to_string()),
            error: None,
        },
        Err(e) => CheckResult {
            available: false,
            version: None,
            error: Some(e),
        },
    }
}

#[tauri::command]
async fn check_ffmpeg(app: AppHandle) -> CheckResult {
    let _ = sona_download::ensure_sona_sidecars_present(&app).await;
    if let Some(bin) = sona_paths::resolve_ffmpeg_path(&app) {
        match tokio::process::Command::new(&bin).arg("-version").output().await {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(|l| l.replace("ffmpeg version ", ""))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                return CheckResult {
                    available: true,
                    version: Some(version),
                    error: None,
                };
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return CheckResult {
                    available: false,
                    version: None,
                    error: Some(stderr),
                };
            }
            Err(e) => {
                return CheckResult {
                    available: false,
                    version: None,
                    error: Some(e.to_string()),
                };
            }
        }
    }

    let mut c = tokio::process::Command::new("ffmpeg");
    apply_user_tool_path(&mut c);
    match c.arg("-version").output().await {
        Ok(out) => {
            let version = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|l| l.replace("ffmpeg version ", ""))
                .unwrap_or_default()
                .trim()
                .to_string();
            CheckResult {
                available: true,
                version: Some(version),
                error: None,
            }
        }
        Err(e) => CheckResult {
            available: false,
            version: None,
            error: Some(e.to_string()),
        },
    }
}

#[tauri::command]
async fn check_ollama() -> CheckResult {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    match client.get("http://localhost:11434/api/version").send().await {
        Ok(resp) if resp.status().is_success() => {
            let version = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v["version"].as_str().map(String::from));
            CheckResult { available: true, version, error: None }
        }
        Ok(resp) => CheckResult {
            available: false,
            version: None,
            error: Some(format!("Unexpected status: {}", resp.status())),
        },
        Err(e) => CheckResult { available: false, version: None, error: Some(e.to_string()) },
    }
}

#[tauri::command]
async fn open_audio_midi_setup() -> Result<(), String> {
    tokio::process::Command::new("open")
        .args(["-a", "Audio MIDI Setup"])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn check_blackhole() -> CheckResult {
    // Fast path: check for the installed HAL driver bundle
    if std::path::Path::new("/Library/Audio/Plug-Ins/HAL/BlackHole2ch.driver").exists() {
        return CheckResult { available: true, version: Some("2ch".to_string()), error: None };
    }

    // Fallback: scan system_profiler audio output
    let output = tokio::process::Command::new("system_profiler")
        .arg("SPAudioDataType")
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
            if text.contains("blackhole 2ch") {
                CheckResult { available: true, version: Some("2ch".to_string()), error: None }
            } else {
                CheckResult {
                    available: false,
                    version: None,
                    error: Some("BlackHole 2ch audio driver not found".to_string()),
                }
            }
        }
        _ => CheckResult {
            available: false,
            version: None,
            error: Some("BlackHole 2ch audio driver not found".to_string()),
        },
    }
}

#[tauri::command]
async fn check_ollama_model(model: String) -> CheckResult {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    match client
        .post("http://localhost:11434/api/show")
        .json(&serde_json::json!({ "name": model }))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let details = resp.json::<serde_json::Value>().await.ok();
            let version = details
                .as_ref()
                .and_then(|v| v["details"]["parameter_size"].as_str())
                .map(String::from);
            CheckResult { available: true, version, error: None }
        }
        Ok(_) => CheckResult {
            available: false,
            version: None,
            error: Some(format!("Model '{}' not found", model)),
        },
        Err(e) => CheckResult { available: false, version: None, error: Some(e.to_string()) },
    }
}

// ── macOS automatic audio routing setup ─────────────────────────────────────

#[cfg(target_os = "macos")]
mod audio_setup {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_void};
    use std::path::Path;
    use std::ptr;

    const MULTI_OUT_UID: &str = "com.dory.multioutput";
    const AGGREGATE_UID: &str = "com.dory.aggregate";

    type AudioObjectID = u32;
    type AudioDeviceID = AudioObjectID;
    type OSStatus = i32;
    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CFIndex = i64;
    type CFStringEncoding = u32;

    const UTF8: CFStringEncoding = 0x0800_0100;
    const CF_NUM_SINT32: i64 = 3;
    const NULL_ALLOC: CFAllocatorRef = ptr::null();
    const SYSTEM_OBJECT: AudioObjectID = 1;
    const ELEMENT_MAIN: u32 = 0;

    fn cc(b: &[u8; 4]) -> u32 { u32::from_be_bytes(*b) }
    fn scope_global()          -> u32 { cc(b"glob") }
    fn scope_output()          -> u32 { cc(b"outp") }
    fn sel_devices()           -> u32 { cc(b"dev#") }
    fn sel_default_input()     -> u32 { cc(b"dIn ") }
    fn sel_default_output()    -> u32 { cc(b"dOut") }
    fn sel_uid()               -> u32 { cc(b"uid ") }
    fn sel_name()              -> u32 { cc(b"lnam") }
    fn sel_volume()            -> u32 { cc(b"volm") } // kAudioDevicePropertyVolumeScalar
    /// `kAudioAggregateDevicePropertyFullSubDeviceList` — sub-device UIDs (Multi-Output & aggregates).
    fn sel_aggregate_full_subdevices() -> u32 { cc(b"grnd") }

    #[repr(C)]
    struct Addr { sel: u32, scope: u32, elem: u32 }
    impl Addr {
        fn global(sel: u32) -> Self { Self { sel, scope: scope_global(), elem: ELEMENT_MAIN } }
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyDataSize(id: AudioObjectID, addr: *const Addr, qs: u32, qd: *const c_void, out: *mut u32) -> OSStatus;
        fn AudioObjectGetPropertyData(id: AudioObjectID, addr: *const Addr, qs: u32, qd: *const c_void, sz: *mut u32, out: *mut c_void) -> OSStatus;
        fn AudioObjectSetPropertyData(id: AudioObjectID, addr: *const Addr, qs: u32, qd: *const c_void, sz: u32, data: *const c_void) -> OSStatus;
        fn AudioHardwareCreateAggregateDevice(desc: CFDictionaryRef, out: *mut AudioDeviceID) -> OSStatus;
        fn AudioHardwareDestroyAggregateDevice(device_id: AudioDeviceID) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(a: CFAllocatorRef, s: *const c_char, enc: CFStringEncoding) -> CFStringRef;
        fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, len: i64, enc: CFStringEncoding) -> bool;
        fn CFNumberCreate(a: CFAllocatorRef, ty: i64, val: *const c_void) -> CFNumberRef;
        fn CFArrayCreate(a: CFAllocatorRef, vals: *const CFTypeRef, n: CFIndex, cbs: *const c_void) -> CFArrayRef;
        fn CFDictionaryCreate(a: CFAllocatorRef, keys: *const CFTypeRef, vals: *const CFTypeRef, n: CFIndex, kcbs: *const c_void, vcbs: *const c_void) -> CFDictionaryRef;
        fn CFRelease(r: CFTypeRef);
        fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        static kCFTypeArrayCallBacks: c_void;
    }

    unsafe fn cf_str(s: &str) -> CFStringRef {
        let cs = CString::new(s).unwrap();
        CFStringCreateWithCString(NULL_ALLOC, cs.as_ptr(), UTF8)
    }
    unsafe fn cf_num(n: i32) -> CFNumberRef {
        CFNumberCreate(NULL_ALLOC, CF_NUM_SINT32, &n as *const i32 as *const c_void)
    }
    unsafe fn cf_to_string(r: CFStringRef) -> Option<String> {
        if r.is_null() { return None; }
        let mut buf = vec![0u8; 512];
        if CFStringGetCString(r, buf.as_mut_ptr() as *mut c_char, buf.len() as i64, UTF8) {
            Some(CStr::from_ptr(buf.as_ptr() as *const c_char).to_string_lossy().into_owned())
        } else {
            None
        }
    }

    /// Copy a `CFString` that we do **not** own (e.g. values inside a `CFArray`).
    unsafe fn cf_string_to_string_unowned(r: CFStringRef) -> Option<String> {
        if r.is_null() {
            return None;
        }
        let mut buf = vec![0u8; 1024];
        if CFStringGetCString(r, buf.as_mut_ptr() as *mut c_char, buf.len() as i64, UTF8) {
            Some(CStr::from_ptr(buf.as_ptr() as *const c_char).to_string_lossy().into_owned())
        } else {
            None
        }
    }

    fn list_devices() -> Vec<AudioDeviceID> {
        let addr = Addr::global(sel_devices());
        let mut sz = 0u32;
        unsafe {
            AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, ptr::null(), &mut sz);
            let n = sz as usize / std::mem::size_of::<AudioDeviceID>();
            let mut devs = vec![0u32; n];
            AudioObjectGetPropertyData(SYSTEM_OBJECT, &addr, 0, ptr::null(), &mut sz, devs.as_mut_ptr() as *mut c_void);
            devs
        }
    }

    fn get_str_prop(id: AudioDeviceID, sel: u32) -> Option<String> {
        let addr = Addr::global(sel);
        let mut sz = std::mem::size_of::<CFStringRef>() as u32;
        let mut r: CFStringRef = ptr::null();
        let ok = unsafe {
            AudioObjectGetPropertyData(id, &addr, 0, ptr::null(), &mut sz, &mut r as *mut CFStringRef as *mut c_void) == 0
        };
        if !ok || r.is_null() { return None; }
        let s = unsafe { cf_to_string(r) };
        unsafe { CFRelease(r) };
        s
    }

    fn dev_uid(id: AudioDeviceID)  -> Option<String> { get_str_prop(id, sel_uid()) }
    fn dev_name(id: AudioDeviceID) -> Option<String> { get_str_prop(id, sel_name()) }

    fn get_device_output_volume(id: AudioDeviceID) -> Option<f32> {
        let addr = Addr { sel: sel_volume(), scope: scope_output(), elem: ELEMENT_MAIN };
        let mut sz = std::mem::size_of::<f32>() as u32;
        let mut val: f32 = 0.0;
        let ok = unsafe {
            AudioObjectGetPropertyData(id, &addr, 0, ptr::null(), &mut sz,
                &mut val as *mut f32 as *mut c_void) == 0
        };
        if ok { Some(val) } else { None }
    }

    fn set_device_output_volume(id: AudioDeviceID, scalar: f32) {
        let addr = Addr { sel: sel_volume(), scope: scope_output(), elem: ELEMENT_MAIN };
        let val = scalar.clamp(0.0, 1.0);
        unsafe {
            AudioObjectSetPropertyData(id, &addr, 0, ptr::null(),
                std::mem::size_of::<f32>() as u32, &val as *const f32 as *const c_void);
        }
    }

    fn default_device(sel: u32) -> Option<AudioDeviceID> {
        let addr = Addr::global(sel);
        let mut sz = std::mem::size_of::<AudioDeviceID>() as u32;
        let mut id: AudioDeviceID = 0;
        let ok = unsafe {
            AudioObjectGetPropertyData(SYSTEM_OBJECT, &addr, 0, ptr::null(), &mut sz, &mut id as *mut AudioDeviceID as *mut c_void) == 0
        };
        if ok && id != 0 { Some(id) } else { None }
    }

    fn set_default_output(id: AudioDeviceID) {
        let addr = Addr::global(sel_default_output());
        unsafe {
            AudioObjectSetPropertyData(SYSTEM_OBJECT, &addr, 0, ptr::null(), std::mem::size_of::<AudioDeviceID>() as u32, &id as *const AudioDeviceID as *const c_void);
        }
    }

    fn set_default_input(id: AudioDeviceID) {
        let addr = Addr::global(sel_default_input());
        unsafe {
            AudioObjectSetPropertyData(SYSTEM_OBJECT, &addr, 0, ptr::null(), std::mem::size_of::<AudioDeviceID>() as u32, &id as *const AudioDeviceID as *const c_void);
        }
    }

    /// Sets the system default **input** to our Aggregate Device so System Settings matches the app.
    pub fn sync_default_input_to_aggregate() -> Result<(), String> {
        let id = find_by_uid(AGGREGATE_UID)
            .ok_or("Dory Input not found. Finish audio setup on the setup screen first.")?;
        set_default_input(id);
        Ok(())
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OutputRestoreState {
        output_device_uid: String,
    }

    fn persist_output_restore(path: &Path, uid: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let payload = OutputRestoreState {
            output_device_uid: uid.to_string(),
        };
        std::fs::write(path, serde_json::to_string(&payload).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }

    fn load_output_restore(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok().and_then(|s| {
            serde_json::from_str::<OutputRestoreState>(&s)
                .ok()
                .map(|p| p.output_device_uid)
        })
    }

    /// If output is still our Multi-Output device, switch back to the last real speakers / headphones.
    pub fn restore_default_output_if_needed(path: &Path) {
        let Some(saved) = load_output_restore(path) else {
            return;
        };
        let Some(current_id) = default_device(sel_default_output()) else {
            return;
        };
        let Some(current_uid) = dev_uid(current_id) else {
            return;
        };
        if current_uid != MULTI_OUT_UID {
            return;
        }
        if let Some(target_id) = find_by_uid(&saved) {
            set_default_output(target_id);
        }
    }

    fn find_blackhole() -> Option<(AudioDeviceID, String)> {
        // Prefer 2ch variant
        for id in list_devices() {
            let uid  = dev_uid(id) .unwrap_or_default();
            let name = dev_name(id).unwrap_or_default();
            let ulo = uid.to_lowercase(); let nlo = name.to_lowercase();
            if (ulo.contains("blackhole") || nlo.contains("blackhole"))
                && (ulo.contains("2ch") || nlo.contains("2ch") || nlo.contains("2 ch"))
            {
                return Some((id, uid));
            }
        }
        // Any BlackHole as fallback
        for id in list_devices() {
            let uid  = dev_uid(id) .unwrap_or_default();
            let name = dev_name(id).unwrap_or_default();
            if uid.to_lowercase().contains("blackhole") || name.to_lowercase().contains("blackhole") {
                return Some((id, uid));
            }
        }
        None
    }

    fn find_by_uid(uid: &str) -> Option<AudioDeviceID> {
        list_devices().into_iter().find(|&id| dev_uid(id).as_deref() == Some(uid))
    }

    fn aggregate_full_subdevice_uids(device_id: AudioDeviceID) -> Option<Vec<String>> {
        let addr = Addr::global(sel_aggregate_full_subdevices());
        let mut arr: CFArrayRef = ptr::null();
        let mut data_sz = std::mem::size_of::<CFArrayRef>() as u32;
        let ok = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                &addr,
                0,
                ptr::null(),
                &mut data_sz,
                &mut arr as *mut CFArrayRef as *mut c_void,
            ) == 0
        };
        if !ok || arr.is_null() {
            return None;
        }
        let count = unsafe { CFArrayGetCount(arr) };
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let v = unsafe { CFArrayGetValueAtIndex(arr, i) } as CFStringRef;
            if let Some(s) = unsafe { cf_string_to_string_unowned(v) } {
                out.push(s);
            }
        }
        unsafe { CFRelease(arr as CFTypeRef) };
        Some(out)
    }

    fn two_subdevices_match(device_id: AudioDeviceID, a: &str, b: &str) -> bool {
        let Some(uids) = aggregate_full_subdevice_uids(device_id) else {
            return false;
        };
        if uids.len() != 2 {
            return false;
        }
        uids.iter().any(|u| u == a) && uids.iter().any(|u| u == b)
    }

    /// Default mic/input UID — when the default is our aggregate, use the real mic leg (not BlackHole).
    fn resolve_physical_mic_uid(bh_uid: &str) -> Result<String, String> {
        let in_id = default_device(sel_default_input()).ok_or("No default input device")?;
        let uid = dev_uid(in_id).ok_or("No input device UID")?;
        if uid == AGGREGATE_UID {
            let subs = aggregate_full_subdevice_uids(in_id).ok_or(
                "macOS did not list sub-devices for Dory Input (Core Audio). \
Try this order: (1) System Settings → Sound → Input → pick your real mic (e.g. Built-in or AirPods), not this aggregate — then Setup → Re-check. \
(2) Quit other apps using the mic and Re-check. \
(3) In Audio MIDI Setup, delete Dory Input/Output if duplicated, then Re-check. \
(4) Terminal: sudo killall coreaudiod — then Re-check.",
            )?;
            subs.into_iter()
                .find(|u| u != bh_uid)
                .ok_or_else(|| "Dory Input has no microphone subdevice".to_string())
        } else {
            Ok(uid)
        }
    }

    fn destroy_virtual_device(id: AudioDeviceID) -> Result<(), String> {
        let st = unsafe { AudioHardwareDestroyAggregateDevice(id) };
        if st == 0 {
            Ok(())
        } else {
            Err(format!(
                "Could not remove stale audio device (OSStatus {st}). Quit other apps using audio, then Re-check."
            ))
        }
    }

    /// Subdevice dict for `AudioHardwareCreateAggregateDevice`.
    /// Multi-Output must enable drift compensation on non-master outputs or playback to the
    /// physical device can be silent while BlackHole still receives signal.
    unsafe fn subdevice_entry(uid_str: &str, drift_compensation: bool) -> CFDictionaryRef {
        let k_uid = cf_str("uid");
        let v_uid = cf_str(uid_str);
        if drift_compensation {
            let k_drift = cf_str("drift compensation");
            let v_drift = cf_num(1);
            let keys: [CFTypeRef; 2] = [k_uid as CFTypeRef, k_drift as CFTypeRef];
            let vals: [CFTypeRef; 2] = [v_uid as CFTypeRef, v_drift as CFTypeRef];
            let d = CFDictionaryCreate(
                NULL_ALLOC,
                keys.as_ptr(),
                vals.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(k_uid);
            CFRelease(k_drift);
            CFRelease(v_uid);
            CFRelease(v_drift);
            d
        } else {
            let keys: [CFTypeRef; 1] = [k_uid as CFTypeRef];
            let vals: [CFTypeRef; 1] = [v_uid as CFTypeRef];
            let d = CFDictionaryCreate(
                NULL_ALLOC,
                keys.as_ptr(),
                vals.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(k_uid);
            CFRelease(v_uid);
            d
        }
    }

    // Creates an Aggregate Device (stacked=false) or Multi-Output Device (stacked=true).
    unsafe fn create_device(name: &str, uid: &str, main_uid: &str, sub_uids: &[&str], stacked: bool) -> Option<AudioDeviceID> {
        // Sub-device dicts: [{uid: <subdevice_uid>}, optional drift compensation for Multi-Output]
        let sub_dicts: Vec<CFDictionaryRef> = sub_uids
            .iter()
            .map(|&u| {
                let need_drift = stacked && u != main_uid;
                subdevice_entry(u, need_drift)
            })
            .collect();

        let items: Vec<CFTypeRef> = sub_dicts.iter().map(|d| *d as CFTypeRef).collect();
        let arr = CFArrayCreate(NULL_ALLOC, items.as_ptr(), items.len() as CFIndex, &kCFTypeArrayCallBacks);
        for d in &sub_dicts { CFRelease(*d); }

        // Top-level description dict
        let kn = cf_str("name"); let ku = cf_str("uid"); let ks2 = cf_str("subdevices");
        let km = cf_str("master"); let kp = cf_str("private"); let kt = cf_str("stacked");
        let vn = cf_str(name); let vu = cf_str(uid); let vm = cf_str(main_uid);
        let vp = cf_num(0); let vt = cf_num(if stacked { 1 } else { 0 });

        let keys: [CFTypeRef; 6] = [kn as _, ku as _, ks2 as _, km as _, kp as _, kt as _];
        let vals: [CFTypeRef; 6] = [vn as _, vu as _, arr as _, vm as _, vp as _, vt as _];

        let desc = CFDictionaryCreate(NULL_ALLOC, keys.as_ptr(), vals.as_ptr(), 6, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        // CFDictionaryCreate retains keys+vals; release our refs
        for k in &keys { CFRelease(*k); }
        CFRelease(vn); CFRelease(vu); CFRelease(arr); CFRelease(vm); CFRelease(vp); CFRelease(vt);

        let mut new_id: AudioDeviceID = 0;
        let status = AudioHardwareCreateAggregateDevice(desc, &mut new_id);
        CFRelease(desc);

        if status == 0 && new_id != 0 { Some(new_id) } else { None }
    }

    #[derive(Debug, serde::Serialize)]
    pub struct SetupResult {
        pub aggregate_device_name: String,
        pub previous_output_uid: String,
    }

    pub fn check_routing() -> bool {
        if find_by_uid(AGGREGATE_UID).is_some() {
            // Both virtual devices exist → routing is ready; no need for the output to already
            // be switched to Dory Output (that only happens when recording starts).
            if find_by_uid(MULTI_OUT_UID).is_some() {
                return true;
            }
            return true;
        }
        // Also accept any aggregate or virtual device that contains BlackHole by name
        find_blackhole().map_or(false, |(_, bh_uid)| {
            list_devices().iter().any(|&id| {
                let name = dev_name(id).unwrap_or_default().to_lowercase();
                let uid = dev_uid(id).unwrap_or_default().to_lowercase();
                (name.contains("aggregate") || uid.contains("aggregate"))
                    && (name.contains("blackhole") || uid.contains(&bh_uid.to_lowercase()))
            })
        })
    }

    pub fn run(restore_path: Option<&Path>) -> Result<SetupResult, String> {
        let driver_on_disk = std::path::Path::new("/Library/Audio/Plug-Ins/HAL/BlackHole2ch.driver").exists();
        let (_bh_id, bh_uid) = find_blackhole().ok_or_else(|| {
            if driver_on_disk {
                "BlackHole 2ch driver is installed but macOS is not listing it yet (Core Audio). \
Try opening Audio MIDI Setup, restart your Mac, or run: sudo killall coreaudiod"
                    .to_string()
            } else {
                "BlackHole 2ch not found. Is it installed?".to_string()
            }
        })?;

        let out_id = default_device(sel_default_output()).ok_or("No default output device")?;
        let out_uid = dev_uid(out_id).ok_or("No output device UID")?;

        let physical_output_uid = if out_uid == MULTI_OUT_UID {
            restore_path
                .and_then(load_output_restore)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| {
                    "Output is still set to “Dory Output”, but the app has no saved speaker to restore. \
Pick your speakers or headphones in **System Settings → Sound → Output** (or Audio MIDI Setup), then tap **Re-check**."
                        .to_string()
                })?
        } else {
            if let Some(p) = restore_path {
                let _ = persist_output_restore(p, &out_uid);
            }
            out_uid.clone()
        };

        let mic_uid = resolve_physical_mic_uid(&bh_uid)?;

        let need_new_multi = match find_by_uid(MULTI_OUT_UID) {
            None => true,
            Some(mid) => !two_subdevices_match(mid, &physical_output_uid, &bh_uid),
        };

        let need_new_agg = match find_by_uid(AGGREGATE_UID) {
            None => true,
            Some(aid) => !two_subdevices_match(aid, &mic_uid, &bh_uid),
        };

        if need_new_multi {
            if let Some(mid) = find_by_uid(MULTI_OUT_UID) {
                if default_device(sel_default_output()) == Some(mid) {
                    let phys_id = find_by_uid(&physical_output_uid).ok_or_else(|| {
                        format!(
                            "Could not find output device “{physical_output_uid}”. \
In System Settings → Sound → Output, pick your speakers or AirPods, then Re-check."
                        )
                    })?;
                    set_default_output(phys_id);
                }
                destroy_virtual_device(mid)?;
            }
            let new_mid = unsafe {
                create_device(
                    "Dory Output",
                    MULTI_OUT_UID,
                    &physical_output_uid,
                    &[&physical_output_uid, &bh_uid],
                    true,
                )
            }
            .ok_or_else(|| {
                "Failed to create Multi-Output Device (Dory Output). \
Bluetooth headphones sometimes need to be paired as the system output first — pick them in Sound settings, then Re-check."
                    .to_string()
            })?;
            if new_mid == 0 {
                return Err("Failed to create Multi-Output Device".to_string());
            }
        }

        if need_new_agg {
            if let Some(aid) = find_by_uid(AGGREGATE_UID) {
                if default_device(sel_default_input()) == Some(aid) {
                    let mic_dev = find_by_uid(&mic_uid).ok_or_else(|| {
                        format!(
                            "Could not find microphone “{mic_uid}”. \
In System Settings → Sound → Input, pick your mic, then Re-check."
                        )
                    })?;
                    set_default_input(mic_dev);
                }
                destroy_virtual_device(aid)?;
            }
            let new_agg = unsafe {
                create_device(
                    "Dory Input",
                    AGGREGATE_UID,
                    &mic_uid,
                    &[&mic_uid, &bh_uid],
                    false,
                )
            }
            .ok_or_else(|| {
                "Failed to create Dory Input (mic + BlackHole). \
If you use Bluetooth-only headphones, try switching the default **input** to the Mac microphone for setup, then Re-check."
                    .to_string()
            })?;
            if new_agg == 0 {
                return Err("Failed to create Aggregate Device".to_string());
            }
        }

        let agg_id = find_by_uid(AGGREGATE_UID).ok_or("Dory Input not found after setup")?;

        Ok(SetupResult {
            aggregate_device_name: dev_name(agg_id).unwrap_or_else(|| "Dory Input".to_string()),
            previous_output_uid: physical_output_uid,
        })
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct RecordingSessionRestore {
        output_device_uid: String,
        input_device_uid: String,
        #[serde(default)]
        output_volume_scalar: Option<f32>,
    }

    /// Save current Sound output/input, switch to Dory Output + Dory Input.
    /// Idempotent if already on those devices (no session file).
    pub fn begin_recording_audio_routing(output_restore_path: &Path, session_path: &Path) -> Result<(), String> {
        let multi_id = find_by_uid(MULTI_OUT_UID).ok_or_else(|| {
            "Dory Output not found. Open Setup → Re-check (Audio routing), then try again.".to_string()
        })?;
        let agg_id = find_by_uid(AGGREGATE_UID).ok_or_else(|| {
            "Dory Input not found. Open Setup → Re-check (Audio routing), then try again.".to_string()
        })?;

        let cur_out_id = default_device(sel_default_output()).ok_or("No default output device")?;
        let cur_out_uid = dev_uid(cur_out_id).ok_or("No output device UID")?;
        let cur_in_id = default_device(sel_default_input()).ok_or("No default input device")?;
        let cur_in_uid = dev_uid(cur_in_id).ok_or("No input device UID")?;

        let out_ok = cur_out_uid == MULTI_OUT_UID;
        let in_ok = cur_in_uid == AGGREGATE_UID;

        if out_ok && in_ok {
            return Ok(());
        }

        // Read the physical output device's CoreAudio volume scalar before we switch away from it.
        let output_volume_scalar = get_device_output_volume(cur_out_id);

        if let Some(parent) = session_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let payload = RecordingSessionRestore {
            output_device_uid: cur_out_uid.clone(),
            input_device_uid: cur_in_uid.clone(),
            output_volume_scalar,
        };
        std::fs::write(
            session_path,
            serde_json::to_string(&payload).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        if !out_ok {
            let _ = persist_output_restore(output_restore_path, &cur_out_uid);
            set_default_output(multi_id);
            // Re-apply the physical device's volume via CoreAudio so it stays correct inside
            // the Multi-Output aggregate (bypasses the greyed-out macOS menu-bar control).
            if let Some(vol) = output_volume_scalar {
                set_device_output_volume(cur_out_id, vol);
            }
        }
        if !in_ok {
            set_default_input(agg_id);
        }
        Ok(())
    }

    /// Restore Sound output/input saved at recording start. Safe if no session file.
    ///
    /// When [`begin_recording_audio_routing`] skips writing a session (already on Multi-Output +
    /// aggregate), there is no JSON snapshot — fall back to [`restore_default_output_if_needed`]
    /// using `audio_output_restore.json` from setup so real speakers/headphones return.
    pub fn end_recording_audio_routing(session_path: &Path) {
        let output_restore_path = session_path.parent().map(|p| p.join("audio_output_restore.json"));

        match std::fs::read_to_string(session_path) {
            Ok(s) => match serde_json::from_str::<RecordingSessionRestore>(&s) {
                Ok(payload) => {
                    if let Some(id) = find_by_uid(&payload.output_device_uid) {
                        set_default_output(id);
                        // Restore the saved CoreAudio volume on the physical device.
                        if let Some(vol) = payload.output_volume_scalar {
                            set_device_output_volume(id, vol);
                        }
                    }
                    if let Some(id) = find_by_uid(&payload.input_device_uid) {
                        set_default_input(id);
                    }
                    let _ = std::fs::remove_file(session_path);
                }
                Err(_) => {
                    let _ = std::fs::remove_file(session_path);
                }
            },
            Err(_) => {}
        }

        if let Some(ref p) = output_restore_path {
            restore_default_output_if_needed(p);
        }
    }
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn check_audio_routing() -> CheckResult {
    let available = tokio::task::spawn_blocking(audio_setup::check_routing)
        .await
        .unwrap_or(false);
    if available {
        CheckResult { available: true, version: Some("configured".to_string()), error: None }
    } else {
        CheckResult {
            available: false,
            version: None,
            error: Some("Audio routing not configured — system audio won't be captured".to_string()),
        }
    }
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn setup_audio_capture(app: tauri::AppHandle) -> Result<audio_setup::SetupResult, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("audio_output_restore.json");
    tokio::task::spawn_blocking(move || audio_setup::run(Some(path.as_path())))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn sync_default_input_to_aggregate() -> Result<(), String> {
    tokio::task::spawn_blocking(audio_setup::sync_default_input_to_aggregate)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn begin_recording_audio_routing(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let output_restore = dir.join("audio_output_restore.json");
    let session = dir.join("audio_recording_session.json");
    tokio::task::spawn_blocking(move || {
        audio_setup::begin_recording_audio_routing(output_restore.as_path(), session.as_path())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn end_recording_audio_routing(app: tauri::AppHandle) -> Result<(), String> {
    let session = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("audio_recording_session.json");
    tokio::task::spawn_blocking(move || audio_setup::end_recording_audio_routing(session.as_path()))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn begin_recording_audio_routing() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn end_recording_audio_routing() -> Result<(), String> {
    Ok(())
}

/// macOS menu‑bar volume is often disabled when the default output is a Multi‑Output device.
/// AppleScript still updates the system output volume in many setups.
#[cfg(target_os = "macos")]
fn macos_output_volume_applescript_get() -> Result<u8, String> {
    let out = std::process::Command::new("osascript")
        .args(["-e", "output volume of (get volume settings)"])
        .output()
        .map_err(|e| format!("Could not read output volume ({e})."))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim()
        .parse::<u8>()
        .map_err(|_| format!("Unexpected volume output: {}", s.trim()))
}

#[cfg(target_os = "macos")]
fn macos_output_volume_applescript_set(percent: u8) -> Result<(), String> {
    let v = percent.min(100);
    let script = format!("set volume output volume {v}");
    let st = std::process::Command::new("osascript")
        .args(["-e", &script])
        .status()
        .map_err(|e| format!("Could not change output volume ({e})."))?;
    if st.success() {
        Ok(())
    } else {
        Err("Changing output volume failed (osascript).".to_string())
    }
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn get_macos_output_volume() -> Result<u8, String> {
    tokio::task::spawn_blocking(macos_output_volume_applescript_get)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[cfg(target_os = "macos")]
async fn set_macos_output_volume(percent: u8) -> Result<(), String> {
    let pct = percent.min(100);
    tokio::task::spawn_blocking(move || macos_output_volume_applescript_set(pct))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn get_macos_output_volume() -> Result<u8, String> {
    Ok(0)
}

#[tauri::command]
#[cfg(not(target_os = "macos"))]
async fn set_macos_output_volume(_percent: u8) -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .try_init();

    let pipeline = Arc::new(PipelineCoordinator::new());
    let sona_state = Arc::new(Mutex::new(SonaState::default()));
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_liquid_glass::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(pipeline.clone())
        .manage(sona_state)
        .setup(move |app| {
            let coord = Arc::clone(&*app.state::<Arc<PipelineCoordinator>>());
            let handle = app.handle().clone();
            {
                let coord = coord.clone();
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    pipeline_worker_loop(handle, coord).await;
                });
            }
            tauri::async_runtime::spawn(async move {
                recover_pipeline_queue(handle, coord).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_recordings_dir,
            save_recording,
            list_recordings,
            save_metadata,
            set_recording_display_name,
            delete_recording,
            enqueue_pipeline_jobs,
            get_pipeline_queue_snapshot,
            cancel_pipeline_jobs,
            set_pipeline_job_priority,
            check_sona,
            check_transcription_ready,
            check_ffmpeg,
            get_transcription_settings,
            set_transcription_settings,
            get_models_dir,
            download_diarize_model,
            get_diarize_model_info,
            is_diarization_platform_supported,
            check_ollama,
            check_ollama_model,
            check_blackhole,
            check_audio_routing,
            open_audio_midi_setup,
            setup_audio_capture,
            sync_default_input_to_aggregate,
            begin_recording_audio_routing,
            end_recording_audio_routing,
            get_macos_output_volume,
            set_macos_output_volume,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            #[cfg(target_os = "macos")]
            {
                if let Ok(dir) = app_handle.path().app_data_dir() {
                    let session = dir.join("audio_recording_session.json");
                    audio_setup::end_recording_audio_routing(session.as_path());
                    let p = dir.join("audio_output_restore.json");
                    audio_setup::restore_default_output_if_needed(&p);
                }
            }
        }
    });
}
