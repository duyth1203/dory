# User Guide — Dory

## Table of Contents

1. [First Launch — Setup Screen](#1-first-launch--setup-screen)
   - [Device expectations (RAM and disk)](#device-expectations-ram-and-disk)
2. [Recording a Meeting](#2-recording-a-meeting)
3. [Capturing System Audio](#3-capturing-system-audio)
4. [Transcription and the background AI queue](#4-transcription-and-the-background-ai-queue)
5. [Summarising with AI](#5-summarising-with-ai)
6. [Playback and Navigation](#6-playback-and-navigation)
7. [Managing Recordings](#7-managing-recordings)
8. [Troubleshooting](#8-troubleshooting)

---

## 1. First Launch — Setup Screen

Every time the app opens it runs a health check on its dependencies before showing the main interface.

```
┌─────────────────────────────────────────┐
│  Getting ready…                         │
│                                         │
│  ✅  BlackHole 2ch   2ch                │
│  ✅  Sona transcription  ready                 │
│  ⚠️  ffmpeg          optional           │
│  ✅  Ollama          0.6.1              │
│  ✅  Gemma 4 (e4b)   4.5B               │
│                                         │
│  [Re-check]                             │
└─────────────────────────────────────────┘
```

When all required checks pass the screen **fades out automatically** — there is no Continue button. If any required check fails, fix the issue and click **Re-check**; the main app opens only after every required item is green.

### Status icons

| Icon | Meaning |
|------|---------|
| ⏳ spinning | Currently checking |
| ✅ green | Installed and ready |
| ⚠️ amber | Optional — app works without it, but quality may be reduced |
| ❌ red | Required and missing — must be resolved before the app will open |

### What each check verifies

| Requirement | Why it's needed | Required? |
|-------------|----------------|-----------|
| **BlackHole 2ch** | Virtual audio driver that captures system audio alongside your mic | **Yes** |
| **Sona + GGUF model** | Bundled Sona sidecar and path to a whisper.cpp-compatible model | **Yes** |
| **ffmpeg** | Converts WebM audio to WAV before transcription — improves reliability | No |
| **Ollama** | Runs the Gemma AI model locally | **Yes** |
| **Gemma 4 (e4b)** | Generates meeting summaries from transcripts | **Yes** |

### When a check fails

Click the **Guide** link next to any failed item to expand step-by-step fix instructions with copyable terminal commands. After following the steps, click **Re-check** to verify without restarting the app.

### Device expectations (RAM and disk)

The app does not enforce a minimum CPU or RAM tier. For **disk budgets** (models + recordings), **RAM** guidance for `gemma4:e4b` vs larger Gemma variants, and **Apple Silicon** context for performance, see **[Device and hardware recommendations](../README.md#device-and-hardware-recommendations)** in the README.

---

## 2. Recording a Meeting

### Step 1 — Choose your input device

The **Input device** dropdown lists microphones, headsets, and — after successful setup — **Dory Input**.

| Input | What gets recorded |
|-------|--------------------|
| **Dory Input** *(preferred)* | Your **voice** and **audio playing on this Mac** (meetings, browser, media) — uses the app’s aggregate (microphone + BlackHole). |
| **Built-in / USB / headset mic** | **Microphone only.** Remote participants’ voices coming out of your speakers are **not** included unless you use the preferred input and routing from §3. |

**Important:** Standalone loopback devices (for example **BlackHole 2ch** by itself) are **not shown** in the list. That avoids picking “system only” by mistake. BlackHole is still installed and used **inside** **Dory Input**.

When **Dory Input** appears, the app **sorts it to the top** and **selects it by default**. If you choose a plain mic while the preferred input exists, a short **amber** hint may suggest switching back.

> The dropdown is disabled while recording. Choose your device before you start.

> Choosing **Dory Input** can sync macOS **default input** with Sound settings so the name matches what you hear in other apps (see in-app help text).

### Step 2 — Start recording

Click the **red microphone button**. macOS will ask for microphone permission the first time — click **Allow**.

The waveform visualiser lights up in red and the timer starts counting.

### Step 3 — Pause if needed

Click **⏸ Pause** to pause mid-meeting (for a break or off-the-record conversation). Click **▶ Resume** to continue. The timer stops while paused.

### Step 4 — Stop and save

Click the **red square (■)** to stop. The recording is saved instantly to your local app-data folder and appears in the recordings list on the right.

**Transcript + summary:** The new recording is saved with **`pipeline_status: idle`**. Nothing runs until you add it to the AI queue — either click **Summarize** on that card or select it (with others) and use **Queue for processing**. Everything shares **one** FIFO worker (**Sona** then **Gemma**). Only **one** recording runs at a time; the rest wait. Card badges show **Queued**, **Transcribing**, **Summarizing**, or **AI failed** as appropriate. See [Section 4](#4-transcription-and-the-background-ai-queue) for batch queue, **Cancel queue**, and recovery.

### Recording limits

- **Maximum duration:** 2 hours per recording
- **Warning:** The timer turns amber and a pulsing banner appears when 5 minutes remain
- **Auto-stop:** At the 2-hour mark the app stops and saves the recording automatically

---

## 3. Capturing System Audio

Dory needs **BlackHole 2ch** so **Dory Input** can combine your **microphone** with **everything playing through the Mac** (Zoom, Google Meet, YouTube, etc.) in one recording.

BlackHole is verified on the **setup screen**. It must be installed before the app will pass startup checks; you normally do **not** select BlackHole in the recorder — select **Dory Input** instead ([Section 2](#2-recording-a-meeting)).

### Automatic setup (normal path)

When BlackHole is installed and **Audio routing** succeeds on the setup screen, the app creates:

- **Dory Output** — Multi-Output (your speakers/headphones + BlackHole) so sound still comes out of your speakers while a copy feeds BlackHole  
- **Dory Input** — Aggregate (your mic + BlackHole) so one input carries **voice + system audio**

Then open the recorder and leave **Dory Input** selected (or pick it from the list). See the **Audio routing** item on the setup screen for restore behaviour when you quit the app.

### Manual install — BlackHole only

Download and install from [existential.audio/blackhole](https://existential.audio/blackhole/).

After installing you may need to reload the Core Audio daemon for the device to appear:

```bash
sudo launchctl kickstart -kp system/com.apple.audio.coreaudiod
```

Return to the app and tap **Re-check**. If routing still fails, continue with the steps below (or follow the **Audio routing** guide in the setup screen).

### Manual Multi-Output (if automatic setup failed)

This lets your speakers/headphones continue to work while BlackHole captures a copy of system output.

1. Open **Audio MIDI Setup** (Spotlight → "Audio MIDI Setup"):
   ```bash
   open -a 'Audio MIDI Setup'
   ```
2. Click **+** → **Create Multi-Output Device**
3. Check both **BlackHole 2ch** and your speakers or headphones
4. Set this Multi-Output Device as your Mac's system output:
   - Option-click the speaker icon in the menu bar  
   - Select your new Multi-Output Device  

You can name it anything; the app’s automatic device is usually **Dory Output**.

### Manual Aggregate (if automatic setup failed)

This combines BlackHole 2ch with your microphone into a single recordable input.

1. In Audio MIDI Setup, click **+** → **Create Aggregate Device**
2. Check **BlackHole 2ch** and your microphone
3. In Dory, select **that aggregate** from the **Input device** dropdown — or use **Dory Input** if the app created it after **Re-check**

> Standalone BlackHole will not appear in the recorder’s input list; use **Dory Input** (or another aggregate you created).

---

## 4. Transcription and the background AI queue

### After save: idle until you queue

When you stop a recording, Dory:

1. Saves the audio file and metadata with **`pipeline_status: idle`**
2. Shows the item in the list — **no** transcription or Gemma run yet

To process it, use either:

- **Summarize** on the card (same action as **Re-summarize** when both transcript and summary already exist — that clears prior text and re-queues a **full** run), **or**
- **Queue for processing** in the list toolbar (multi-select + batch queue)

Both call the same Rust entry point — **`enqueue_pipeline_jobs`** — and append to **one** internal FIFO queue.

### What the worker does

When an item reaches the front of the queue:

1. **Sona** (if there is no transcript yet) → writes **transcript**, emits **`transcription-chunk`** lines for live preview  
2. **Gemma** (if there is no summary yet) → writes **summary**  
3. Sets **`pipeline_status: done`** (or **`failed`** + **`pipeline_error`**)

You can **keep using the app** while work runs. The list **refreshes** on **`pipeline-job-update`** events.

### Queue behaviour

- If a job is already running, new IDs stay **Queued** until their turn.
- **Remove from queue** (card) or **Cancel queue** (toolbar) cancels transcription/Gemma for matching IDs and/or removes them from the waiting queue.
- After you quit and reopen, **incomplete** in-flight states are **re-queued** where appropriate; **`failed`** and **`idle`** are not auto-started.

### Card badges and actions

| UI | Meaning |
|----|---------|
| **Queued** | Waiting for the worker |
| **Transcribing** / **Summarizing** | Pipeline step in progress for this recording |
| **AI failed** | See error on the card; click **Summarize** again to re-enqueue |
| **Priority** | Metadata flag for this queued item (moves to front when set via `set_pipeline_job_priority`) |
| **Summarize** / **Re-summarize** | Enters this recording into the **same** queue as **Queue for processing** |

### Persistence

All results live in **`metadata/<id>.json`** (same folder as in [Section 7](#7-managing-recordings)). Reopening the app loads transcripts and summaries from disk with **no** re-processing unless the pipeline is incomplete or you enqueue again.

### How Sona fits in (technical)

```
Recording (.webm)
      ↓  ffmpeg converts to WAV 16 kHz mono (recommended)
Sona (+ optional sona-diarize for speaker labels)
      ↓  plain text transcript ([Speaker N] lines when diarization is on)
metadata JSON + streaming chunks to the UI (during transcribe step)
```

### Expected times (Apple Silicon, base model)

| Recording length | Approx. time |
|-----------------|-------------|
| 5 minutes | ~30 seconds |
| 30 minutes | ~3 minutes |
| 1 hour | ~6 minutes |

Work runs in the **background worker**; the window stays usable.

### Improving accuracy

Use a **larger GGUF** (e.g. `medium` / `large`) and set its path in **Setup → Transcription settings**. To change language, edit the `language` field in **`run_sona_transcription`** in `src-tauri/src/lib.rs`.

---

## 5. Summarising with AI

### Same queue as the card **Summarize** button

**Gemma** always runs **after** a usable transcript exists, inside the **same pipeline job** as **Sona** when started from **idle**. There are no separate “manual” `summarize` / `transcribe` IPC commands — only **`enqueue_pipeline_jobs`** (used by the card button and the batch toolbar).

### Using **Summarize** on a card

1. Ensure the recording is **not** already **Queued / Transcribing / Summarizing** (the button is disabled while it is).
2. Click **Summarize** (or **Re-summarize** if both transcript and summary exist — this clears them and re-runs from audio).
3. The card label becomes **Queued…** / **Transcribing…** / **Summarizing…** while the shared worker runs.
4. Expand **Summary** on the card to read the result.

If there is **no transcript** yet, the worker runs **Sona** first. If only the summary is missing, it skips to Gemma.

### How it works internally

```
Transcript (plain text)
      ↓  POST localhost:11434/api/generate
Gemma 4 e4b model
      ↓  structured summary
Summary stored in metadata JSON
```

### Ollama must be running

If you see _"Failed to connect to Ollama"_:
- Open the Ollama menu-bar app, or
- Run `ollama serve` in a terminal

Check it's up: `curl http://localhost:11434` → should return `Ollama is running`.

---

## 6. Playback and Navigation

Each recording card has a built-in audio player:

```
[▶]  ──────────●────────────  0:42 / 5:13
```

| Control | Action |
|---------|--------|
| **▶ / ⏸** | Play or pause |
| **Click on bar** | Seek to that position |
| **0:42 / 5:13** | Current time / total duration |

The audio is loaded from your local app-data folder — no streaming, no network.

---

## 7. Managing Recordings

### Where files are stored

All recordings and metadata are stored locally — nothing is sent to the cloud.

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/com.dory.app/` |
| Linux | `~/.local/share/com.dory.app/` |
| Windows | `%APPDATA%\com.dory.app\` |

```
<app-data>/
├── recordings/      ← .webm audio files
├── models/          ← optional diarization ONNX
├── transcription_settings.json
└── metadata/        ← <id>.json sidecar:
                       id, transcript, summary, duration_secs, created_at,
                       pipeline_status, pipeline_error, pipeline_priority
```

`pipeline_status` is typically `queued` → `transcribing` → `summarizing` → `done`, or `failed` / `idle` as described in §4.

### List toolbar (search, batch queue)

Use the **search** field to filter by name, id, filename, or snippets of transcript/summary. The **status** dropdown limits the list (for example **In queue / processing** or **Failed**). **Select all** / **Queue for processing** / **Cancel queue** operate on the **filtered** list (fully summarized rows are excluded from batch selection).

### Deleting a recording

Click the **🗑 trash icon** on a recording card. Both the audio file and its metadata (transcript + summary) are permanently deleted.

### Re-summarising (overwrite)

Click **Re-summarize** when both **Transcribed** and **Summarized** are present. The app clears the stored transcript and summary, sets **`queued`**, and runs **Sona → Gemma** again — useful after switching to a different GGUF model or editing the recording title for clarity (content still comes from the audio file).

There is no separate **Re-transcribe** control: a full redo is always **Re-summarize** (full pipeline).

---

## 8. Troubleshooting

### "BlackHole 2ch audio driver not found"

BlackHole 2ch is not installed or was not detected.

1. Download and install from [existential.audio/blackhole](https://existential.audio/blackhole/)
2. Reload the audio daemon so macOS registers the new device:
   ```bash
   sudo launchctl kickstart -kp system/com.apple.audio.coreaudiod
   ```
3. Click **Re-check** in the setup screen — the app will proceed automatically once detected.

If you already installed BlackHole but the check still fails, verify the driver is present:
```bash
ls /Library/Audio/Plug-Ins/HAL/ | grep -i blackhole
```
You should see `BlackHole2ch.driver`.

---

### Dory Input is missing from the recorder

The app only lists inputs macOS exposes after **microphone permission** and **working audio routing**.

1. On the **setup screen**, ensure **Microphone permission**, **BlackHole 2ch**, and **Audio routing** are all **OK** (use **Re-check** after Audio MIDI Setup changes).
2. Grant microphone access if prompted (see [_Could not access microphone_](#could-not-access-microphone) below).
3. **Quit and reopen** the app; sometimes WebKit lags after new aggregate devices appear.
4. Open **Audio MIDI Setup** and confirm an aggregate exists whose name includes **Dory Input** (or your manual aggregate with mic + BlackHole). If routing failed, complete §3 manually, then **Re-check**.

**Why BlackHole does not appear in the list:** The recorder **hides** standalone loopback drivers so you do not record “system only” without your mic. Use **Dory Input** (or another aggregate you created) for mic + system audio.

---

### "Could not access microphone"

macOS denied microphone access, or the app never got a usable TCC entry.

**If you run the app with `pnpm dev` (`tauri dev`):** the process is usually the **debug binary** (`dory`), not the shipped app bundle. **System Settings → Privacy & Security → Microphone** may list **dory** (or a path under `target/debug`) instead of “Dory”. Enable that entry, or use a built app for the normal name (see below).

```bash
# Packaged .app only — resets permission for the bundle id from tauri.conf.json
tccutil reset Microphone com.dory.app

# Dev-friendly: reset microphone prompts for *all* apps (you will be re-prompted elsewhere too)
tccutil reset Microphone
```

Then relaunch the app and click Record.

**Packaged app:** run `pnpm build` and open **Dory** from `src-tauri/target/release/bundle/macos/`. Then Settings shows the real app name and the bundle-id reset above matches.

Alternatively: **System Settings → Privacy & Security → Microphone** and enable the correct process for how you launch the app.

---

### "sona binary not found" or transcription setup fails

**Normal install (downloaded app):** Dory downloads Sona when you are online and the bundled copy is missing. Try, in order:

1. Connect to the internet and click **Re-check** on the setup screen (or quit and reopen the app).
2. Rule out VPNs, captive portals, or strict firewalls blocking GitHub downloads.
3. Download and install the app again from the **official release** you got it from.

In **Setup**, you must still set the full path to a **Whisper GGUF** / `.bin` model file and click **Save transcription settings**.

**Building from source:** Contributors must populate `src-tauri/binaries/` before packaging — run **`pnpm prebuild:sona`** or **`python3 scripts/pre_build.py`** from the repo root (Python 3.10+; on Windows, **`py -3 scripts\pre_build.py`**). See **README** prerequisites.

**Speaker diarization** is unavailable on **Intel Mac** (upstream binary). On supported platforms, download the ONNX model from Setup when diarization is enabled.

---

### "ffmpeg conversion failed" or garbled transcript

Install ffmpeg:

```bash
brew install ffmpeg
ffmpeg -version    # verify
```

---

### "Failed to connect to Ollama"

Ollama is not running:

```bash
ollama serve        # start in terminal
# or open the Ollama.app from Applications
```

---

### "Model 'gemma4:e4b' not found"

The model hasn't been downloaded yet:

```bash
ollama pull gemma4:e4b    # ~9.6 GB, takes a few minutes
ollama list               # verify it appears
```

---

### Transcript is in the wrong language

Edit **`run_sona_transcription`** in `src-tauri/src/lib.rs` — the **`language`** field passed to Sona (default `en`).

---

### The **Summarize** button stays on one label while something else runs

**Summarize** is disabled while **this** recording is **Queued / Transcribing / Summarizing**. Work always goes through the **shared queue**; the button does not block the whole window.

If you click **Summarize** but nothing new is enqueued (for example the ID was already waiting), the app shows a short notice — check badges on the card.

### Background job stuck or **AI failed**

1. Confirm **Sona sidecars** (`pnpm prebuild:sona`), **GGUF path** in Setup, and **Ollama** + **Gemma** (setup screen).
2. Read the error text on the card (also stored as `pipeline_error` in metadata).
3. Click **Summarize** again to re-enqueue; a successful run sets **`done`** and clears **`pipeline_error`**.
4. For recordings stuck in **`idle`** with partial data, enqueue via **Summarize** or batch **Queue for processing** — nothing auto-starts from **idle** alone.
