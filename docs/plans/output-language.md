# Output language

One app-wide setting says which language transcripts and summaries are written in. The default, **Same as spoken**, behaves exactly as Dory does today. Picking a language makes Dory translate the transcript into it and write the summary in it, on device.

## What the user gets

- Settings > Transcription gains a row, "Write transcripts and summaries in". Its popup offers "Same as spoken", then every language Dory names.
- A recording processed with a target gets a translated transcript beside its original, and a summary written in the target. The transcript header shows an Original / Translated switch. Translated is the default.
- Existing recordings do not change. The setting applies to recordings processed after it is set.
- Without a summary model, translation blocks exactly as summarizing does, with the same "needs attention" row and the same fix.

## Where the design came from

Two whole shapes were weighed. One reviewer, no multi-model arena, so this is a single-author design.

| Shape | Verdict |
| --- | --- |
| **A. A `Translate` stage between Transcribe and Summarize, with its own stored translation.** | Chosen. The job machine, `StageProgress`, checkpoints, cancel, and the blocked path already exist for stages. The original transcript is never written. |
| B. Translate inside the Summarize stage. | Rejected. No separate progress, no checkpoint, so a quit loses all translation work, and the Summarize stage would need two outputs. |
| C. Overwrite segment text with the translation. | Rejected. Destroys the original, and a target change could not be undone. |

Rejected alternative for the engine: Whisper's translate task. It only goes to English, it ties translation to one speech model, and it cannot translate text that another model produced. Translation runs on the summary LLM through the existing `ModelProvider<Llm>` instead.

## Premise check

The brief says each segment carries `language`. Only `RawSegment` and the transcription checkpoint do. The stored `Segment` drops it when the transcribe stage places and merges segments, so the skip rule ("do not translate what is already in the target") has nothing to read. The fix is to carry it: `Segment` gains `language: Option<LangTag>` and v9 stores it. A segment with `None` (an import, or a recording transcribed before v9) is treated as unknown language and is translated.

## Data shapes

`dory-core/src/domain/`:

```rust
Settings { output_language: Option<LangTag>, .. }          // None = Same as spoken
Segment  { language: Option<LangTag>, .. }
Stage    { Transcribe, Translate, Summarize }
Goal     { transcribe, translate, summarize }              // translate only with transcribe

struct Translation {
    target: LangTag,
    transcript_revision: Revision,
    produced_by: ModelRef,
    texts: Vec<Option<String>>,   // by segment index; None keeps the spoken text
}
enum TranslationFlag { None, Current, Stale }
TranslationFlag::derive(translated: Option<(Revision, &LangTag)>, transcript: Option<Revision>, wanted: Option<&LangTag>)
```

`derive` mirrors `SummaryFlag::derive`. A translation is Current only for the transcript revision it was made from and the target now wanted. Anything else is Stale. Every reader also checks `Translation::applies_to(revision)`, so a translation made for another revision is never shown against segments it does not line up with.

`ports/store.rs`:

```rust
StoreRead::translation(&RecordingId) -> Option<Translation>
StoreRead::translation_checkpoints(&TranslationKey) -> Vec<TranslationBatch>
Change::TranslationReplaced { id, translation }
Change::TranslationBatchSaved { key, batch }
Change::TranslationCheckpointsCleared(RecordingId)
struct TranslationKey { recording, revision, target, model }
struct TranslationBatch { segments: Vec<(usize, String)> }
```

Every persisted fact goes through `Store::apply`. The final translation and its `StageDone` commit together, as the transcript's do.

## Pure core rules (`app/stages/translate.rs`)

- `needs_translation(segment, target)`: the text is not blank and the segment's language is unknown or has another primary subtag than the target. `en-US` against `en` is already in the target.
- `plan_batches(costs, budget)`: consecutive segments, never split, packed by token cost. A segment over the budget goes alone.
- `budget(context, system)`: 40% of what the context leaves after the system prompt and the overhead, capped at 1,200 tokens. The output is allowed 125% of the input, so input, output, prompt, and overhead fit the context.
- `system_prompt(target)` and `user_prompt(items)`: numbered lines `[n] text` in, the same markers out.
- `parse_reply(reply, n)`: exactly the markers 1 to n, each once, in order. Anything else is a miss.
- `translate_batch`: on a miss, split the batch in two and retry, down to one segment. A one-segment miss takes the whole reply, trimmed, as the translation. A model that fumbles the format costs calls, not segments.

No function names a model. The stage resolves the LLM through the registry like Summarize.

## The stage

`stages::translate` runs after Transcribe and before Summarize.

1. No target in `Settings`, or the LLM choice is Off: return no changes. The job moves on. This is how a stage that was queued before the user cleared the setting stays harmless.
2. Resolve the LLM exactly as Summarize does. A missing model is `Blocked`, with the same blockers, so the UI's existing path applies.
3. Plan the segments that need translating. If none do (an English meeting translated to English), return no changes and store no translation. This runs before the model is resolved, so such a meeting never blocks on a missing model.
4. Load the saved batches for `(recording, revision, target, model)` and drop the segments they cover.
5. Batch the rest. Before each batch: honour cancel, read the pace budget, publish `StageProgress { stage: Translate, fraction, budget }`. Run the batch. Commit a `TranslationBatchSaved`.
6. Return `TranslationReplaced` plus `TranslationCheckpointsCleared`.

A quit mid-stage leaves the job `Queued` (the existing shutdown path). The next run finds the checkpoints and continues. A new transcript revision or another model or target changes the key, so old checkpoints are ignored and cleared with the next result.

Pace: the budget is read before every batch and published, so the Processing card shows Light and why. The LLM port has no thread knob, so Light does not slow an Ollama call. The summary stage is in the same position. This is approximated, not enforced.

## Summary

With a target, `Summarize` writes in the target, not `main_language()`. When a Current translation exists, the transcript lines it summarizes come from the translated text, because a small model summarizes translated text better than it translates and summarizes in one go. The notes passes of map-reduce also see translated lines and therefore write in the target. The `LlmNeeds` language is the target. `Summary.language` is the target.

With no target nothing changes: `transcript_lines` reads the spoken text, `language` is `main_language()`, and the prompt strings are byte for byte today's. A pipeline test pins both the prompt and the stage list.

## Schema v9

The search branch takes v8 and lands first. `MIGRATIONS` is a positional list, so this branch carries `schema_v8.sql` as a one-line placeholder and its own work in `schema_v9.sql`. On merge, take search's `schema_v8.sql` and keep `schema_v9.sql` as is. If the search branch ends up at a different number, rename `schema_v9.sql` and fix its `include_str!` position. Nothing else mentions the number except the `LATEST` constant in the store contract test.

```sql
ALTER TABLE segments ADD COLUMN language TEXT;
CREATE TABLE translations (recording_id PK, target, transcript_revision, produced_by, texts JSON, ..);
CREATE TABLE translation_batches (recording_id, transcript_revision, target, model, first_idx, segments JSON, ..);
```

Both tables cascade on recording delete. The job `goal` column gains bit 4 for translate, so no job migration is needed. The new `output_language` setting is a key in the existing `settings` table.

## Apps

- **dory-cli**: `output-language [TAG|same]` shows or sets it through `update_settings`, next to the existing `languages` command. `show ID` prints the translated transcript when there is one, and `show ID --original` prints the spoken one. `process` names the translate stage in its progress lines.
- **dory-gpui**:
  - Settings > Transcription gets the row, in the same popup style as Language.
  - The transcript header gets a two-button Original / Translated switch when the recording has a translation that applies. Each translated turn's VoiceOver label ends with "translated from Tagalog". Copy and export use what is shown.
  - The processing card, the queue, the library row, and the menu-bar item learn the Translating stage. A translation blocked for want of the summary model takes the summary's path (Set Up Summaries). A failed translation says so and offers Try Again.
  - The Original / Translated choice lives on the `Session`, so the list, the switch, Copy, and Export read one value. It starts on Translated for each recording you open.
- **dory-mcp** (through `dory-core::assistant`): `get_transcript` returns the translated text as `text` when there is a translation, plus `original` and `translated_from` per translated segment. Grants and the hidden flag do not change.

## Existing recordings and trash

- A translation made before the user turned the setting off is still shown and still switchable. It only goes stale when the transcript is re-made, and then it is hidden, because its segments no longer line up.
- Move to Trash and Undo carry the translation with the transcript and the summary.

## Not built

- A per-recording "Translate" action, for recordings that predate the setting.
- Context from neighbouring batches. Each batch is translated alone.
- Translating a recording whose speakers were renamed does not touch names. Names stay.

## Known limits

- Skipping relies on per-window language detection. A Tagalog sentence that Whisper labels English is not translated. Taglish inside one sentence is not measured (see the code-switching note in `speech-model-evaluation.md`).
- The target name in the prompt is a BCP 47 tag, as the summary prompt already does.

## Verification plan

Unit tests for the rules, store contract tests for both stores (migration, round trip, staleness, cascade), pipeline tests for the stage (resume, cancel, blocked, skipped, same-as-spoken golden), GPUI tests for the setting, the switch, and the VoiceOver label, then a live run on a copy of the test library with Gemma.
