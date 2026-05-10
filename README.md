# Meeting Note Taker

A local-first desktop app for recording meetings, transcribing audio, and summarising with AI — all on-device, no cloud required.

Built with **Tauri 2**, **React**, **Vite**, **shadcn/ui**, and **Lucide** icons.
Transcription via **Whisper** · Summarisation via **Gemma 4** (Ollama).

---

## Documentation

| Document | Description |
|----------|-------------|
| **README** *(this file)* | Setup, architecture, configuration reference |
| [docs/USER_GUIDE.md](docs/USER_GUIDE.md) | Step-by-step guide for recording, transcribing, and summarising |

---

## Prerequisites

| Tool | Purpose | Required | Install |
|------|---------|----------|---------|
| [BlackHole 2ch](https://existential.audio/blackhole/) | Virtual audio driver — captures system audio alongside your mic | **Yes** | [existential.audio/blackhole](https://existential.audio/blackhole/) |
| [Ollama](https://ollama.com/download) | Runs Gemma 4 locally | **Yes** | Mac app or `brew install ollama` |
| [openai-whisper](https://github.com/openai/whisper) | Speech-to-text | **Yes** | `pip install openai-whisper` |
| [ffmpeg](https://ffmpeg.org) | WebM → WAV conversion for Whisper | No (recommended) | `brew install ffmpeg` |
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

# 2. Install dependencies
pnpm install

# 3. Run in development
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

Every launch shows a setup screen that verifies **BlackHole 2ch**, **audio routing** (Multi-Output + aggregate when possible), **microphone permission**, Whisper, ffmpeg (optional), Ollama, and the Gemma model. Failed checks expand automatically with copyable terminal commands and step-by-step guides. Click **Re-check** after installing or changing Audio MIDI Setup without restarting the app. When all required checks pass the screen fades out automatically — there is no manual Continue button.

### Audio recording

- **Input device** dropdown lists real microphones, headsets, and **Meeting Note Taker Input** (the app-created aggregate). Standalone loopback drivers such as **BlackHole** are **hidden** on purpose — you should use **Meeting Note Taker Input** to capture **your voice and audio playing on the Mac** together; other listed inputs are effectively **mic-only** unless you built your own aggregate elsewhere.
- **Meeting Note Taker Input** is listed first when present and selected by default; the recorder explains when you are using this preferred input.
- Live waveform visualiser driven by the Web Audio API
- Pause / Resume mid-recording
- **Max duration:** 2 hours — amber warning at 5 minutes remaining, auto-save at the limit

### System audio capture

BlackHole 2ch remains **required** under the hood: the setup flow creates **Meeting Note Taker Output** (Multi-Output) and **Meeting Note Taker Input** (Aggregate: mic + BlackHole). You do **not** pick BlackHole itself in the app — pick **Meeting Note Taker Input** after setup. See [docs/USER_GUIDE.md § 3](docs/USER_GUIDE.md#3-capturing-system-audio) for install, auto-setup, and manual fallback.

### Transcription

Whisper runs locally. Before calling Whisper, the app converts the WebM recording to 16 kHz mono WAV via ffmpeg — this is the format Whisper is optimised for and significantly improves reliability.

### AI summarisation

Gemma 4 (via Ollama) generates a structured summary from the transcript, including key topics, action items, and decisions.

### Playback

Inline audio player per recording with a seekable progress bar and live time counter.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│  Frontend (React + Vite)                        │
│                                                 │
│  SetupScreen ──► 4 requirement checks           │
│                                                 │
│  RecorderPanel ──► useAudioRecorder             │
│    MediaRecorder API + Web Audio (level meter)  │
│    enumerateDevices — hides loopback picks;     │
│    prefers Meeting Note Taker Input             │
│    Max-duration timer with auto-stop            │
│                                                 │
│  RecordingsList ──► RecordingCard               │
│    Playback via readFile + Blob URL             │
│    Seekable progress bar                        │
│    Inline transcript + summary viewer           │
└──────────────────────┬──────────────────────────┘
                       │ Tauri IPC (invoke)
┌──────────────────────▼──────────────────────────┐
│  Backend (Rust / Tauri 2)                       │
│                                                 │
│  check_whisper / check_ffmpeg                   │
│  check_ollama  / check_ollama_model             │
│                                                 │
│  save_recording      writes .webm to disk       │
│  list_recordings     reads recordings dir       │
│  save_metadata       writes JSON sidecar        │
│  delete_recording    removes file + metadata    │
│                                                 │
│  transcribe_with_whisper                        │
│    ffmpeg: .webm → 16kHz mono WAV               │
│    whisper: WAV → transcript text               │
│                                                 │
│  summarize_with_gemma                           │
│    POST localhost:11434/api/generate            │
└──────────────────────┬──────────────────────────┘
                       │
          ┌────────────┴────────────┐
          ▼                         ▼
   whisper CLI               Ollama REST API
   (openai-whisper)          localhost:11434
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
└── metadata/          # <id>.json — transcript, summary, duration, created_at
```

---

## Configuration

### Whisper model

Edit [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs) → `transcribe_with_whisper`:

```rust
"--model", "base",      // tiny | base | small | medium | large
"--language", "en",     // remove for auto language detection
```

| Model | Size | Approx. speed (Apple Silicon) | Accuracy |
|-------|------|-------------------------------|----------|
| `tiny` | 75 MB | ~5× real-time | lowest |
| `base` | 145 MB | ~3× real-time | good |
| `small` | 461 MB | ~1.5× real-time | better |
| `medium` | 1.5 GB | ~0.8× real-time | great |
| `large` | 3 GB | ~0.4× real-time | best |

### Ollama model

Edit [`src-tauri/src/lib.rs`](src-tauri/src/lib.rs) → `summarize_with_gemma`:

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
| Built `.app` claims Whisper/ffmpeg missing (brew works in Terminal) | GUI apps get a minimal `PATH`; the app prepends `/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin`. Rebuild (`pnpm build`) and reopen the bundle. |
| Microphone denied / no Settings entry (`pnpm dev`) | Settings → Microphone → enable **meeting-note-taker** (debug binary), or `tccutil reset Microphone` (all apps), or run `pnpm build` and use the `.app` (then `com.meeting-note-taker.app` applies) |
| "Failed to run whisper" | `pip install openai-whisper` |
| "Failed to connect to Ollama" | `ollama serve` or open Ollama.app |
| "Model not found" | `ollama pull gemma4:e4b` |
| Garbled transcript | `brew install ffmpeg` |
| Wrong language | Remove `--language en` from lib.rs |

---

## Known Limitations

- **UI freezes during transcription** — Whisper runs synchronously; long recordings block the window.
- **No streaming summaries** — Gemma's response arrives all at once.
- **No export** — transcript and summary are view-only inside the app.
- **No search** — recordings cannot be filtered by content.

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
| Transcription | [openai-whisper](https://github.com/openai/whisper) |
| Summarisation | [Gemma 4](https://ollama.com/library/gemma4) via [Ollama](https://ollama.com) |

---

## License

MIT
