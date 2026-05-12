# Meeting Note Taker

A local-first desktop app for recording meetings, transcribing audio, and summarising with AI — all on-device, no cloud required.

Built with **Tauri 2**, **React**, **Vite**, **shadcn/ui**, and **Lucide** icons.
Transcription via **Sona** (bundled) + a **Whisper GGUF** model · Summarisation via **Gemma 4** (Ollama). Optional **speaker diarization** (Sortformer ONNX, same stack as [Vibe](https://github.com/thewh1teagle/vibe)) on supported platforms.

---

## Documentation

| Document | Description |
|----------|-------------|
| **README** *(this file)* | Setup, architecture, configuration reference, **device / hardware notes** |
| [docs/USER_GUIDE.md](docs/USER_GUIDE.md) | Step-by-step guide for recording, transcribing, and summarising |

---

## Device and hardware recommendations

The app does **not** enforce fixed CPU, RAM, or GPU minimums. Use the following to set expectations for **disk**, **memory**, and **performance** with the default stack (`gemma4:e4b` + local Sona transcription).

### Platform

- **macOS** with **Apple Silicon** (M-series) matches the documented **system-audio** setup (BlackHole, aggregate devices). Intel Macs work; **speaker diarization** is unavailable on Intel (no `sona-diarize` binary upstream).

### Disk space

Budget for model downloads, the app, tools, and growing recordings:

| Component | Typical footprint |
|-----------|-------------------|
| Gemma `gemma4:e2b` | ~7.2 GB |
| Gemma `gemma4:e4b` *(default-style)* | ~9.6 GB |
| Gemma `gemma4:26b` / `gemma4:31b` | ~18–20 GB |
| GGUF / `.bin` model (whisper.cpp format) | ~75 MB (`tiny`) … ~3 GB (`large`) — pick in Setup |
| Recordings | Varies; each meeting can be up to **2 hours** of audio plus `metadata/` JSON |

Keep **well above** the model size alone in free space for macOS, dependencies, and recording growth.

### Memory (RAM)

- **`gemma4:e4b`**: **16 GB** system RAM is usually comfortable with other apps open; **8 GB** can work but may swap or feel tight when **Sona** and **Ollama** run together.
- **Larger Gemma models** (`gemma4:26b`, `gemma4:31b`): expect to need **substantially more RAM** than `e4b` (see [Pull the Gemma 4 model](#pull-the-gemma-4-model)).

### GPU

No discrete GPU is required. **Apple Silicon** uses unified memory; Sona can use GPU when available and falls back to CPU on load failure.

---

## Prerequisites

| Tool | Purpose | Required | Install |
|------|---------|----------|---------|
| [BlackHole 2ch](https://existential.audio/blackhole/) | Virtual audio driver — captures system audio alongside your mic | **Yes** | [existential.audio/blackhole](https://existential.audio/blackhole/) |
| [Ollama](https://ollama.com/download) | Runs Gemma 4 locally | **Yes** | Mac app or `brew install ollama` |
| [Sona](https://github.com/thewh1teagle/sona) | Local ASR server (bundled sidecar) | **Yes** | `pnpm prebuild:sona` or `python3 scripts/pre_build.py` → `src-tauri/binaries/` (see [.sona-version](.sona-version)) |
| Python **3.10+** | Runs `scripts/pre_build.py` (stdlib only) | Dev only | `python3 --version` or [python.org](https://www.python.org/downloads/) |
| Whisper **GGUF** / `.bin` | Model weights for Sona | **Yes** | Download from [whisper.cpp models](https://huggingface.co/ggerganov/whisper.cpp); set path in Setup |
| [ffmpeg](https://ffmpeg.org) | WebM → WAV (and bundled on macOS/Windows app builds) | Optional | `brew install ffmpeg` if not bundled |
| [Rust](https://rustup.rs) | Tauri backend | Dev only | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Node.js ≥ 18 | Frontend build | Dev only | [nodejs.org](https://nodejs.org) or `brew install node` |
| pnpm | Package manager | Dev only | `npm i -g pnpm` |

### Pull the Gemma 4 model

```bash
ollama pull gemma4:e4b     # ~9.6 GB — recommended for most machines
# ollama pull gemma4:26b   # ~18 GB — higher quality, needs more RAM
```

> If you installed the Ollama Mac app it starts automatically on login.  
> If you used Homebrew, run `brew services start ollama` once to register it as a background service.

---

## Getting Started

```bash
# 1. Clone
git clone https://github.com/your-username/meeting-note-taker.git
cd meeting-note-taker

# 2. Download Sona sidecars (required before first `pnpm dev` / `pnpm build`)
pnpm prebuild:sona
# Same as: python3 scripts/pre_build.py  (stdlib only; Python 3.10+)

# 3. Install dependencies
pnpm install

# 4. Run in development
pnpm dev
```

On first launch the app runs a **setup screen** that checks all dependencies and shows step-by-step fix instructions for anything missing. When **every required check passes**, the screen fades out and the main app opens automatically (after a short pause). If something fails, use **Re-check** after fixing it — there is no way to skip ahead with broken requirements.

### Production build

```bash
pnpm build
# Output: src-tauri/target/release/bundle/
```

---

## Features

### Startup requirements check

Every launch shows a setup screen that verifies **BlackHole 2ch**, **audio routing**, **microphone permission**, **Sona + transcription model path**, ffmpeg (optional if bundled), Ollama, and the Gemma model. Optional **speaker diarization** needs the Sortformer ONNX file (download from Setup). **Intel Mac:** diarization is disabled (upstream binary gap).

### Audio recording

- **Input device** dropdown lists real microphones, headsets, and **Meeting Note Taker Input** (the app-created aggregate). Standalone loopback drivers such as **BlackHole** are **hidden** on purpose — you should use **Meeting Note Taker Input** to capture **your voice and audio playing on the Mac** together; other listed inputs are effectively **mic-only** unless you built your own aggregate elsewhere.
- **Meeting Note Taker Input** is listed first when present and selected by default; the recorder explains when you are using this preferred input.
- Live waveform visualiser driven by the Web Audio API
- Pause / Resume mid-recording
- **Max duration:** 2 hours — amber warning at 5 minutes remaining, auto-save at the limit

### System audio capture

BlackHole 2ch remains **required** under the hood: the setup flow creates **Meeting Note Taker Output** (Multi-Output) and **Meeting Note Taker Input** (Aggregate: mic + BlackHole). You do **not** pick BlackHole itself in the app — pick **Meeting Note Taker Input** after setup. See [docs/USER_GUIDE.md § 3](docs/USER_GUIDE.md#3-capturing-system-audio) for install, auto-setup, and manual fallback.

### AI pipeline (single queue)

After you **stop and save**, the recording appears in the list with **`pipeline_status: idle`**. Nothing runs until you **queue** it:

- Click **Summarize** (or **Re-summarize** on fully processed items) on a card, **or**
- Select one or more eligible recordings and use **Queue for processing**.

All of the above use the **same** backend queue: **one** recording is processed at a time (**Sona transcription** → **Gemma**). Extra items wait in a **FIFO** queue.

- **Metadata** (`metadata/<id>.json`) stores `transcript`, `summary`, and pipeline fields (`pipeline_status`, `pipeline_error`, `pipeline_priority`) so results survive **app restarts** without re-running.
- On launch, **incomplete** jobs (e.g. stuck in **queued** / **transcribing** / **summarizing**) are **re-queued** automatically where appropriate; **failed** and **idle** items are not auto-started.

**Cancel / remove from queue:** **Remove from queue** (per card) or **Cancel queue** (batch) stops the active job and/or clears matching IDs from the queue.

Statuses include: `queued`, `transcribing`, `summarizing`, `done`, `failed`, and `idle` (not in the queue).

See [docs/USER_GUIDE.md § 4–5](docs/USER_GUIDE.md#4-transcription-and-the-background-ai-queue).

### Transcription

The **Sona** sidecar transcribes audio over HTTP. The app converts WebM to 16 kHz mono WAV via ffmpeg when available (bundled ffmpeg on macOS/Windows release builds). With **speaker diarization** enabled (supported platforms only), lines are prefixed with `[Speaker N]` in the stored transcript.

Settings persist in `<app-data>/transcription_settings.json` and the diarization ONNX in `<app-data>/models/`.

### AI summarisation

Gemma 4 (via Ollama) generates a structured summary from the transcript, including key topics, action items, and decisions. After you enqueue a recording, this runs **after** transcription in the **same** pipeline job.

### Playback

Inline audio player per recording with a seekable progress bar and live time counter.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│  Frontend (React + Vite)                        │
│                                                 │
│  SetupScreen — dependency & routing checks              │
│                                                 │
│  RecorderPanel ──► useAudioRecorder             │
│    MediaRecorder API + Web Audio (level meter)  │
│    enumerateDevices — hides loopback picks;     │
│    prefers Meeting Note Taker Input             │
│    Max-duration timer with auto-stop            │
│                                                 │
│  RecordingsList ──► RecordingCard               │
│    Playback via readFile + Blob URL             │
│    Search + status filters · Batch queue tools   │
│    listen: pipeline-job-update → refresh        │
└──────────────────────┬──────────────────────────┘
                       │ Tauri IPC (invoke + events)
┌──────────────────────▼──────────────────────────┐
│  Backend (Rust / Tauri 2)                       │
│                                                 │
│  check_transcription_ready / check_sona / check_ffmpeg                   │
│  check_ollama  / check_ollama_model             │
│                                                 │
│  save_recording      writes .webm to disk       │
│  list_recordings     reads recordings + meta    │
│  save_metadata       merges JSON sidecar        │
│  delete_recording    removes file + metadata    │
│                                                 │
│  Background pipeline (one job at a time)        │
│    enqueue_pipeline_jobs · cancel_pipeline_jobs  │
│    set_pipeline_job_priority                    │
│    recover incomplete on startup                │
│    emit pipeline-job-update                     │
│                                                 │
│  Per job (worker):                              │
│    run_sona_transcription → transcript          │
│    (streams transcription-chunk to UI)          │
│    Ollama POST /api/generate → summary          │
└──────────────────────┬──────────────────────────┘
                       │
          ┌────────────┴────────────┐
          ▼                         ▼
   sona sidecar              Ollama REST API
   (+ optional sona-diarize) localhost:11434
   speech → text             text → summary
                             (gemma4:e4b)
```

### Data storage

All data is stored locally — nothing leaves your machine.

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/com.meeting-note-taker.app/` |
| Linux | `~/.local/share/com.meeting-note-taker.app/` |
| Windows | `%APPDATA%\com.meeting-note-taker.app\` |

```
<app-data>/
├── recordings/        # .webm audio files
├── models/            # diarization ONNX (optional)
├── transcription_settings.json
└── metadata/          # <id>.json — transcript, summary, duration, created_at,
                        #   pipeline_status, pipeline_error, pipeline_priority
```

---

## Configuration

### Transcription (Sona + GGUF)

1. Run **`pnpm prebuild:sona`** so `src-tauri/binaries/` contains `sona`, `sona-diarize` (where supported), and **ffmpeg** on macOS/Windows bundles.
2. Download a **whisper.cpp-compatible** model (e.g. `ggml-base.bin` from Hugging Face) and paste its **full path** into **Setup → Transcription settings → Save**.
3. Optional: enable **Speaker diarization** and download the ONNX model from Setup (~25 MB). **Not available on Intel Mac.**

Language is currently fixed to English in the pipeline (`run_sona_transcription` in [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs)); adjust `language` in code if you need another language.

### Ollama model

Edit [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs) → `run_summarize_internal` (the `OllamaRequest` `model` field used by the pipeline worker):

```rust
model: "gemma4:e4b".to_string(),
// options: gemma4:e2b (7.2GB) | gemma4:e4b (9.6GB) | gemma4:26b (18GB) | gemma4:31b (20GB)
```

### Max recording duration

Edit [`src/hooks/useAudioRecorder.ts`](src/hooks/useAudioRecorder.ts):

```ts
const MAX_DURATION_SECS = 7200; // 2 hours
const WARN_BEFORE_SECS  = 300;  // warn 5 min before
```

---

## Troubleshooting

See [docs/USER_GUIDE.md § 8 — Troubleshooting](docs/USER_GUIDE.md#8-troubleshooting) for detailed solutions to every common error.

Quick reference:

| Error | Fix |
|-------|-----|
| "BlackHole 2ch not found" | Install from [existential.audio/blackhole](https://existential.audio/blackhole/), then Re-check |
| Missing **Meeting Note Taker Input** in the recorder | Fix **Audio routing** + microphone on the setup screen, then Re-check; see [USER_GUIDE §8](docs/USER_GUIDE.md#meeting-note-taker-input-is-missing-from-the-recorder) |
| Built `.app` claims Sona/ffmpeg missing | **If you use a downloaded app:** stay online or reinstall — the app can fetch Sona when the bundle is incomplete. **If you package the app yourself:** run **`pnpm prebuild:sona`** before **`pnpm build`** so `src-tauri/binaries/` is populated. |
| "Sona binary not found" | Stay online, **Re-check** in Setup, or reinstall from the official download. (From source: **`pnpm prebuild:sona`** before **`pnpm build`** — see prerequisites above.) |
| Microphone denied / no Settings entry (`pnpm dev`) | Settings → Microphone → enable **meeting-note-taker** (debug binary), or `tccutil reset Microphone` (all apps), or run `pnpm build` and use the `.app` (then `com.meeting-note-taker.app` applies) |
| Transcription model path missing | Setup → paste full path to GGUF / `.bin` → Save. |
| "Failed to connect to Ollama" | `ollama serve` or open Ollama.app |
| "Model not found" | `ollama pull gemma4:e4b` |
| Garbled transcript | Install ffmpeg or use a desktop build that bundles ffmpeg |
| Wrong language | Edit `language` in `run_sona_transcription` (`src-tauri/src/lib.rs`) |
| **AI failed** / queue stuck | Check Sona + GGUF path + Ollama; click **Summarize** again on the card to re-enqueue; see [USER_GUIDE §8 — Background job stuck or AI failed](docs/USER_GUIDE.md#background-job-stuck-or-ai-failed) |

---

## Known Limitations

- **No blocking “manual” transcribe/summarize** — both steps run only through the **shared queue**; the main button enqueues and the UI stays responsive while you use the app.
- Sona streams **segment lines** to the card while a job is **transcribing**; **Gemma** still returns the summary in one shot.
- **No export** — transcript and summary are view-only inside the app.

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Desktop shell | [Tauri 2](https://v2.tauri.app) |
| Frontend | [React 19](https://react.dev) + [Vite 8](https://vite.dev) |
| Styling | [Tailwind CSS v4](https://tailwindcss.com) |
| UI primitives | [Radix UI](https://www.radix-ui.com) (shadcn/ui style) |
| Icons | [Lucide React](https://lucide.dev) |
| Backend | Rust (stable) |
| Transcription | [Sona](https://github.com/thewh1teagle/sona) + whisper.cpp GGUF |
| Summarisation | [Gemma 4](https://ollama.com/library/gemma4) via [Ollama](https://ollama.com) |

---

## License

MIT
