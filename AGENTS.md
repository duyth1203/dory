# Dory agent guide

Dory is a local-first macOS meeting recorder. It records audio, transcribes it on-device, and summarizes it with Gemma through Ollama. The core transcribes in process through sherpa-onnx (`crates/dory-engines`) and the app is native GPUI. Every agent session in this repo, local or cloud, Claude or not, follows this file.

## Drive every task with pstack

Run each task under pstack's poteto-mode.

- In Claude Code, read the newest `~/.claude/plugins/cache/cursor-plugins/pstack/*/skills/poteto-mode/SKILL.md` with the Read tool. The Skill tool cannot load it.
- In Cursor, run `/poteto-mode`.
- If pstack is not installed in this environment, say so in the first reply and still follow the rules below.

Route by task shape. Use `figure-it-out` for large or multi-part work. Use `architect` before code that crosses a module boundary. Use `why` or `how` for investigation, `tdd` for test-first work, `blast-radius` before shipping a risky diff, and `interrogate` for adversarial review of the finished change. Pick only the skills that fit.

Verify finished work on the real app, not by "it compiles". Name what you checked and what you could not check.

## Know where the work is going

The GPUI program is done except for its last steps. Read the plan before you touch its area.

- `docs/plans/gpui-migration.md` built the native GPUI app and the shared `dory-core` crate that replaced the old app.
- `docs/plans/ux-redesign.md` is the UX spec the GPUI screens implement. `docs/plans/claude-design-brief.md` is the matching Claude Design request.
- `docs/plans/speech-model-evaluation.md` is the speech-to-text and Gemma bake-off.
- `docs/plans/decisions.tsv` is the append-only decision log. Add a row for each real decision. Never edit old rows.

Live tests never use the real data folder. Set `DORY_DATA_DIR` to a throwaway folder. A run with `DORY_DATA_DIR` set also skips the one-time legacy audio cleanup, because that cleanup changes system-wide devices.

## Keep the hexagon intact

The new code follows `docs/architecture/hexagonal.md`. Read it before you touch `crates/` or `apps/`.

- `dory-core` holds product rules as data and pure functions. It never depends on a platform crate, an async runtime, an HTTP client, a database, or a codec, and it stays `#![forbid(unsafe_code)]`.
- Platform code lives in its own adapter crate (`dory-macos` first). A new platform is a new crate that implements the same ports.
- Speech-to-text and summary models are `ModelProvider<K>` adapters. Add a model as a catalog row, and an engine as one provider plus one line in the composition root. Never branch on a model or engine name in the core.
- Only the composition root (`crates/dory`) names adapters. Apps use `dory-core` types and `dory::open`.
- Every persisted change goes through `Store::apply`. Every job transition goes through `Job::on`.

## Build and run

- `cargo test --workspace --all-features` and `cargo clippy --workspace --all-features --all-targets -- -D warnings` cover the new crates. Run `cargo fmt` before committing.
- `dory-cli` drives the app headlessly through the `dory` composition root: `import <old-data-dir>`, `list`, `show <id>`, `record`, `process <id> [--fake-engines]`, and `models`. Set `DORY_DATA_DIR` to a throwaway folder for anything you run by hand. The default is `~/Library/Application Support/com.dory.app/v2`, and the old app's folder is refused. The `--fake-*` flags are refused unless `DORY_DATA_DIR` is set.
- One process at a time owns a data folder. `record`, `process`, and `import` lock it, and a second one fails at once with "Dory is already running". `list`, `show`, and `models` only read, so they work while `record` runs.
- `cargo run -p dory-macos --example dory-capture -- --layout mic-system --seconds 10 --out /tmp/take.wav` records through the macOS adapter directly, for capture debugging. `-- devices` lists the input devices. macOS attributes the permission prompts to the app that launched it.
- `cargo run -p dory-gpui` runs the app from a terminal, which macOS attributes the permission prompts to. `scripts/bundle-macos.sh` builds `target/bundle/Dory.app` with bundle id `com.dory.app`, for permission lanes. It signs with `DORY_SIGNING_IDENTITY` when set and ad hoc otherwise, and notarizes only when a real identity and `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` are set. It needs `cargo install cargo-packager --locked`. `apps/dory-gpui/bundle.sh` is the quick dev bundler.
- `crates/dory-macos/src/legacy_cleanup.rs` removes the audio devices the old app left (UIDs `com.dory.aggregate` and `com.dory.multioutput`) once at launch. Its tests use a fake HAL. Never run its destroying path against a real machine in a test or check.
- `dory-engines` links sherpa-onnx statically. Its build script downloads a 20.8 MB prebuilt library from the k2-fsa GitHub release on the first build.
- Do not claim a test run that did not happen.

## Find the code

- `crates/dory-core` holds the domain, ports, recorder state machine, model selection, and the frame ring that capture adapters push into.
- `crates/dory-store` holds the SQLite store, file takes, and legacy import. `crates/dory-macos` holds the macOS 26 adapters. `crates/dory` is the composition root, the only crate that names adapters. `apps/dory-cli` is the headless driving adapter and test harness. The capture harness is `crates/dory-macos/examples/dory-capture`.
- App data lives in `~/Library/Application Support/com.dory.app/v2`. The old app's folder one level up is only read, by the importer and the legacy cleanup.
- `apps/dory-gpui/Info.plist`, `Dory.entitlements`, and `Dory.icns` feed the bundle.

## Git

- Name branches `<type>/<short-kebab-slug>`, with type one of `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `ci`, `style`.
- Do not add AI attribution trailers to commit messages.
- Commit or push only when the user asks.
