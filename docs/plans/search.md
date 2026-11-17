# Search plan

Dory's sidebar search becomes real search for people who look up a past meeting by a word they remember. Today it finds one contiguous lowercase substring, so "muller" misses "Müller", "budget review" misses "review of the budget", one typo finds nothing, and a title hit looks the same as one mention in a 2-hour transcript.
After this program, every word you type matches the start of a word, in any order, with accents and case folded. A word that matches nothing is corrected when it is one typo away. Results come in two sections, titles first. A conversation hit shows a snippet with the matched words marked, and opening it jumps to the first matching turn.
The rule the program enforces is that `dory-core` owns every search rule as pure functions, and the SQLite adapter only indexes and looks up. One tokenizer in the core decides what a word is for the index, the snippet, and the highlight, so a hit always has something to highlight.
PRs S1, S2, S3, and S4 land in that order. S1 and S2 change the store and the core. S3 and S4 change the sidebar and the detail pane.

## How to read this

One box is one unit of work. Every box names the evidence that checks it. A nested box is a sub-step of the box above it. Check a box only when its evidence exists, a file, a log line, a screenshot, a test run, or a SHA. The body is a how-to. The appendices explain and record.

The program runs `pstack/skills/poteto-mode/playbooks/autopilot-stack.md`. The root agent builds one linear stack of PRs from S1 to S4, and the operator reviews and lands it. S3 and S4 are the operator's review items and stop at merge-ready.

Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

Requirement ids such as `UX-1` and `A11Y-4` point into `docs/plans/ux-redesign.md`. Appendix B records the designs that lost and Appendix E records the operator's answers to the open questions.

A lane marked **Operator lane** needs a person at the Mac, because it runs with VoiceOver on or changes System Settings. The root prepares the exact steps and the evidence to capture, and the operator runs it. Every other lane is agent-runnable.

pstack is a Claude Code plugin, not part of this repo. Paths that start with `pstack/` mean `~/.claude/plugins/cache/cursor-plugins/pstack/<version>/`. Validate this file with `node <pstack>/skills/poteto-mode/scripts/check-plan.mjs docs/plans/search.md`.

## Program checklist

### Arm the program

- [ ] State the protocol and this plan to the operator, then stop. Start execution only on the operator's explicit go.
- [x] Get the operator's answers to the open questions in Appendix E. The operator answered on 2026-11-17, and each answer has a row in `docs/plans/decisions.tsv`.
- [ ] Read these from trunk at program start. Re-read them at every tick.
  - [ ] `git show origin/main:docs/plans/search.md`
  - [ ] `git show origin/main:docs/plans/ux-redesign.md`
  - [ ] `git show origin/main:docs/architecture/hexagonal.md`
  - [ ] `git show origin/main:AGENTS.md`
  - [ ] The pstack copies of `poteto-mode/playbooks/autopilot-stack.md`, `poteto-mode/playbooks/opening-a-pr.md`, `swarm/SKILL.md`, and `tdd/SKILL.md`. pstack lives outside the repo, so `git show origin/main:` applies only to the repo files above.
- [ ] Tag S1's first commit as `baseline-search` once it exists. That commit adds only `dory-cli seed` and the timing log lines (see S1 **Build**), so it searches like `main`. Every "trunk" measurement in this plan runs on that tag, so the baseline does not move while the stack is open.
- [ ] On the operator's go, arm the audit tick as `/loop 1h` with the tick prompt below. Never leave the cadence to memory.
- [ ] Use this tick prompt, verbatim. "Re-read the execution playbook from trunk. Audit the operation against it and fix drift in this tick. Probe every active lane and judge progress by side effects only. Stand down a stuck lane and dispatch its replacement now. Then post a short status message to the operator in chat only when the audit found a tracked change that no earlier status message reported, such as a PR opened, a code-ready head, a round launched or closed, a verdict, a merge, a stuck agent and the action taken, a blocker added or cleared, or a decision only the operator can make. Name every such change and nothing else. Do not repeat a table, the merged list, or an unchanged blocker. If the audit found none, end the turn with no reply text. Either way, log this tick's row in your decision trail. The row names the items reported, or none."
- [ ] On the operator's hold or stand-down, send every owner a zero-writes order at once.

### Spawn owners

- [ ] Spawn one owner per PR with the full lifecycle the execution playbook names. Owners use `pstack:poteto-agent` on the `feature, refactoring` model from `~/.claude/rules/pstack-models.md`. S1 uses the `hardest tasks` model, because it changes the store port, the schema, and the migration together.
- [ ] Follow this dependency graph. Each owner branches from its parent's branch, because the stack is linear.
  - [ ] S1 is first. It branches from `main`.
  - [ ] S2 after S1.
  - [ ] S3 after S2. S3 shows the correction notice that S2 returns.
  - [ ] S4 after S3. S4 reads the query that S3 keeps in `AppState`.
- [ ] Hold the file boundaries. Each PR's **Files** list is its boundary. A PR that needs a file outside its list stops and asks the root, which either adds a small PR below it in the stack or amends this plan with a logged decision.
- [ ] Hold the review gate. S3 and S4 change an interaction. They wait for the operator's review in chat with screenshots and a video before merge.

### PR mechanics, for every PR

- [ ] Resolve the forge once. Default to `gh`; if `command -v origin` succeeds and Origin can resolve the repository, use `origin pr` for every PR operation. Record any fallback to `gh`. Never require `gt`.
- [ ] Open the PR ready, never draft, per **Opening a PR**. Use the run's built-in PR tool when it has one, else `origin pr create --status open --base <base-branch>` or `gh pr create --base <base-branch>` according to the resolved forge. A stack child targets its parent branch.
- [ ] Run `cargo fmt --check`, `cargo clippy --workspace --all-features --all-targets -- -D warnings`, and `cargo test --workspace --all-features` once before the PR-facing push. Push with hooks on.
- [ ] Run `/deslop` before each commit and `/no-comments` before review. Both come from `cursor-team-kit`. If the plugin is missing, write `Skipped in Claude Code` with the reason in the PR body.
- [ ] Triage every Bugbot and security-reviewer comment per `pstack/skills/poteto-mode/references/bugbot-triage.md`.
- [ ] Rebase onto current trunk before the code-ready report and babysit. Keep that merge base in fix rounds. Rebase again only at merge prep, on a `git merge-tree` conflict with trunk, or on a CI failure that comes from a change on trunk.
- [ ] Name branches `<type>/<slug>` per `AGENTS.md`. S1 uses `feat/search-index`, S2 uses `feat/search-typos`, S3 uses `feat/search-results`, and S4 uses `feat/search-jump`. No AI trailer in commit messages.
- [ ] Append a row to `docs/plans/decisions.tsv` for every real decision with the pstack `show-me-your-work/scripts/log.sh` helper. Mark any external fact you did not re-check this session as unverified in the row.

### Verdict and merge, for every PR

- [ ] A full swarm runs once, at the code-ready head. Another full swarm runs only for new behavior, a redesign, or a large new diff. A rebase, a merge-conflict resolution, or a minor fix gets a single verifier at the new head. Run each full swarm per `pstack/skills/swarm/SKILL.md`. One gates lane. The ten live lanes from the PR's **Verify, live** block. The perf lane from its **Verify, perf** block. Two or more audit lanes, each with its own focus, that read the diff and the receipts and distrust the PR body. The root audits the receipts in the merge-ready report before the verdict.
- [ ] Clean only when every lane is `PASS`. An Operator lane that has not run is `NOT VERIFIED`, never `PASS`. Findings go back to the owner, including a defect that a lane filed as a note. A new head gets a fresh verdict, except for results that stay valid under the patch-id rule in `playbooks/shipping.md`.
- [ ] The root appends each clean PR to the stack with its base set to its parent's branch. The operator lands the stack bottom-up. A rebase that leaves `git patch-id` unchanged keeps the verdict. A changed patch-id needs a single verifier at the new head.

### Boot recipe, for every live lane

Live lanes run on the operator's Mac, an Apple Silicon machine on macOS 26 or later. Search lanes need no microphone, so several lanes can run at once, each on its own data folder. No lane uses the real data folder.

- [ ] `git fetch origin <head-branch> && git checkout <head SHA>`. Build into a lane-owned target folder with `CARGO_TARGET_DIR=/tmp/dory-target-<pr-id>`, so lanes do not race other agents in the shared `target/`. Check `df -h /` first and keep at least 10 GB free.
- [ ] Seed a throwaway data folder. From `baseline-search` on, run `DORY_DATA_DIR=/tmp/dory-search-<n> cargo run --release -p dory-cli -- seed --recordings <count> --words <words>`. The `seed` command writes deterministic recordings through `Store::apply` and plants the fixed words that **Appendix A** lists at known positions. It is refused unless `DORY_DATA_DIR` is set.
- [ ] For a trunk lane, build `baseline-search` into `/tmp/dory-target-trunk` and seed with that binary's `seed`, which writes schema v5. To compare one corpus at trunk and head, seed at trunk, copy the folder with `cp -cR`, and open the copy at head, which migrates it to v6.
- [ ] Run the CLI with `cargo run --release -p dory-cli -- <command>`. Start the app with `DORY_DATA_DIR=/tmp/dory-search-<n> cargo run --release -p dory-gpui` and wait for the `first-frame` log line.
- [ ] Deliver input to the app only through the Claude Code computer-use tools (`app_type`, `app_key`, `app_click`, `app_ax_find`, `app_screenshot`), because no `control-ui` skill drives a native macOS window. Capture Dory's own window with `screencapture -l <window id>`, never the full screen.
- [ ] Read-only diagnostics are `log stream --predicate 'subsystem == "com.dory.app"'`, `sqlite3 -readonly /tmp/dory-search-<n>/dory.sqlite`, and the `search ms=`, `search-settled ms=`, `match-revealed ms=`, and `search-backfill ms=` log lines.
- [ ] Save every screenshot to `/tmp/swarm-<pr-id>/worker-<n>/<slug>.png` and return the paths with the report.

## Match words in any order, with prefixes and folded accents (S1)

**Depends on.** None.

**Files.**

- [ ] Create `crates/dory-core/src/search/mod.rs`, `tokens.rs`, `query.rs`, and `matcher.rs`.
- [ ] Edit `crates/dory-core/src/lib.rs`, `crates/dory-core/Cargo.toml`, and the root `Cargo.toml` to add the `search` module and the `unicode-normalization` workspace dependency.
- [ ] Edit `crates/dory-core/src/ports/store.rs`, `crates/dory-core/src/domain/recording.rs`, `crates/dory-core/src/app/mod.rs`, and `crates/dory-core/src/testing/store.rs`.
- [ ] Edit `crates/dory/src/lib.rs` to add `dory::with_store`, which `dory-cli seed` writes through, because the CLI names no adapter and the facade has no public `apply` (2026-11-17, logged).
- [ ] Edit `crates/dory-store/src/sqlite/mod.rs` and create `crates/dory-store/src/sqlite/schema_v6.sql` and `crates/dory-store/src/sqlite/search.rs`. Trunk added its own `schema_v5.sql` (the job `place` column) after this plan was written, so the search schema is v6 (2026-11-17, logged).
- [ ] Edit `crates/dory-store/tests/store_contract.rs` and `crates/dory-core/tests/facade.rs`.
- [ ] Edit `apps/dory-cli/src/args.rs`, `apps/dory-cli/src/library.rs`, and `apps/dory-cli/src/main.rs`, and create `apps/dory-cli/src/seed.rs`.
- [ ] Edit `apps/dory-gpui/src/library.rs` and `apps/dory-gpui/src/app.rs` only to call the new search, to delete `SearchQuery`, and to add the timing log lines. Edit `apps/dory-gpui/src/detail.rs` only for the `match-revealed` log line.
- [ ] Edit `docs/architecture/hexagonal.md` to document `StoreRead::search` and the index.

**Build.**

- [ ] Make the first commit instrumentation only. Add `dory-cli seed` (described below), log `search ms=<n> hits=<n>` from `Library::list` when the query has text, log `search-settled ms=<n>` from the debounce firing to the first paint of the new rows, and log `match-revealed ms=<n>` from a selection to the first paint of the detail pane. The root tags this commit `baseline-search`.
- [ ] Name the data shapes first, in `dory_core::search`.
  - [ ] `pub fn tokens(text: &str) -> impl Iterator<Item = Token>`. A `Token` is `{ folded: String, span: Range<usize> }`, with `span` in bytes of the original text. A token is a run of alphanumeric characters. Each Han, Hiragana, and Katakana character is a token of its own, because those scripts have no spaces. A run that holds such characters becomes a phrase when it is typed in a query.
  - [ ] `fold` lowercases, applies NFKD, drops combining marks, and maps the letters that NFKD leaves alone with one table. The table maps `ß` to `ss`, `ø` to `o`, `ł` to `l`, `æ` to `ae`, `œ` to `oe`, `đ` to `d`, and `þ` to `th`. "muller" then finds "Müller", "strasse" finds "Straße", and "orsted" finds "Ørsted". This rule has one definition, and every store and view calls it.
  - [ ] `Query`, which is opaque outside the core. `Query::parse(text: &str) -> Option<Query>` returns `None` for blank text. A query is a list of clauses joined by AND. `Clause::Word { alts: Vec<String>, prefix: bool }` matches the start of any word and is what an unquoted word becomes. `Clause::Phrase(Vec<String>)` matches whole words in order and is what a quoted run becomes. Typed text that splits into several tokens, such as "e-mail", "don't", or "Q3.5", becomes a phrase. An unbalanced quote runs to the end of the text.
  - [ ] `Query::clauses(&self) -> &[Clause]` is the only read access, for store adapters. Search always covers the title, speaker names, summary, and transcript. There is no title-only scope.
  - [ ] `pub fn highlights(query: &Query, text: &str) -> Vec<Range<usize>>`, `pub fn locate(query: &Query, items: &[&str]) -> Option<Located>`, and `pub fn snippet(query: &Query, text: &str, width: usize) -> Option<Snippet>`. `Located` is `{ item: usize, spans: Vec<Range<usize>> }`. `Snippet` is `{ text: String, marks: Vec<Range<usize>> }`, a window of about `width` characters that starts a few words before the first match and has an ellipsis at each cut. When one item holds every clause, `locate` picks the first such item. Otherwise it picks the first item that holds any clause.
- [ ] Change the port. `RecordingQuery` loses `text` and keeps `limit`. Add `fn search(&self, query: &Query) -> Result<Vec<Found>, StoreError>` to `StoreRead`. `Found` is `{ row: RecordingRow, in_title: bool }`, where `in_title` is true when the title alone matches every clause. The store returns hits newest first. It does not rank.
- [ ] Add `Library::search(&self, text: &str) -> Result<Option<SearchResults>, StoreError>` to the facade, `None` when the text holds no word (2026-11-17, logged). `SearchResults` is `{ query: Query, hits: Vec<Found> }`. S2 adds corrections and S3 adds sections. Remove `SearchQuery::to_query` from `apps/dory-gpui/src/library.rs`. `SearchQuery` itself stays as the trimmed typed text in S1, because `sidebar.rs` and `shell.rs` read it and S1 does not touch them. S3, which owns `sidebar.rs`, replaces it with `search: Option<SearchResults>` (2026-11-17, logged). The app calls `Library::search` when the field has text and `Library::list` when it is empty, and keeps the 150 ms `SEARCH_DEBOUNCE` and the generation guard in `AppState::set_search`. The sidebar keeps its date groups in S1.
- [ ] `MemStore` implements `search` with `tokens` and `highlights` over its entries. It is the reference implementation in the contract suite.
- [ ] Write `schema_v6.sql` and the index in `crates/dory-store/src/sqlite/search.rs`.
  - [ ] `CREATE TABLE search_keys (key INTEGER PRIMARY KEY, recording_id TEXT NOT NULL UNIQUE REFERENCES recordings (id) ON DELETE CASCADE)`. The FTS rowid is `search_keys.key`. `recordings` has a TEXT primary key, so VACUUM may renumber its implicit rowid.
  - [ ] `CREATE VIRTUAL TABLE search USING fts5(title, speakers, summary, transcript, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 0', prefix='1 2 3')`. The S1 owner measured `prefix='2 3'` at 15 ms for `a*` against 0.1 ms with the one-letter prefix index, for 9.7 MB more index on the 500 by 2-hour corpus (2026-11-17, logged). The adapter inserts the core's tokens joined by spaces, never the raw text. FTS5 then only splits on spaces, so the index and the core agree on every word by construction.
  - [ ] `CREATE TABLE search_meta (tokens_version INTEGER NOT NULL, generation INTEGER NOT NULL)`. `tokens_version` is a constant in `dory_core::search`. A core change to `tokens` or `fold` bumps it, and `SqliteStore::open` then rebuilds every row.
  - [ ] Drop `title_lc`, `transcript_lc`, and `summary_lc` from `recordings`.
- [ ] Let `MIGRATIONS` hold a Rust step next to SQL. v6 runs its SQL, then calls `reindex` for every recording inside the same immediate transaction. A crash before commit leaves v5, and the next open runs v6 again.
- [ ] Write one function, `reindex(tx, id)`, that deletes the recording's FTS row and inserts it again. It reads the title, the speaker names for the current transcript revision, the summary Markdown, and the segment texts in `idx` order, then bumps `search_meta.generation`. Call it from `apply_change`, not from `apply`, because the legacy importer calls `apply_change` directly (`crates/dory-store/src/legacy.rs`). The arms that call it are `RecordingCreated`, which inserts the key first, `RecordingEdited { Title }`, `TranscriptReplaced`, `SpeakerNamesReplaced`, and `SummaryReplaced`. `RecordingDeleted` deletes the FTS row before the recording row. Job, settings, and checkpoint arms never touch the index.
- [ ] Compile a `Query` to one FTS5 `MATCH` string in the adapter. Double-quote every token, so a word such as "or" or "near" never becomes FTS syntax. A word becomes `("tok"*)`, a phrase becomes `"tok1 tok2"`, and clauses join with `AND`. Run a second `{title} :` query to set `in_title`. Join the rows with the same SQL that `recordings` uses, ordered by `created_at DESC, id ASC`.
- [ ] Move the `search ms=<n> hits=<n>` log line to `Library::search`. Log `search-backfill ms=<n> recordings=<n>` when v6 or a `tokens_version` rebuild finishes.
- [ ] Add `dory-cli search <text>`, which prints each hit's id, title, date, and whether it matched in the title, then `search ms=<n>`. Delete `list --search`. The first commit adds `dory-cli seed --recordings <n> --words <n> [--seed <n>]`, which writes synthetic recordings with titles, transcripts, summaries, and speaker names through `Store::apply`. Its word list is `/usr/share/dict/words` with Zipf frequencies plus the planted words in **Appendix A**. `seed` is refused unless `DORY_DATA_DIR` is set, like the `--fake-*` flags.

**You see.**

- [ ] `dory-cli search "review budget"` on a seeded folder lists the recordings that contain both words in any order, and `search ms=` stays under 10 for a word of three letters or more.
- [ ] In the app, typing "mull" into the sidebar search field shows the recordings that mention "Müller", grouped by date as before.
- [ ] Opening a v5 library at head runs v6 once, logs the backfill time, and finds the same recordings for a whole word that trunk finds.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/src/search/tokens.rs` tests a table of inputs and their tokens and spans, with apostrophes, digits, hyphens, emoji, `Müller`, `Straße`, `Ørsted`, the `ﬁ` ligature, and a Japanese run. Run `cargo test -p dory-core search::tokens`.
- [ ] `crates/dory-core/src/search/query.rs` tests a table of typed text and its clauses, with blank text, quotes, an unbalanced quote, "e-mail", and a mixed Japanese and English query. Run `cargo test -p dory-core search::query`.
- [ ] `crates/dory-core/src/search/matcher.rs` tests `highlights`, `locate`, and `snippet`, including a phrase split across two turns, which falls back to the first item with any clause. Run `cargo test -p dory-core search::matcher`.
- [ ] The store contract suite in `crates/dory-core/src/testing/store.rs` gains `search_words_any_order_and_prefix`, `search_folds_accents_both_ways`, `search_phrase_needs_order`, `search_sets_in_title`, `search_follows_rename_retranscribe_names_and_summary`, `search_forgets_deleted_and_finds_restored`, and `search_quotes_fts_keywords`. Rewrite `list_order_limit_and_filter` without `text`. Run `cargo test -p dory-store --test store_contract` and `cargo test -p dory-core --all-features`.
- [ ] `crates/dory-store/tests/store_contract.rs` gains `v6_migrates_a_v5_file`, which builds a v5 file with the v1 to v5 SQL, writes three recordings, opens it at head, and checks `user_version` is 6, the `_lc` columns are gone, and search finds each recording. It also gains `tokens_version_bump_reindexes`. Run `cargo test -p dory-store`.
- [ ] `crates/dory-store/tests/legacy.rs` gains a case that imports a legacy folder and searches a word from its transcript. Run `cargo test -p dory-store --test legacy`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Seed 50 recordings, then search the planted whole word "kilimanjaro" with trunk `dory-cli list --search kilimanjaro` and head `dory-cli search kilimanjaro`. Save `regress-cli.png`. Pass when both print the same ids.
- [ ] Lane 2. In the app at head, type "budget review" and then "review budget". Save `any-order.png`. Pass when both show the same rows, and the rows hold both words at the planted positions.
- [ ] Lane 3. Type "kilim" one letter at a time with `app_type`. Save `prefix.png`. Pass when the row with "kilimanjaro" is listed once "kil" is typed, and `search ms=` logs one line per settled query, not one per key.
- [ ] Lane 4. Search "muller", then "Müller", then "MULLER". Save `accents.png`. Pass when all three show the same rows, which include the planted "Müller" and "Muller" recordings.
- [ ] Lane 5. Search `"budget review"` with the quotes. Save `phrase.png`. Pass when only the recording with the adjacent planted pair is listed, and the one with "review of the budget" is not.
- [ ] Lane 6. Rename a recording to "Atlas kickoff" in the app, then search "atlas". Save `rename.png`. Pass when the renamed row is listed within one debounce, without a relaunch.
- [ ] Lane 7. Rename "Speaker 2" to "Priyanka" in a recording, then search "priyanka". Save `speaker.png`. Pass when that recording is listed, because speaker names are now searchable.
- [ ] Lane 8. Run `dory-cli process <id> --fake-engines` on a seeded recording whose planted word the fake transcript lacks, then search that word. Save `retranscribe.png`. Pass when the recording is no longer listed for the old word and is listed for a word in the new transcript.
- [ ] Lane 9. Move a listed recording to the Trash, search, then press Command-Z and search again. Save `trash-undo.png`. Pass when it disappears from the results and returns after the undo.
- [ ] Lane 10. Copy `~/Library/Application Support/dory-dev/live-data` with `cp -cR` to `/tmp/dory-search-10` and remove `.lock`, then open it in the head app. Separately, seed 500 recordings of 18,000 words with `baseline-search`, open the folder in the head app, `kill -9` the app before `search-backfill ms=` is logged, and launch it again. Save `migrate.png`. Pass when the live-data copy reports `user_version` 6 and finds a word from each of its transcripts, and the killed folder finishes v6 on the second launch with `search_keys` holding one row per recording.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. Search time for the queries "budget", "kilimanjaro", "review budget", `"budget review"`, "ro", and "a", taken from `search ms=` at head and from `dory-cli list --search` timing at trunk. Also the v6 backfill time, the time of one `TranscriptReplaced` apply for a 2-hour transcript, and the database size after `VACUUM INTO`.
- [ ] Probe. Seed 500 recordings of 18,000 words each, which is every recording at the 2-hour cap. Run each query seven times at trunk and seven times at head, alternating, and take the median and the maximum.
- [ ] Baseline. Record the trunk numbers first. The spike measured 41 ms for a hit and 68 ms for a miss with trunk's `instr` scan on this corpus.
- [ ] Rule. Fail when any head median for a word of two letters or more exceeds 10 ms, when "a" exceeds 50 ms, when any head median exceeds the trunk median for the same query, when the backfill exceeds 5 s, when one `TranscriptReplaced` apply grows by more than 15 ms over trunk, or when the head file is larger than the trunk file.

**Review gate.** None. S1 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends S1 to the stack on `main`.

## Correct a typo when the exact search finds nothing (S2)

**Depends on.** S1.

**Files.**

- [ ] Create `crates/dory-core/src/search/vocabulary.rs`.
- [ ] Edit `crates/dory-core/src/search/mod.rs`, `crates/dory-core/src/search/query.rs`, `crates/dory-core/src/ports/store.rs`, `crates/dory-core/src/app/mod.rs`, and `crates/dory-core/src/testing/store.rs`.
- [ ] Edit `crates/dory-store/src/sqlite/search.rs` and `crates/dory-store/src/sqlite/mod.rs`.
- [ ] Edit `crates/dory-store/tests/store_contract.rs` and `apps/dory-cli/src/library.rs`.

**Build.**

- [ ] Name the data shapes first. `Vocabulary` holds the sorted, folded terms of four letters or more and the store generation it was built from. `Correction` is `{ typed: String, used: Vec<String> }`. `SearchResults` gains `corrections: Vec<Correction>`.
- [ ] Write the rule as one pure function, `Query::corrected(&self, vocabulary: &Vocabulary) -> Option<(Query, Vec<Correction>)>`. For each `Clause::Word` of five letters or more whose text starts no term in the vocabulary, it adds every term within one edit, or within two edits for nine letters or more. An edit is one letter added, dropped, or changed, or two neighbouring letters swapped, which is the optimal string alignment distance. Phrases and shorter words are never corrected. It returns `None` when no word changed.
- [ ] `Library::search` runs the exact query first. Only when it finds nothing does it ask the store for the vocabulary, correct the query, and search once more. The user-facing rule is "When nothing matches what you typed, Dory looks for words one typo away from each long word that appears nowhere in your recordings." A search that finds something is never corrected.
- [ ] Add `fn vocabulary(&self) -> Result<Arc<Vocabulary>, StoreError>` to `StoreRead`. `SqliteStore` builds it from an `fts5vocab(search, 'row')` table and caches it in `Mutex<Option<Arc<Vocabulary>>>`. It rebuilds when `search_meta.generation` differs from the cached value, so any text write since the last build invalidates it. `MemStore` builds it on every call.
- [ ] `dory-cli search` prints each correction as `showing results for <used> instead of <typed>`.

**You see.**

- [ ] `dory-cli search roadmpa` prints `showing results for roadmap instead of roadmpa` and lists the recordings with "roadmap".
- [ ] `dory-cli search roadmap` prints no correction, even though "roadmaps" is also one edit away.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/src/search/vocabulary.rs` tests a table of distances, with a swap, a drop, an insert, a change, a two-edit case at nine letters, and a four-letter word that is never corrected. Run `cargo test -p dory-core search::vocabulary`.
- [ ] `crates/dory-core/src/search/query.rs` tests `corrected` with a vocabulary fixture. A word that starts a vocabulary term stays exact, a phrase is never corrected, and a query with one misspelt word and one good word corrects only the misspelt one. Run `cargo test -p dory-core search::query`.
- [ ] The store contract suite gains `search_corrects_only_when_nothing_matches` and `vocabulary_follows_writes`. Run `cargo test -p dory-store --test store_contract` and `cargo test -p dory-core --all-features`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Search "roadmpa" with trunk `dory-cli list --search` and head `dory-cli search`. Save `regress-typo.png`. Trunk lacks typo correction, so record its empty result and gate head. Pass when head lists the "roadmap" recordings and prints the correction.
- [ ] Lane 2. Search "budgte" in the app. Save `swap.png`. Pass when the "budget" recordings are listed.
- [ ] Lane 3. Search "kilimanjro", one dropped letter. Save `drop.png`. Pass when the "kilimanjaro" recording is listed.
- [ ] Lane 4. Search "antinatoinal", nine letters or more with two edits. Save `two-edits.png`. Pass when the "antinational" recordings are listed.
- [ ] Lane 5. Search "atlsa", five letters with one swap. Save `five-letters.png`. Pass when "atlas" recordings are listed. Then search "atls", four letters. Pass when nothing is listed.
- [ ] Lane 6. Search "roadmap". Save `exact-wins.png`. Pass when the CLI prints no correction and the app lists exactly the recordings that contain a word starting with "roadmap".
- [ ] Lane 7. Search `"budgte review"` with the quotes. Save `phrase-exact.png`. Pass when nothing is listed, because phrases are never corrected.
- [ ] Lane 8. Search "budgte kilimanjaro". Save `mixed.png`. Pass when only recordings with both "budget" and "kilimanjaro" are listed and the correction names only "budgte".
- [ ] Lane 9. With the app open, search "zanzibra" and see no results. Then rename a recording to "Zanzibar offsite" and search "zanzibra" again. Save `fresh-word.png`. Pass when the renamed recording is listed, which shows that the vocabulary rebuilt after the write.
- [ ] Lane 10. Search a nonsense word "qqqqqqq" in the app. Save `no-match.png`. Pass when the list is empty, the app stays responsive, and `search ms=` is logged.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. `search ms=` for a corrected query with a cold vocabulary (first search after a write) and with a warm one, and `search ms=` for an exact query, at trunk and head. Trunk has no correction, so trunk gives only the exact numbers.
- [ ] Probe. On the 500 by 2-hour seeded folder, alternate trunk and head seven times each for "budget" and "budgte". For the cold case, rename one recording before each head run.
- [ ] Baseline. Record the trunk "budget" and "budgte" medians first.
- [ ] Rule. Fail when an exact query at head is slower than its S1 number by more than 1 ms, when a warm corrected query exceeds 30 ms, or when a cold corrected query exceeds 100 ms. The spike measured 64 ms to read the vocabulary and 2.5 to 10 ms to scan 40,000 terms, so the cold case is the one search that may exceed the 50 ms target.

**Review gate.** None. S2 is not review-gated.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends S2 to the stack on S1's branch.

## Show title matches first, with a snippet for conversation matches (S3)

**Depends on.** S2.

**Files.**

- [ ] Edit `crates/dory-core/src/search/mod.rs` and `crates/dory-core/src/app/mod.rs`, and create `crates/dory-core/src/search/excerpt.rs`, which holds the snippet-source rules.
- [ ] Edit `apps/dory-gpui/src/sidebar.rs`, `apps/dory-gpui/src/library.rs`, `apps/dory-gpui/src/app.rs`, and `apps/dory-gpui/src/theme.rs`. With no scope control, `menus.rs` and `actions.rs` stay unchanged.
- [ ] Create `apps/dory-gpui/tests/search.rs`.
- [ ] Edit `docs/plans/ux-redesign.md` to rewrite UX-1's search sentence and add the search rules as UX-32.

**Build.**

- [ ] Name the data shapes first. `Section { Titles, Conversations }`. `SearchResults` gains `sections() -> [(Section, Vec<&Found>); 2]`, built by one pure function, `search::sections(hits)`. A hit with `in_title` goes to Titles. Every other hit goes to Conversations. Each section is newest first. `AppState` holds `typed: SearchText` (the trimmed field text), `search: Option<SearchResults>`, `snippets: HashMap<RecordingId, Excerpt>`, and `announced`, the last result count spoken. An `Excerpt` is a `Snippet` and its `Source` (`Transcript`, `Summary`, or `Speakers`), so a row can say where it matched.
- [ ] Add `Library::snippets(&self, query: &Query, ids: &[RecordingId]) -> Result<Vec<Option<Excerpt>>, StoreError>`. For each id it reads the transcript, speaker names, and summary, builds the turns, and calls the pure `search::excerpt`, which uses `search::locate` and `search::snippet`. It falls back to the summary's lines, with their Markdown stripped, when no turn matches, and to a speaker name when neither does. `reload` asks for the first 30 Conversations hits in the same `core.run` call as the search, so the first screen paints once. A second call fills the rest in the background under the same generation.
- [ ] While the field has text, the sidebar replaces its date groups with two section headings, "Titles" and "In conversations", each a heading for the VoiceOver rotor. An empty section has no heading.
- [ ] Search always covers all text. There is no scope control, no filter-menu group, and no **Search Titles Only** menu item. The Titles section already answers "which recording is called this", so a title-only mode adds a control without a new answer (operator, 2026-11-17).
- [ ] A Conversations row gains a third line, the snippet, in 12 pt `text_secondary`. Matched words use `text`, semibold weight, and a new `search_match` fill token, so color never carries the match alone (A11Y-8). At text scale 1.25 or below the snippet is one line with an ellipsis. Above 1.25 it wraps to two lines, like the title. The row grows to fit, from 52 pt to about 68 pt at 100%. A title hit marks the matched words in its title the same way and has no third line.
- [ ] Each row's VoiceOver label adds where it matched to today's sentence, for example "Design review, 10:30, 48 minutes, Done. In transcript, we agreed the roadmap slips to Q3." A summary snippet says "In summary", a speaker-name match says "Speaker name", and a title hit adds "Title match".
- [ ] When results settle, post one polite announcement through `a11y::announce`, such as "12 results", "No results", or "Showing results for roadmap, 3 results". Typing that has not settled announces nothing.
- [ ] When S2 corrected the query, show one line above the sections, "Showing results for **roadmap**". When nothing matches, the list says "No recordings match "<text>"."
- [ ] The status filter (All, Processing, Needs attention, Done) applies on top of the search, as it does today.
- [ ] Escape in the search field clears it, like the clear button. Lane 10 depends on it, and the field did not do it before.

**You see.**

- [ ] Typing "budget" shows "Titles" with the recordings titled with "budget", then "In conversations" with snippets in which "budget" is marked.
- [ ] Typing a word found only in transcripts shows only "In conversations", with no empty "Titles" heading.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `crates/dory-core/src/search/mod.rs` tests `sections` with title hits, conversation hits, and an empty side. Run `cargo test -p dory-core search`.
- [ ] `crates/dory-core/tests/facade.rs` gains `snippets_come_from_turns_then_summary`. Run `cargo test -p dory-core --all-features --test facade`.
- [ ] `apps/dory-gpui/tests/search.rs` uses `TestAppContext` and the `gpui_kit::test` accessibility snapshots to check the section headings, a row label with "In transcript", the announcement text, the correction line, and the empty state's text. Run `cargo test -p dory-gpui --test search`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Search "budget" on trunk and at head on copies of one seeded folder. Save `sections-trunk.png` and `sections-head.png`. Trunk has no sections, so record its date-grouped list and gate head. Pass when head shows "Titles" above "In conversations", and the two sections hold the same set of recordings as the trunk list plus the head-only word-order matches.
- [ ] Lane 2. Search "roadmap". Save `snippet.png`. Pass when each Conversations row shows a snippet in which "roadmap" is marked by weight and fill, and the row is not clipped.
- [ ] Lane 3. Search "atlas", which the seed plants only in titles. Save `titles-only.png`. Pass when only the Titles section shows, its rows mark "atlas" in the title, and no empty "In conversations" heading is drawn.
- [ ] Lane 4. Search "kilimanjaro", which the seed plants in one transcript. Save `transcript-only.png`. Pass when only "In conversations" shows, with one row whose snippet marks the word.
- [ ] Lane 5. Search "roadmpa". Save `correction.png`. Pass when "Showing results for roadmap" sits above the sections.
- [ ] Lane 6. Search "zzqx", which matches nothing and is too short to correct. Save `empty.png`. Pass when the list says `No recordings match "zzqx".` and `app_ax_find` finds no section heading.
- [ ] Lane 7. Set View > Bigger to 200% and search "roadmap". Save `scale-200.png`. Pass when every snippet wraps to two lines at most and no row text is clipped.
- [ ] Lane 8. Operator lane. With VoiceOver on, search "roadmap" and move through the results with VO-Right. Save `vo-rows.png` with `screencapture -x`. Pass when VoiceOver speaks the result count once, then each heading, then each row with "In transcript" and its snippet.
- [ ] Lane 9. Set the status filter to **Needs attention**, then search "budget". Save `filter-and-search.png`. Pass when only recordings that both need attention and match are listed.
- [ ] Lane 10. Clear the search with the clear button and with Escape. Save `cleared.png`. Pass when the date groups return and the selection is kept.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. `search-settled ms=` at head, and the hitch time ratio while typing an eight-letter word at one key per 120 ms. Trunk has no sections or snippets, so its `search-settled ms=` measures the date-grouped list that the same keys produce.
- [ ] Probe. On the 500 by 2-hour seeded folder, type "budget", "roadmap", and "a" seven times each at trunk and head, alternating. Record the hitch time ratio with `xctrace record --template 'Animation Hitches'`.
- [ ] Baseline. Record the trunk debounce-to-paint medians first.
- [ ] Rule. Fail when a head median for a word of two letters or more exceeds 50 ms, when "a" exceeds 80 ms, or when the hitch time ratio exceeds 5 ms per second. The spike measured 2.6 to 4.7 ms to build 15 snippets from stored segments.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 2, 3, and 7 screenshots into `docs/design/review/s3-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of typing a query, a corrected typo, and reading the snippets. Save it as `docs/design/review/s3-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends S3 to the stack on S2's branch.

## Open a result at its first match and step through the matches (S4)

**Depends on.** S3.

**Files.**

- [ ] Edit `apps/dory-gpui/src/detail.rs`, `apps/dory-gpui/src/transcript_view.rs`, `apps/dory-gpui/src/summary_view.rs`, `apps/dory-gpui/src/app.rs`, `apps/dory-gpui/src/menus.rs`, and `apps/dory-gpui/src/actions.rs`.
- [ ] Edit `apps/dory-gpui/tests/search.rs`.
- [ ] Edit `docs/plans/ux-redesign.md` UX-32 and the shortcut table.

**Build.**

- [ ] Name the data shapes first. `Matches { query: Query, at: Vec<MatchAt>, current: usize }` lives in the detail pane's session. `MatchAt` is `{ place: Place, span: Range<usize> }`, and `Place` is `Title`, `Turn(usize)`, or `SummaryBlock(usize)`. One pure function, `matches(query, title, turns, summary_blocks) -> Vec<MatchAt>`, builds the list from `search::highlights`, in reading order.
- [ ] When the user opens a recording while a search is active, the detail pane picks the tab. A transcript match opens the Transcript tab, scrolls the first matching turn into view with `scroll_to_reveal_item`, and puts the keyboard cursor on it. A summary-only match opens the Summary tab. A title-only match opens the default tab.
- [ ] Mark every match in the title, the turns, and the summary with the same `search_match` style as the sidebar. Turns from legacy transcripts without times are marked and scrolled to the same way. They still cannot seek.
- [ ] Add **Edit > Find > Find Next** (Command-G) and **Find Previous** (Shift-Command-G). They move `current` and wrap at the ends. The tab action row shows "2 of 9 matches" with previous and next buttons. Moving to a match in the other tab switches the tab.
- [ ] Each move posts a VoiceOver announcement such as "Match 2 of 9, Sam, 12 minutes 30 seconds". Return on the current turn plays from its start, as it does today.
- [ ] Clearing the search removes the marks and the match counter.
- [ ] `match-revealed ms=` now ends at the first paint with the match in view.

**You see.**

- [ ] Opening a "roadmap" hit lands on the Transcript tab with the first turn that says "roadmap" in view and marked, and Command-G walks through every other "roadmap" in the recording.

**Verify, unit.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] `apps/dory-gpui/src/detail.rs` tests `matches` with a title match, two turns, and a summary block in reading order, and the wrap of next and previous. Run `cargo test -p dory-gpui detail`.
- [ ] `apps/dory-gpui/tests/search.rs` gains `opening_a_hit_reveals_the_first_matching_turn`, `find_next_switches_tabs`, and `clearing_search_removes_marks`. Run `cargo test -p dory-gpui --test search`.

**Verify, live.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked. Ten lanes on `sonnet high` at the PR head, per the boot recipe.

- [ ] Lane 1. Regression lane against trunk. Search "kilimanjaro" and open the hit at trunk and at head. Save `open-trunk.png` and `open-head.png`. Trunk opens at the top with no marks, so record that and gate head. Pass when head opens the Transcript tab with the planted turn in view and marked.
- [ ] Lane 2. Search a word planted only in a summary and open the hit. Save `summary-hit.png`. Pass when the Summary tab opens with the word marked.
- [ ] Lane 3. Search "roadmap", open a hit, and press Command-G until the counter wraps. Save `find-next.png` at each step. Pass when the counter reads "1 of N" to "N of N" and then "1 of N" again, and each marked word is in view.
- [ ] Lane 4. Press Shift-Command-G from the first match. Save `find-previous.png`. Pass when the counter reads "N of N" and the last match is in view.
- [ ] Lane 5. Open a hit whose matches are in both tabs and step past the last transcript match. Save `tab-switch.png`. Pass when the Summary tab opens on its first match.
- [ ] Lane 6. Open a hit in an imported legacy recording without times. Save `legacy.png`. Pass when the matching turn is in view and marked and its timestamp column is empty.
- [ ] Lane 7. On a match, press Return. Save `play-from-match.png`. Pass when the player starts at that turn's start time.
- [ ] Lane 8. Operator lane. With VoiceOver on, open a hit and press Command-G twice. Save `vo-match.png` with `screencapture -x`. Pass when VoiceOver speaks "Match 2 of N" with the speaker and the time, then "Match 3 of N".
- [ ] Lane 9. Operator lane. Turn on **Increase contrast** and open a hit. Save `contrast.png`. Pass when the marked words stay visible by weight and border.
- [ ] Lane 10. Clear the search while a hit is open. Save `cleared-detail.png`. Pass when the marks and the counter disappear and the scroll position stays.

**Verify, perf.** Tests alone are not sufficient verification. A PR is verified only when its unit, live, and perf boxes are all checked.

- [ ] Metric. `match-revealed ms=` when opening a 2-hour hit, and the time of one Command-G step. Trunk has no jump, so its `match-revealed ms=` measures opening the same recording at the top.
- [ ] Probe. On the 500 by 2-hour seeded folder, open the "kilimanjaro" hit and a "roadmap" hit seven times each at trunk and head, alternating, then press Command-G 20 times at head.
- [ ] Baseline. Record the trunk selection-to-paint median first.
- [ ] Rule. Fail when the head median exceeds the trunk median by more than 30 ms, when it exceeds 150 ms, or when a Command-G step exceeds 16 ms. The spike measured 2.4 ms to tokenize one 2-hour transcript and find its matches.

**Review gate.** The operator reviews before merge.

- [ ] Copy lane 1, 3, and 5 screenshots into `docs/design/review/s4-review-<slug>.png`.
- [ ] Record a 30 to 60 second video of opening a hit and stepping through its matches. Save it as `docs/design/review/s4-review.mp4`.
- [ ] Post the screenshots and the video in chat. Stop at merge-ready. Wait for the operator's click.

**Merge.**

- [ ] Root's clean verdict at the exact head SHA.
- [ ] Bugbot triage done.
- [ ] Rebased onto current trunk after the verdict, patch-id unchanged.
- [ ] The root appends S4 to the stack on S3's branch.

## Close the program

- [ ] Every box above is checked with its evidence.
- [ ] `docs/plans/decisions.tsv` has a row for every merge, every operator answer from Appendix E, and every scope change.
- [ ] Reply to the operator with the report the execution playbook names.

## Appendix A. Prototype evidence

A throwaway spike settled the engine and the speed questions before this plan. Its source and raw output are in `docs/plans/search-spike/`, committed with this plan. It is its own cargo workspace and never joins the app build. To rerun it, use `CARGO_TARGET_DIR=/tmp/search-spike-target cargo run --release --manifest-path docs/plans/search-spike/Cargo.toml -- /tmp/search-spike 500 18000`, then `--bin two -- /tmp/search-spike` for the second half.

The corpus is synthetic. It has 500 recordings of 18,000 words each, every one at the 2-hour cap, which comes to 88 MB of transcript text and 40,445 distinct terms with Zipf frequencies from `/usr/share/dict/words`. Dictionary words are about twice as long as spoken words, so a real 500-recording library has less text than this one. The numbers are medians of seven warm runs on an Apple M3 Pro with the bundled SQLite 3.54.1 that `rusqlite` 0.41.1 builds with `SQLITE_ENABLE_FTS5`. The test library at `dory-dev/live-data` has only three short recordings, so it was used only to read the real schema.

| Measurement | Result |
| --- | --- |
| Trunk `instr` scan, one query | 41 ms with hits, 68 ms with none |
| Contentless FTS5 index, `prefix='2 3'` | 61 MB, built in 2.3 s |
| FTS5 query with bm25, a word or two words | 0.03 to 0.3 ms |
| FTS5 query `a*` | 31 ms with the prefix index, 77 ms without |
| Replace one 2-hour transcript in the index | 4 ms |
| FTS5 `snippet()` on a contentful table | 205 ms for 376 hits, 53 ms for the top 40, 928 ms for the top 40 of `a*` |
| Snippets in Rust from stored segments, 15 rows | 2.6 to 4.7 ms, 38 ms when no segment matches |
| Read the `fts5vocab` term list | 64 ms |
| Optimal string alignment over 40,445 terms | 2.5 to 10 ms |
| Trigram FTS5 index | 276 MB, built in 6.5 s |
| Rust tokenizing and matching over all text per query | 640 ms |

What the spike proved:

- FTS5 is compiled into the workspace's bundled SQLite, so S1 adds no dependency to `dory-store`.
- A contentless index is smaller than the three `_lc` columns it replaces, and it answers word queries more than 100 times faster than trunk.
- FTS5 `snippet()` is too slow for the sidebar, and it needs a full copy of the text. Building snippets in the core from the stored transcript is fast enough for the rows on screen.
- One-edit correction finds "roadmap" for "roadmpa", "budget" for "budgte", "muller" for "mueller", and "antinational" for "antinatoinal". Plain Levenshtein distance missed the two swaps, which is why S2 uses optimal string alignment.
- SQLite's `unicode61` tokenizer splits "don't" into "don" and "t", leaves `ø`, `ł`, `ß`, and the `ﬁ` ligature unfolded, and keeps a Japanese run as one token. That probe is why the core tokenizes and folds and the index stores the core's tokens.

What stays unproven until the PRs run:

- The speed on a real library with real speech, which S1's perf block measures with the `seed` corpus and lane 10 checks on the test library.
- How a VoiceOver user hears sections, snippets, and match steps. S3 lane 8 and S4 lane 8 are Operator lanes.

The planted words that `dory-cli seed` writes are "kilimanjaro" in one transcript, "Müller" and "Muller" in different transcripts, the adjacent pair "budget review" in one transcript, "review of the budget" in another, "roadmap" in many transcripts and one title, "atlas" in titles, "antinational" in transcripts, and one word that appears only in a summary.

## Appendix B. Alternatives rejected

- **The core scans all text for each query.** It keeps the store simple but took 640 ms per query on the spike corpus and needs every transcript in memory.
- **A trigram FTS5 index.** It finds substrings, so "map" would find "roadmap", but it was 4.5 times the size of the word index, took 6.5 s to build, and cannot rank by words. The operator accepted losing infix matches.
- **FTS5 `snippet()` and `highlight()`.** They need a contentful table that copies all text again, and they cost up to 928 ms. They would also mark words by SQLite's tokenizer, not the core's.
- **A separate `SearchIndex` port, fed by store events.** It would commit apart from the stage output, so a crash between the two commits would leave the index wrong and need repair code. Inside the store, the index row commits in the same transaction as the text it indexes.
- **Letting SQLite's `unicode61` tokenizer fold accents.** It does not fold `ß`, `ø`, or `ł`, and the core would need a copy of its rules to mark words. A custom FTS5 tokenizer that calls the core needs unsafe FFI in `dory-store`. Storing the core's tokens gets the same result with no FFI.
- **Correcting every long word, even when the exact search finds results.** It finds more, but a correctly spelt word would pull in near words, and results would change in ways the user did not ask for. S2 corrects only when nothing matches.
- **Prefix only on the last word typed.** It keeps "a" cheap but makes "bud rev" miss what "budget review" finds. Every word matches word starts, and "a" still meets its budget.
- **A `title:` search token.** Nobody discovers it, and VoiceOver users get no hint that it exists.
- **A title-only scope, as a visible All and Titles control or a "Search in" group in the filter menu.** The operator chose one search over all text. The Titles section already lists title hits first, so a second mode adds a control and a state without a new answer.
- **Ranking conversation matches by bm25.** It puts a meeting about a topic above a passing mention, but the order stops matching the dates the user remembers. The operator chose newest first.

## Appendix C. Risks

- **A long backfill on the first launch after S1 (S1).** v6 indexes every recording in the migration transaction. The spike's worst case is 2.3 s for 500 recordings at the 2-hour cap. The owner logs the time and adds a chunked background backfill only if the perf lane fails.
- **A tokenizer change silently stales the index (S1, later).** A contentless index cannot be read back to check it. `tokens_version` forces a full rebuild, and `tokens_version_bump_reindexes` tests it. Every change to `tokens` or `fold` must bump the constant, and the S1 owner adds a test that fails when the token table changes without a bump.
- **Word-start matching loses infix matches (S1).** "map" no longer finds "roadmap", and a German compound such as "Projektbudget" is not found by "budget". Trunk finds both. The operator accepted this, and the owner records it in the S1 PR body.
- **Scripts without spaces other than Chinese and Japanese (S1).** Thai and Lao still index as long runs and match only by prefix. The meeting languages do not include them today.
- **The facade rule "every persisted change goes through `Store::apply`" (S1).** The migration backfill and the `tokens_version` rebuild write the index outside `apply`. Both are adapter-internal, like a schema migration, and every domain change still reindexes inside `apply_change` in the same transaction. The S1 owner states this in `docs/architecture/hexagonal.md`.
- **Cold vocabulary on a zero-result search (S2).** The first corrected search after a write may take up to 100 ms. Only zero-result searches pay it.
- **A phrase that spans two speakers (S3, S4).** The index joins segments, so the store reports a hit, but no single turn holds the phrase. The snippet then falls back to the first turn with any of its words. `locate` tests cover it.
- **Untitled recordings are not found by their shown title (S1).** The sidebar shows "Recording 17 Nov, 10:30" for an untitled recording, built from local time. The index holds an empty title, as trunk does. Date search is out of scope.
- **Shared build folder (all PRs).** Other agents build in the shared `target/`. Lanes build into `/tmp/dory-target-<pr-id>` and check free disk first.

## Appendix D. Links and reading list

- `docs/plans/ux-redesign.md` defines UX-1, UX-21, A11Y-1, A11Y-4, A11Y-7, and A11Y-8, and gains UX-32 in S3.
- `docs/architecture/hexagonal.md` defines the store port and the composition root. S1 updates it.
- `docs/design/handoff-direction-c.md` section 4 and `docs/design/processing-view.md` section 7 define the sidebar row that S3 grows.
- `docs/plans/search-spike/` holds the spike source and its output.
- `docs/plans/decisions.tsv` is the decision trail per pstack `show-me-your-work/SKILL.md`.
- Run pstack `how/SKILL.md` over `crates/dory-store/src/sqlite/` before S1. Run pstack `interrogate/SKILL.md` on S1 before review, because it changes the schema and the port.

## Appendix E. Operator answers

The operator answered these on 2026-11-17, before S1 starts.

1. Inside "In conversations", should results be newest first or most relevant first? **Newest first.** `Found` carries no bm25 column.
2. Word-start matching means "map" no longer finds "roadmap", and "budget" no longer finds "Projektbudget". Trunk finds both. Is that acceptable, or should a later PR add a trigram index for infix matches? **Accepted.** No trigram index. On the spike corpus it was 276 MB against 61 MB for the word index.
3. Should the All and Titles control be visible under the search field, or live only in the filter menu? **Neither.** Search always covers all text, with no scope control and no **Search Titles Only** menu item. The Titles and In conversations sections stay.
