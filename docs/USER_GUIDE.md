# User Guide — Meeting Note Taker

## Table of Contents

1. [First Launch — Setup Screen](#1-first-launch--setup-screen)
2. [Recording a Meeting](#2-recording-a-meeting)
3. [Capturing System Audio](#3-capturing-system-audio)
4. [Transcribing a Recording](#4-transcribing-a-recording)
5. [Summarising with AI](#5-summarising-with-ai)
6. [Playback and Navigation](#6-playback-and-navigation)
7. [Managing Recordings](#7-managing-recordings)
8. [Troubleshooting](#8-troubleshooting)

---

## 1. First Launch — Setup Screen

Every time the app opens it runs a health check on its five dependencies before showing the main interface.

```
┌─────────────────────────────────────────┐
│  Getting ready…                         │
│                                         │
│  ✅  BlackHole 2ch   2ch                │
│  ✅  Whisper         0.9.0              │
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
| **Whisper** | Converts your recordings to text | **Yes** |
| **ffmpeg** | Converts WebM audio to WAV before transcription — improves reliability | No |
| **Ollama** | Runs the Gemma AI model locally | **Yes** |
| **Gemma 4 (e4b)** | Generates meeting summaries from transcripts | **Yes** |

### When a check fails

Click the **Guide** link next to any failed item to expand step-by-step fix instructions with copyable terminal commands. After following the steps, click **Re-check** to verify without restarting the app.

---

## 2. Recording a Meeting

### Step 1 — Choose your input device

The **Input device** dropdown lists microphones, headsets, and — after successful setup — **Meeting Note Taker Input**.

| Input | What gets recorded |
|-------|--------------------|
| **Meeting Note Taker Input** *(preferred)* | Your **voice** and **audio playing on this Mac** (meetings, browser, media) — uses the app’s aggregate (microphone + BlackHole). |
| **Built-in / USB / headset mic** | **Microphone only.** Remote participants’ voices coming out of your speakers are **not** included unless you use the preferred input and routing from §3. |

**Important:** Standalone loopback devices (for example **BlackHole 2ch** by itself) are **not shown** in the list. That avoids picking “system only” by mistake. BlackHole is still installed and used **inside** **Meeting Note Taker Input**.

When **Meeting Note Taker Input** appears, the app **sorts it to the top** and **selects it by default**. Green helper text confirms _“Using preferred input — voice and device audio.”_ If you choose a plain mic while the preferred input exists, an amber hint suggests switching back.

> The dropdown is disabled while recording. Choose your device before you start.

> Choosing **Meeting Note Taker Input** can sync macOS **default input** with Sound settings so the name matches what you hear in other apps (see in-app help text).

### Step 2 — Start recording

Click the **red microphone button**. macOS will ask for microphone permission the first time — click **Allow**.

The waveform visualiser lights up in red and the timer starts counting.

### Step 3 — Pause if needed

Click **⏸ Pause** to pause mid-meeting (for a break or off-the-record conversation). Click **▶ Resume** to continue. The timer stops while paused.

### Step 4 — Stop and save

Click the **red square (■)** to stop. The recording is saved instantly to your local app-data folder and appears in the recordings list on the right.

### Recording limits

- **Maximum duration:** 2 hours per recording
- **Warning:** The timer turns amber and a pulsing banner appears when 5 minutes remain
- **Auto-stop:** At the 2-hour mark the app stops and saves the recording automatically

---

## 3. Capturing System Audio

Meeting Note Taker needs **BlackHole 2ch** so **Meeting Note Taker Input** can combine your **microphone** with **everything playing through the Mac** (Zoom, Google Meet, YouTube, etc.) in one recording.

BlackHole is verified on the **setup screen**. It must be installed before the app will pass startup checks; you normally do **not** select BlackHole in the recorder — select **Meeting Note Taker Input** instead ([Section 2](#2-recording-a-meeting)).

### Automatic setup (normal path)

When BlackHole is installed and **Audio routing** succeeds on the setup screen, the app creates:

- **Meeting Note Taker Output** — Multi-Output (your speakers/headphones + BlackHole) so sound still comes out of your speakers while a copy feeds BlackHole  
- **Meeting Note Taker Input** — Aggregate (your mic + BlackHole) so one input carries **voice + system audio**

Then open the recorder and leave **Meeting Note Taker Input** selected (or pick it from the list). See the **Audio routing** item on the setup screen for restore behaviour when you quit the app.

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

You can name it anything; the app’s automatic device is usually **Meeting Note Taker Output**.

### Manual Aggregate (if automatic setup failed)

This combines BlackHole 2ch with your microphone into a single recordable input.

1. In Audio MIDI Setup, click **+** → **Create Aggregate Device**
2. Check **BlackHole 2ch** and your microphone
3. In Meeting Note Taker, select **that aggregate** from the **Input device** dropdown — or use **Meeting Note Taker Input** if the app created it after **Re-check**

> In the recorder, green helper text **“Using preferred input — voice and device audio”** appears when **Meeting Note Taker Input** is selected. Standalone BlackHole will not appear in the list.

---

## 4. Transcribing a Recording

1. Find the recording in the list on the right
2. Click **Transcribe**
3. The button shows a spinner and **Transcribing…** while Whisper runs
4. When done, click **▾** to expand the card and read the transcript

### How it works internally

```
Recording (.webm)
      ↓  ffmpeg converts to WAV 16kHz mono
Whisper base model (runs on CPU/GPU)
      ↓  outputs plain text
Transcript stored in app data
```

### Expected times (Apple Silicon, base model)

| Recording length | Approx. time |
|-----------------|-------------|
| 5 minutes | ~30 seconds |
| 30 minutes | ~3 minutes |
| 1 hour | ~6 minutes |

> The app UI is unresponsive while Whisper runs. For long recordings, wait for the spinner to stop before clicking elsewhere.

### Improving accuracy

Edit `src-tauri/src/lib.rs` → `transcribe_with_whisper` to use a larger model:

```
"--model", "small",   // better than base, ~460 MB
"--model", "medium",  // great accuracy, ~1.5 GB
```

Remove `"--language", "en"` for automatic language detection.

---

## 5. Summarising with AI

Summarisation requires a transcript to already exist. If **Summarize** is greyed out, run **Transcribe** first.

1. Click **Summarize** on a transcribed recording
2. Gemma 4 generates a structured summary including:
   - Key topics discussed
   - Action items
   - Decisions made
3. Click **▾** to expand and read the summary

### How it works internally

```
Transcript (plain text)
      ↓  sent to Ollama at localhost:11434
Gemma 4 e4b model
      ↓  generates structured summary
Summary stored alongside transcript
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
| macOS | `~/Library/Application Support/com.meeting-note-taker.app/` |
| Linux | `~/.local/share/com.meeting-note-taker.app/` |
| Windows | `%APPDATA%\com.meeting-note-taker.app\` |

```
<app-data>/
├── recordings/      ← .webm audio files
└── metadata/        ← <id>.json (transcript, summary, duration, date)
```

### Deleting a recording

Click the **🗑 trash icon** on a recording card. Both the audio file and its metadata (transcript + summary) are permanently deleted.

### Re-transcribing or Re-summarising

Click **Re-transcribe** or **Re-summarize** to overwrite a previous result — useful after switching to a more accurate Whisper model.

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

### Meeting Note Taker Input is missing from the recorder

The app only lists inputs macOS exposes after **microphone permission** and **working audio routing**.

1. On the **setup screen**, ensure **Microphone permission**, **BlackHole 2ch**, and **Audio routing** are all **OK** (use **Re-check** after Audio MIDI Setup changes).
2. Grant microphone access if prompted (see [_Could not access microphone_](#could-not-access-microphone) below).
3. **Quit and reopen** the app; sometimes WebKit lags after new aggregate devices appear.
4. Open **Audio MIDI Setup** and confirm an aggregate exists whose name includes **Meeting Note Taker Input** (or your manual aggregate with mic + BlackHole). If routing failed, complete §3 manually, then **Re-check**.

**Why BlackHole does not appear in the list:** The recorder **hides** standalone loopback drivers so you do not record “system only” without your mic. Use **Meeting Note Taker Input** (or another aggregate you created) for mic + system audio.

---

### "Could not access microphone"

macOS denied microphone access, or the app never got a usable TCC entry.

**If you run the app with `pnpm dev` (`tauri dev`):** the process is usually the **debug binary** (`meeting-note-taker`), not the shipped app bundle. **System Settings → Privacy & Security → Microphone** may list **meeting-note-taker** (or a path under `target/debug`) instead of “Meeting Note Taker”. Enable that entry, or use a built app for the normal name (see below).

```bash
# Packaged .app only — resets permission for the bundle id from tauri.conf.json
tccutil reset Microphone com.meeting-note-taker.app

# Dev-friendly: reset microphone prompts for *all* apps (you will be re-prompted elsewhere too)
tccutil reset Microphone
```

Then relaunch the app and click Record.

**Packaged app:** run `pnpm build` and open **Meeting Note Taker** from `src-tauri/target/release/bundle/macos/`. Then Settings shows the real app name and the bundle-id reset above matches.

Alternatively: **System Settings → Privacy & Security → Microphone** and enable the correct process for how you launch the app.

---

### "Failed to run whisper"

Whisper is not on your PATH.

```bash
which whisper          # should print a path
whisper --version      # should print a version

# If missing:
pip install openai-whisper
# or
pip3 install openai-whisper
```

If whisper is installed but the command isn't found, add Python's bin to PATH:

```bash
# Add to ~/.zshrc or ~/.bash_profile
export PATH="$HOME/Library/Python/3.x/bin:$PATH"
```

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

Remove the `--language en` flag from the Whisper command in `src-tauri/src/lib.rs`:

```rust
// Remove this line:
"--language", "en",
```

Whisper will auto-detect the language.

---

### The app freezes during transcription

This is expected — Whisper runs synchronously and blocks the UI on long recordings. Wait for the spinner to disappear. Future versions will run this in a background thread.
