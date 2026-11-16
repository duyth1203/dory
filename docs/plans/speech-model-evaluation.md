# Speech model evaluation plan

Dory transcribes with whisper.cpp GGUF models through the Sona sidecar, with English hardcoded and optional Sortformer diarization. It summarizes with `gemma4:e4b` through Ollama. This plan runs a bake-off on the operator's Mac to pick a faster speech model with equal or better accuracy and speaker labels. It also decides whether a newer Gemma improves summaries. The winner ships as rows in the `ModelCatalog` from GPUI plan PR P6, behind the `Transcriber` trait from PR P1.

The research behind the candidate list is dated 2026-11-15. Leaderboard numbers come from the Open ASR Leaderboard snapshot of 2026-11-13, whose RTFx is measured on GPUs. Apple Silicon speed is unknown until this plan measures it.

## Plan for meetings in more than one language

The operator answered on 2026-11-15 that meetings use English and other languages. The exact languages are not named yet. That answer changes the bake-off in four ways.

- The default model must cover every language the operator names and detect the language by itself. A meeting can switch language partway through, so detection runs per segment where the engine supports it, not once per file.
- Qwen3-ASR (52 languages, built-in language ID) is the default candidate. Whisper large-v3-turbo (99 languages) is the multilingual baseline, because `ggml-base.en` is English only.
- Parakeet TDT v3 covers 25 European languages. It stays the default candidate only if every named language is in that set. Otherwise it competes as an optional fast path for English and European meetings.
- Granite Speech 5.0 covers six languages and Cohere Transcribe 14 with no language detection. Each stays in the bake-off only if it covers every named language.

Before E1, the operator names the languages. Record them as a row in `docs/plans/decisions.tsv`. Until then, E0 scores the baseline on English plus the eight languages of the leaderboard's multilingual tab (zh, de, fr, it, es, pt, hi, nl).

## Candidates

### Speech to text

| Candidate | Why it is in the bake-off | Rust path on Apple Silicon | License |
| --- | --- | --- | --- |
| Current Sona plus `ggml-large-v3-turbo` and `ggml-base.en` | Baseline. Leaderboard average WER 7.69 for turbo. | Sona sidecar, unchanged | MIT |
| Parakeet TDT 0.6B v3 | Average WER 6.19, word timestamps, local attention up to 3 hours. FluidAudio reports 190x to 335x real time on M4 Pro and M5 Pro. | `sherpa-onnx` Rust crate 2.0.1 in process, or `fluidaudio-rs` on the Neural Engine | CC-BY-4.0 |
| Parakeet Ultra | FluidAudio reports higher accuracy than v3 at the same speed. | `fluidaudio-rs` | Check the model card in E3 |
| Qwen3-ASR 0.6B and 1.7B | Best small multilingual model. Average WER 6.30 and 5.42, multilingual error 5.94 for 1.7B. Timestamps need Qwen3-ForcedAligner. | `sherpa-onnx` Rust crate, or a llama.cpp GGUF sidecar | Apache-2.0 |
| Cohere Transcribe 03-2026 | Average WER 5.77 and long-form 11.88. No timestamps and no diarization, so it needs an aligner. | `sherpa-onnx` Rust crate | Apache-2.0 |
| Granite Speech 5.0 2B | Average WER 5.75 with official GGUF and ONNX. Needs about 2-minute segments. English and Japanese strength. | ONNX through `ort`, or GGUF | Apache-2.0 |
| MOSS-Transcribe-Diarize 0.9B | The only small model with speech, timestamps, and speaker labels in one pass up to 90 minutes. Average WER 6.19. | None in Rust today. Evaluate quality only, through mlx-audio in Python. | Apache-2.0 |

Out of the bake-off, with reasons:

- **Gemma 4 audio (E2B, E4B, 12B).** The model card caps audio input at 30 seconds, and E4B scores 12.06 average WER, about twice Parakeet's error. Ollama's `gemma4` tags accept only text and images.
- **VibeVoice-ASR.** About 8 to 9 billion parameters, and one tester reported it needs 24 GB of RAM.
- **Nemotron 3.5 Transcribe 30B-A4B.** It is the most accurate open model on the board but its reference deployment is one H100 80 GB.
- **Voxtral and Omnilingual 7B.** Slower than Whisper at similar or worse WER.
- **Community fine-tunes at the top of the leaderboard.** Low downloads and no vetting.

### Speaker diarization

| Candidate | Why | Rust path | License |
| --- | --- | --- | --- |
| Streaming Sortformer v2.1 | Baseline, already in Dory. Max 4 speakers. | Sona's `sona-diarize` sidecar | NVIDIA Open Model License |
| Nemotron-3-Diarization | Up to 8 speakers, streaming and offline, GGUF published. | `parakeet-rs`, NeMo-Speech.cpp sidecar, or `fluidaudio-rs` | OpenMDW-1.1 |
| pyannote community-1 | FluidAudio reports AMI DER 10.6% at about 323x real time on M5 Pro. | `fluidaudio-rs`, or `sherpa-onnx` with pyannote 3.0 segmentation | CC-BY-4.0 |
| Channel split | GPUI plan P2 records the mic and system audio as separate channels. Mic speech is always "You", so diarization runs only on the system channel. Recordings made before P2 are one mixed track and still need full diarization. | No model needed for the split | None |

Test the channel split in every diarization run. It may make a 4-speaker model enough for most meetings.

### Summaries

| Candidate | Why | Runtime |
| --- | --- | --- |
| `gemma4:e4b` | Baseline. 128K context fits a 2-hour transcript (about 25k to 30k tokens). | Ollama |
| `gemma4:12b` (Gemma 4 12B Unified, released 2026-06-03) | MMLU-Pro 77.2% against 69.4% for E4B, 256K context, about 8 GB download. | Ollama |
| `gemma4:e2b` | A floor for 8 GB Macs. | Ollama |

Google released no Gemma after 4 and no general speech model besides MedASR (medical dictation, gated license) as of 2026-11-15. Re-check https://ai.google.dev/gemma/docs/releases at step E0.

## Build the bench before measuring anything

The bench lands as its own PR, outside the GPUI stack. It can start before GPUI plan P1 and does not touch the app.

1. Create `crates/dory-bench` with a `dory-bench asr` command. It takes `--engine`, `--model`, `--set`, and `--out results/<engine>-<model>-<set>.jsonl`. It writes one JSON line per file with the hypothesis, word timestamps, speaker turns, wall time, audio duration, peak RSS, and model load time.
2. Each engine implements the `Transcriber` trait from GPUI plan PR P1. If P1 has not landed, copy the trait into `dory-bench` and delete the copy when P1 lands.
3. Create `scripts/score.py`, a dev-only Python script. It computes WER with `jiwer`, using the Whisper English normalizer for English and the Whisper basic normalizer for other languages. For languages written without spaces (Chinese, Japanese, Thai) it computes character error rate instead. It also computes cpWER with `meeteval`, and DER with `pyannote.metrics` at a 0.25-second collar. It writes `results/summary.md`.
4. Extend `scripts/fixtures.sh`, which GPUI plan P1 creates. If the bench PR lands first, it creates the script and P1 extends it. It downloads these sets and checks SHA-256:
   - AMI meeting corpus test split (CC BY 4.0), headset mix and far-field, with reference speakers.
   - Earnings-22, for long-form audio up to an hour.
   - A 2-hour file made by joining AMI meetings, for the long-file path.
   - The FLEURS test split for each named language, or the eight default languages until they are named.
   - A code-switching set of at least 5 of the operator's own recordings in which speakers change language mid-meeting, under `fixtures/private/`.
5. Add 5 of the operator's own recordings with hand-corrected transcripts under `fixtures/private/`. Add `fixtures/private/` to `.gitignore`. These never leave the Mac.

## Measure under one protocol

- Run on the operator's M3 Pro on AC power, with other apps closed. Repeat the final shortlist on an 8 GB Apple Silicon Mac if one is available, because that is Dory's floor.
- Run each engine three times per set, interleaved by engine so thermal drift hits every engine equally. Report the median and the spread.
- Report warm speed (model loaded) and cold speed (first file after launch) separately.
- Compute RTFx as audio seconds divided by wall seconds for the whole file, including chunking and diarization.
- Before trusting a speedup, check what limits it. Confirm the engine used the Neural Engine or the GPU with `powermetrics --samplers ane_power,gpu_power`, and confirm the transcript is complete, not truncated.

## Run the steps

- [ ] **E0.** Build the bench, download fixtures, and score the baseline (Sona with turbo and base.en, plus Sortformer). Evidence is `results/summary.md` at a commit SHA.
- [ ] **E1.** Add the `sherpa-onnx` engine and score Parakeet v3, Qwen3-ASR 0.6B and 1.7B, and Cohere Transcribe. Add Qwen3-ForcedAligner for Qwen3 and Cohere timestamps.
- [ ] **E2.** Add the diarization runs. Score Sortformer v2.1, Nemotron-3-Diarization, and pyannote community-1, each with and without the channel split, on the best two E1 models.
- [ ] **E3.** Add the `fluidaudio-rs` engine and score Parakeet v3 and Parakeet Ultra on the Neural Engine. Compare its speed with E1's CPU path for the same model.
- [ ] **E4.** Score MOSS-Transcribe-Diarize through mlx-audio for quality only. If it wins cpWER by more than 2 points, file a follow-up to find or build a Rust runtime. Do not ship a Python runtime in Dory.
- [ ] **E5.** Run each summary candidate on 10 transcripts from E1's winner. Score each summary blind on a 1 to 5 rubric for coverage of decisions, action items, and factual errors against the transcript. Record time to first token and total time.
- [ ] **E7.** Prototype the live transcript recognizer for UX-27. Compare a whisper.cpp streaming setup with Apple's `SpeechTranscriber` volatile results (macOS 26, fixed locale). Feed both from the capture harness during a 60-minute call with a video app running. Record draft accuracy against the final transcript, CPU, and energy from `powermetrics`. The live recognizer is a separate choice from the post-stop model.
- [ ] **E6.** Pick the defaults by the rule below. Write the `ModelCatalog` rows for GPUI plan PR P6 and log the decision.

## Pick the winner by this rule

1. Drop any candidate whose license forbids commercial use or whose runtime cannot run in-process or as a bundled native sidecar.
2. Drop any candidate whose peak RSS exceeds 3 GB, so Dory and Ollama fit together on a 16 GB Mac.
3. Drop any candidate that does not cover every named language. Drop any candidate whose AMI WER, AMI cpWER, or FLEURS error rate in any named language is worse than Whisper large-v3-turbo's.
4. Drop any candidate that picks the wrong language on more than 2% of segments in the code-switching set.
5. Among the rest, the default is the fastest by warm RTFx on the 2-hour file. A candidate within 10% of the fastest's speed wins if its cpWER is at least 1 point better.
6. Offer at most two more models in Settings. One is smaller for 8 GB Macs. The other is Parakeet as an English and European fast path, if it lost the default slot only on language coverage.
7. For summaries, write the summary in the meeting's main language. Add "summary uses the meeting's language" to the E5 rubric, and include at least 3 non-English transcripts among the 10. Switch the default from `gemma4:e4b` only if the new model scores at least 0.5 rubric points higher and finishes a 2-hour transcript in under 60 seconds on the M3 Pro.

## Hand the result to the GPUI program

- The winning engine becomes a `Transcriber` implementation in `dory-core` in its own PR below P6 in the stack, with the bench numbers in the PR body.
- The model rows go into `ModelCatalog` in P6 with size, checksum, and download URL.
- If the winner is not Sona, remove the Sona sidecar and `.sona-version` in the same PR that makes the new engine the default, so the bundle carries one engine.
- Language becomes a setting instead of the hardcoded English in `run_sona_transcription`. The default is **Automatic**, and the user can pin one language per recording (UX-26).

## Measured on the operator's Mac (2026-11-16)

The operator's meetings are Tagalog and English. Sona's repo is gone, so the bake-off moved to sherpa-onnx 2.0.1 in process, int8 models, CPU, and up to 4 threads on an M3 Pro. The bench is `crates/dory-engines/examples/wer-bench.rs`. It ran on the first 37 Filipino and 43 English FLEURS test clips, streamed within a 50 MB download budget. Text is lowercased and stripped of punctuation before scoring. The RTFx column is audio seconds per second of compute.

| Model | Tagalog WER, auto | Tagalog WER, pinned `tl` | English WER, auto | RTFx |
| --- | --- | --- | --- | --- |
| Parakeet TDT 0.6B v3 | 86.5%, unusable | refused | 7.2% | 14 |
| Qwen3-ASR 0.6B | 45.4% | 46.5% | 6.8% | 6 |
| Whisper large-v3 turbo | 14.6% | 14.6% | 6.2% | 3 |

- Qwen3-ASR spells Tagalog phonetically ("tukomento" for "dokumento"), so its errors are real misrecognitions, not a scoring artifact. It is dropped from the catalog. It loses on Tagalog by 31 points and adds nothing for English.
- Parakeet stays the recommended row for meetings whose languages it covers, because it is about five times faster than Whisper for 1 point more English error.
- Whisper turbo is what Auto picks once Tagalog is a meeting language.
- FLEURS is read speech, not meetings. Taglish code-switching inside one sentence is not measured. One code-switch sample in the sherpa agent's live test showed Whisper on auto turning non-English parts into repeated English. Measuring real Taglish meeting audio is the next step before calling this final.
- 945 Tagalog and 921 English reference words. The Tagalog gap is far outside run-to-run noise. The English differences of 1 point are not.

## What is not verified yet

- A research agent reported a 404 for Sona's GitHub repository on 2026-11-15. This run did not re-check it. If it holds, Sona's supported engines and its future are unknown, which raises the value of moving off it. `scripts/pre_build.py` downloads Sona release `v0.2.1`, so check that the download URL still works before E0.
- Apple Silicon speeds for Qwen3-ASR, Cohere Transcribe, Granite, and MOSS.
- Whether the `sherpa-onnx` Rust crate exposes the CoreML execution provider.
- `parakeet-rs` warns that CoreML is unstable for TDT models, so its CPU path may be the only reliable one.
