# Dory agent guide

Dory is a local-first macOS meeting recorder. It records audio, transcribes it on-device with the bundled Sona sidecar, and summarizes it with Gemma through Ollama. Every agent session in this repo, local or cloud, Claude or not, follows this file.

## Drive every task with pstack

Run each task under pstack's poteto-mode.

- In Claude Code, read the newest `~/.claude/plugins/cache/cursor-plugins/pstack/*/skills/poteto-mode/SKILL.md` with the Read tool. The Skill tool cannot load it.
- In Cursor, run `/poteto-mode`.
- If pstack is not installed in this environment, say so in the first reply and still follow the rules below.

Route by task shape. Use `figure-it-out` for large or multi-part work. Use `architect` before code that crosses a module boundary. Use `why` or `how` for investigation, `tdd` for test-first work, `blast-radius` before shipping a risky diff, and `interrogate` for adversarial review of the finished change. Pick only the skills that fit.

Verify finished work on the real app, not by "it compiles". Name what you checked and what you could not check.

## Know where the work is going

Three programs are planned. Read the plan before you touch its area.

- `docs/plans/gpui-migration.md` replaces Tauri and React with a native GPUI app and a shared `dory-core` crate.
- `docs/plans/ux-redesign.md` is the UX spec the GPUI screens implement. `docs/plans/claude-design-brief.md` is the matching Claude Design request.
- `docs/plans/speech-model-evaluation.md` is the speech-to-text and Gemma bake-off.
- `docs/plans/decisions.tsv` is the append-only decision log. Add a row for each real decision. Never edit old rows.

Until the GPUI program lands, do not add features to the React UI in `src/`. Bug fixes and accessibility fixes there are fine.

Live tests never use the real data folder. Set `DORY_DATA_DIR` to a throwaway folder once the GPUI plan's P1 adds it.

## Keep the hexagon intact

The new code follows `docs/architecture/hexagonal.md`. Read it before you touch `crates/` or `apps/`.

- `dory-core` holds product rules as data and pure functions. It never depends on a platform crate, an async runtime, an HTTP client, a database, or a codec, and it stays `#![forbid(unsafe_code)]`.
- Platform code lives in its own adapter crate (`dory-macos` first). A new platform is a new crate that implements the same ports.
- Speech-to-text and summary models are `ModelProvider<K>` adapters. Add a model as a catalog row, and an engine as one provider plus one line in the composition root. Never branch on a model or engine name in the core.
- Only the composition root (`crates/dory`) names adapters. Apps use `dory-core` types and `dory::open`.
- Every persisted change goes through `Store::apply`. Every job transition goes through `Job::on`.

## Build and run

- `cargo test --workspace --all-features` and `cargo clippy --workspace --all-features --all-targets -- -D warnings` cover the new crates. Run `cargo fmt` before committing.
- `cargo run -p dory-cli -- capture --layout mic-system --seconds 10 --out /tmp/take.wav` records through the macOS adapter. macOS attributes the permission prompts to the app that launched it.
- `pnpm prebuild:sona` downloads the Sona sidecars into `src-tauri/binaries/`. Run it once before the first Tauri build.
- `pnpm install`, then `pnpm dev`, runs the Tauri app. `src-tauri/` is outside the Cargo workspace until the GPUI app replaces it.
- `pnpm vite:build` is the only typecheck for the React side.
- Do not claim a test run that did not happen.

## Find the code

- `crates/dory-core` holds the domain, ports, recorder state machine, model selection, and the frame ring that capture adapters push into.
- `crates/dory-macos` holds the macOS 26 adapters. `apps/dory-cli` is the headless driving adapter and test harness.
- `src-tauri/src/lib.rs` holds the old commands, the pipeline queue (`PipelineCoordinator`, `pipeline_worker_loop`), and the BlackHole routing module `audio_setup`.
- `src-tauri/src/sona.rs` runs the Sona HTTP sidecar. `sona_download.rs` and `sona_paths.rs` locate or fetch it.
- `src/` is the React UI. Audio capture lives in `src/hooks/useAudioRecorder.ts`.
- App data lives in `~/Library/Application Support/com.dory.app/`. Each recording has `recordings/<id>.webm` or `.wav` and `metadata/<id>.json`.

## Git

- Name branches `<type>/<short-kebab-slug>`, with type one of `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `ci`, `style`.
- Do not add AI attribution trailers to commit messages.
- Commit or push only when the user asks.
