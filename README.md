# Dory

Dory is a local-first meeting recorder for macOS. It records your microphone and the other side of the call, transcribes the audio on your Mac, and writes a short summary with a local model. Nothing leaves the machine.

Dory is a native app built on [GPUI](https://www.gpui.rs). It needs **macOS 26** or later.

## What it does

- Records the microphone and system audio in one action. It never changes your default input or output device.
- Transcribes in process with [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). Speech models download into the app's data folder on first run.
- Summarizes with Gemma through [Ollama](https://ollama.com). Summaries are optional. A recording without a summary model still gets a transcript.
- Shows speaker-labelled transcripts and a summary you can copy or export.

See the [user guide](docs/USER_GUIDE.md) for a walk through the app.

## Install

1. Build the app, as below, or open a `Dory.app` someone gives you.
2. Drag `Dory.app` to Applications and open it.
3. Follow the three onboarding steps. macOS asks for **Microphone** and **System Audio Recording**.

An ad-hoc signed build loses its permission grants every time it is rebuilt, because macOS ties a grant to the code signature. Any stable signing identity keeps them, a self-signed one included. Name it with `DORY_SIGNING_IDENTITY`, or once per clone with `git config dory.signingIdentity "<identity>"`.

### Upgrading from the Tauri app

Earlier versions of Dory were a Tauri app that used BlackHole and Sona. This version keeps the same bundle id, `com.dory.app`, and imports the old recordings from the old data folder. The old WebM files stay where they are until you move them to the Trash in **Settings > Storage**. macOS asks for the microphone again and, for the first time, for System Audio Recording. On first launch Dory removes the "Dory Input" and "Dory Output" devices the old app created and puts your default devices back. You can uninstall BlackHole.

## Build from source

You need macOS 26, Xcode command line tools, and the Rust toolchain in `rust-toolchain.toml`.

```bash
git clone <this repo> dory && cd dory
cargo run -p dory-gpui
```

The first build downloads a 20.8 MB prebuilt sherpa-onnx library from the k2-fsa GitHub release and links it statically.

To build a release `Dory.app`:

```bash
scripts/bundle-macos.sh
```

The script writes `target/bundle/Dory.app` and signs it with `DORY_SIGNING_IDENTITY`, or else the identity in `git config dory.signingIdentity`, with the hardened runtime. With neither, the app is signed ad hoc. With a Developer ID identity and the `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` variables set, cargo-packager also notarizes the app. The script needs `cargo install cargo-packager --locked` once.

`DORY_DATA_DIR` moves the data folder. The default is `~/Library/Application Support/com.dory.app/v2`. Set it to a throwaway folder for anything you try by hand.

## Develop

```bash
cargo test --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo fmt
```

`apps/dory-cli` drives the same core without a window:

```bash
export DORY_DATA_DIR=/tmp/dory-scratch
cargo run -p dory-cli -- record --layout mic-system
cargo run -p dory-cli -- list
cargo run -p dory-cli -- models
```

The code follows a ports-and-adapters design. Read [docs/architecture/hexagonal.md](docs/architecture/hexagonal.md) before you touch `crates/` or `apps/`. `AGENTS.md` has the repo rules for agents and contributors.

| Path | Holds |
| --- | --- |
| `crates/dory-core` | Domain, ports, recorder state machine, model selection |
| `crates/dory-store` | SQLite store, file takes, legacy import |
| `crates/dory-engines` | Speech and summary engines (sherpa-onnx, Ollama) |
| `crates/dory-macos` | macOS 26 capture, playback, Trash, and legacy audio cleanup |
| `crates/dory` | Composition root, the only crate that names adapters |
| `apps/dory-gpui` | The native app |
| `apps/dory-cli` | Headless driver and test harness |

## Known limits

- One recording is processed at a time. Others wait in a queue.
- A recording stops at two hours.
- Summaries need Ollama running locally with a Gemma model.
