# Patch against sherpa-onnx 2.0.1

This folder is the published `sherpa-onnx` 2.0.1 crate (the `src/`, `Cargo.toml`, `README.md`, and `LICENSE` that crates.io ships), with one change. `OfflineRecognizerResult` gains the fields that sherpa-onnx's JSON result already carries and the 2.0.1 Rust wrapper drops. The root `Cargo.toml` points `[patch.crates-io]` at this folder.

Dory reads `lang`, which Whisper fills on every window, to record the language it heard. The three `segment_*` fields hold sentence times when `enable_segment_timestamps` is set. Dory does not set it yet. Every added field is `#[serde(default)]`, so a model that reports none parses as empty.

Upstream PR to k2-fsa/sherpa-onnx not filed; it needs the operator's go.

Delete this folder and the `[patch.crates-io]` line once a release that carries these fields is the pinned `sherpa-onnx` version.

## Diff against 2.0.1

```diff
--- a/src/offline_asr.rs
+++ b/src/offline_asr.rs
@@ -650,6 +650,19 @@
     pub tokens: Vec<String>,
     pub timestamps: Option<Vec<f32>>,
     pub durations: Option<Vec<f32>>,
+    /// The language the model reports, such as `en` or `tl`. Whisper fills it on every window.
+    /// Empty for models that report none.
+    #[serde(default)]
+    pub lang: String,
+    /// Start of each sentence in seconds, when `enable_segment_timestamps` is set.
+    #[serde(default)]
+    pub segment_timestamps: Vec<f32>,
+    /// Length of each sentence in seconds, when `enable_segment_timestamps` is set.
+    #[serde(default)]
+    pub segment_durations: Vec<f32>,
+    /// Text of each sentence, when `enable_segment_timestamps` is set.
+    #[serde(default)]
+    pub segment_texts: Vec<String>,
 }
 
 /// Offline speech recognizer.
```
