# Fixtures

`legacy-data/` is a small copy of the old app's data folder, shaped like the real one:
`recordings/<id>.webm|wav` and `metadata/<id>.json`, with ids from the old `generateId()`.

| id suffix | what it covers |
| --- | --- |
| `k3j2h1x` | done, three speakers, Vietnamese and English, display name, summary |
| `q9w8e7r` | queued with the front-of-queue flag, nothing produced yet |
| `zz11aa2` | failed after transcribing, with the error text |
| `mm44nn5` | interrupted mid-transcription (`transcribing`) |
| `ww22xx3` | a real WAV, no pipeline status, transcript without speaker labels, summary |
| `cc55dd6` | corrupt JSON, reported and skipped |
| `loose01` | audio with no metadata file |
| `ghost99` | metadata with no audio, skipped |

The `.webm` files are not decodable audio. The importer only points at them.
