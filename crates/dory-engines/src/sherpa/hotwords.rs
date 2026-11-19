//! Hotwords for the transducer models: the phrase string sherpa-onnx takes per stream, and the
//! BPE vocabulary it needs to encode them.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use dory_core::domain::{Boost, VocabularyTerm};
use dory_core::ports::EngineError;

/// The recognizer-wide hotword score. On Two hour planning clips, 2.0 fixed "Lena" to "Lina" 4 of
/// 4 times and changed no other word.
pub(super) const NORMAL_SCORE: f32 = 2.0;
/// A per-phrase score for a `Strong` term. On the same clips, 6 fixed "Kria" to "Priya" 3 of 4
/// times, and 12 turned "Friday" into "Primary".
pub(super) const STRONG_SCORE: f32 = 6.0;

/// The hotwords of one stream: phrases joined by `/`, and ` :SCORE` after a phrase that does not
/// take the recognizer's score. A term with a NUL is dropped, since it cannot cross the C
/// boundary.
pub(super) fn hotwords_string(terms: &[VocabularyTerm]) -> String {
    terms
        .iter()
        .filter(|t| !t.text.contains('\0'))
        .map(|t| match t.boost {
            Boost::Normal => t.text.clone(),
            Boost::Strong => format!("{} :{STRONG_SCORE}", t.text),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Beside the model directory, not inside it: provisioning replaces that directory wholesale.
pub(super) fn bpe_vocab_path(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(|| "model".to_owned(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!("{name}.bpe.vocab"))
}

/// What sherpa-onnx reads as a BPE vocabulary, made from a model's `tokens.txt`: each line's
/// token with every score -1.0, as sherpa-onnx's own Parakeet hotword example does.
fn bpe_vocab(tokens: &str) -> String {
    tokens
        .lines()
        .filter_map(|line| line.rsplit_once(' '))
        .map(|(token, _id)| format!("{token}\t-1.0\n"))
        .collect()
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> EngineError {
    EngineError::Other {
        message: format!("{what} {}: {e}", path.display()),
        source: Some(Box::new(e)),
    }
}

/// The BPE vocabulary for the model in `dir`, written on first use. It goes to a temporary file
/// first and is renamed into place, so a crash never leaves half a vocabulary.
pub(super) fn ensure_bpe_vocab(dir: &Path) -> Result<PathBuf, EngineError> {
    let path = bpe_vocab_path(dir);
    if path.is_file() {
        return Ok(path);
    }
    let tokens_path = dir.join("tokens.txt");
    let tokens =
        fs::read_to_string(&tokens_path).map_err(|e| io_error("cannot read", &tokens_path, e))?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = fs::File::create(&temp)
        .and_then(|mut f| f.write_all(bpe_vocab(&tokens).as_bytes()))
        .and_then(|()| fs::rename(&temp, &path));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(io_error("cannot write", &path, e));
    }
    Ok(path)
}
