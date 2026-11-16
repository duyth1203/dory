//! The sherpa-onnx speech models Dory offers, as data. Adding a model is one row here.

/// How an archive lays out its files, which decides how the recognizer is configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    Parakeet,
    Qwen3,
    WhisperTurbo,
}

impl Layout {
    /// Every file the recognizer reads, relative to the model directory. An installed model has
    /// all of them, and provisioning extracts only these.
    pub(super) fn files(self) -> &'static [&'static str] {
        match self {
            Layout::Parakeet => &[
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "joiner.int8.onnx",
                "tokens.txt",
            ],
            Layout::Qwen3 => &[
                "conv_frontend.onnx",
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "tokenizer/merges.txt",
                "tokenizer/tokenizer_config.json",
                "tokenizer/vocab.json",
            ],
            Layout::WhisperTurbo => &[
                "turbo-encoder.int8.onnx",
                "turbo-decoder.int8.onnx",
                "turbo-tokens.txt",
            ],
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Archive {
    pub file: &'static str,
    pub bytes: u64,
    /// Lowercase hex.
    pub sha256: &'static str,
    /// The single top-level directory inside the archive.
    pub root: &'static str,
}

#[derive(Clone, Debug)]
pub(super) struct Model {
    pub name: &'static str,
    pub display_name: &'static str,
    pub archive: Archive,
    pub layout: Layout,
    /// Extracted size of `layout.files()`.
    pub disk_bytes: u64,
    /// Pairs of a BCP-47 tag and the code the engine expects for it. Aliases repeat the code.
    pub languages: &'static [(&'static str, &'static str)],
    pub window_secs: u64,
    pub recommended: bool,
    pub rank: i16,
    pub summary: &'static str,
    pub pros: &'static [&'static str],
    pub cons: &'static [&'static str],
}

pub(super) fn models() -> Vec<Model> {
    vec![
        Model {
            name: "parakeet-tdt-0.6b-v3",
            display_name: "Parakeet TDT 0.6B v3",
            archive: Archive {
                file: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
                bytes: 487_211_443,
                sha256: "1495e91b59d7fc53ddca91f3431758ec99f8fa253fdb0e1e851be0f72b14b0f9",
                root: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
            },
            layout: Layout::Parakeet,
            disk_bytes: 670_576_784,
            languages: PARAKEET_V3,
            // The encoder attends over the whole window, and two minutes keeps that cheap on CPU
            // while giving the model whole conversational turns; word timestamps split it after.
            window_secs: 120,
            recommended: true,
            rank: 30,
            summary: "NVIDIA's fast speech model for English and 24 other European languages.",
            pros: &[
                "The fastest of the three",
                "Word timestamps, so each sentence gets its own time",
                "6.19% average error on the Open ASR Leaderboard, the lowest of the three",
            ],
            cons: &[
                "No Tagalog, and no languages outside Europe",
                "A meeting that mixes Tagalog and English needs Qwen3-ASR or Whisper",
            ],
        },
        Model {
            name: "qwen3-asr-0.6b",
            display_name: "Qwen3-ASR 0.6B",
            archive: Archive {
                file: "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2",
                bytes: 878_702_423,
                sha256: "393f8a14e2f5fb96746aaab342997a40641001fbd5bf9592a080a8329178ee96",
                root: "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
            },
            layout: Layout::Qwen3,
            disk_bytes: 987_015_347,
            languages: QWEN3_ASR,
            // Each window is generated as one text with a capped token budget, and 30 seconds of
            // fast speech stays inside it.
            window_secs: 30,
            recommended: false,
            rank: 20,
            summary: "Alibaba's multilingual speech model for 30 languages, Filipino among them.",
            pros: &[
                "Understands Filipino (Tagalog) and English in one model",
                "Detects the spoken language by itself",
                "6.30% average error on the Open ASR Leaderboard",
            ],
            cons: &[
                "Filipino accuracy is unverified. One unofficial FLEURS run (whispernotes.app) measured 20.4% word error, against 9.4% for Whisper turbo",
                "No timestamps inside a window, so each window is one block of text",
            ],
        },
        Model {
            name: "whisper-turbo",
            display_name: "Whisper large-v3 turbo",
            archive: Archive {
                file: "sherpa-onnx-whisper-turbo.tar.bz2",
                bytes: 563_790_207,
                sha256: "b11acbbcd660b44a8e0df33724feb5aaa709cf65668f2823d59f656312544f22",
                root: "sherpa-onnx-whisper-turbo",
            },
            layout: Layout::WhisperTurbo,
            disk_bytes: 1_036_613_791,
            languages: WHISPER,
            // Whisper's encoder takes exactly 30 seconds of audio, and sherpa-onnx drops the rest.
            window_secs: 30,
            recommended: false,
            rank: 10,
            summary: "OpenAI's Whisper, with the broadest language list at 100 languages.",
            pros: &[
                "Covers the most languages, Tagalog included",
                "Detects the spoken language by itself",
            ],
            cons: &[
                "The slowest of the three",
                "Can repeat text over long silence",
                "7.69% average error on the Open ASR Leaderboard, the highest of the three",
                "No timestamps inside a window, so each window is one block of text",
            ],
        },
    ]
}

/// From the sherpa-onnx model card, which lists the 25 languages of nvidia/parakeet-tdt-0.6b-v3.
const PARAKEET_V3: &[(&str, &str)] = &[
    ("bg", "bg"),
    ("hr", "hr"),
    ("cs", "cs"),
    ("da", "da"),
    ("nl", "nl"),
    ("en", "en"),
    ("et", "et"),
    ("fi", "fi"),
    ("fr", "fr"),
    ("de", "de"),
    ("el", "el"),
    ("hu", "hu"),
    ("it", "it"),
    ("lv", "lv"),
    ("lt", "lt"),
    ("mt", "mt"),
    ("pl", "pl"),
    ("pt", "pt"),
    ("ro", "ro"),
    ("sk", "sk"),
    ("sl", "sl"),
    ("es", "es"),
    ("sv", "sv"),
    ("ru", "ru"),
    ("uk", "uk"),
];

/// The 30 languages in github.com/QwenLM/Qwen3-ASR README.md, with the names its prompt takes.
/// The README tags Filipino `fil`, and `fil` does not cover `tl`, so Tagalog is listed too.
const QWEN3_ASR: &[(&str, &str)] = &[
    ("zh", "Chinese"),
    ("en", "English"),
    ("yue", "Cantonese"),
    ("ar", "Arabic"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ko", "Korean"),
    ("ru", "Russian"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("ja", "Japanese"),
    ("tr", "Turkish"),
    ("hi", "Hindi"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("fil", "Filipino"),
    ("tl", "Filipino"),
    ("fa", "Persian"),
    ("el", "Greek"),
    ("hu", "Hungarian"),
    ("mk", "Macedonian"),
    ("ro", "Romanian"),
];

/// `LANGUAGES` in openai/whisper `whisper/tokenizer.py`: 99 languages plus Cantonese, which
/// large-v3 added. Whisper's own codes differ from BCP-47 for Javanese (`jw`), and `fil` and `nb`
/// are aliases for its `tl` and `no`.
const WHISPER: &[(&str, &str)] = &[
    ("en", "en"),
    ("zh", "zh"),
    ("de", "de"),
    ("es", "es"),
    ("ru", "ru"),
    ("ko", "ko"),
    ("fr", "fr"),
    ("ja", "ja"),
    ("pt", "pt"),
    ("tr", "tr"),
    ("pl", "pl"),
    ("ca", "ca"),
    ("nl", "nl"),
    ("ar", "ar"),
    ("sv", "sv"),
    ("it", "it"),
    ("id", "id"),
    ("hi", "hi"),
    ("fi", "fi"),
    ("vi", "vi"),
    ("he", "he"),
    ("uk", "uk"),
    ("el", "el"),
    ("ms", "ms"),
    ("cs", "cs"),
    ("ro", "ro"),
    ("da", "da"),
    ("hu", "hu"),
    ("ta", "ta"),
    ("no", "no"),
    ("nb", "no"),
    ("th", "th"),
    ("ur", "ur"),
    ("hr", "hr"),
    ("bg", "bg"),
    ("lt", "lt"),
    ("la", "la"),
    ("mi", "mi"),
    ("ml", "ml"),
    ("cy", "cy"),
    ("sk", "sk"),
    ("te", "te"),
    ("fa", "fa"),
    ("lv", "lv"),
    ("bn", "bn"),
    ("sr", "sr"),
    ("az", "az"),
    ("sl", "sl"),
    ("kn", "kn"),
    ("et", "et"),
    ("mk", "mk"),
    ("br", "br"),
    ("eu", "eu"),
    ("is", "is"),
    ("hy", "hy"),
    ("ne", "ne"),
    ("mn", "mn"),
    ("bs", "bs"),
    ("kk", "kk"),
    ("sq", "sq"),
    ("sw", "sw"),
    ("gl", "gl"),
    ("mr", "mr"),
    ("pa", "pa"),
    ("si", "si"),
    ("km", "km"),
    ("sn", "sn"),
    ("yo", "yo"),
    ("so", "so"),
    ("af", "af"),
    ("oc", "oc"),
    ("ka", "ka"),
    ("be", "be"),
    ("tg", "tg"),
    ("sd", "sd"),
    ("gu", "gu"),
    ("am", "am"),
    ("yi", "yi"),
    ("lo", "lo"),
    ("uz", "uz"),
    ("fo", "fo"),
    ("ht", "ht"),
    ("ps", "ps"),
    ("tk", "tk"),
    ("nn", "nn"),
    ("mt", "mt"),
    ("sa", "sa"),
    ("lb", "lb"),
    ("my", "my"),
    ("bo", "bo"),
    ("tl", "tl"),
    ("fil", "tl"),
    ("mg", "mg"),
    ("as", "as"),
    ("tt", "tt"),
    ("haw", "haw"),
    ("ln", "ln"),
    ("ha", "ha"),
    ("ba", "ba"),
    ("jv", "jw"),
    ("su", "su"),
    ("yue", "yue"),
];
