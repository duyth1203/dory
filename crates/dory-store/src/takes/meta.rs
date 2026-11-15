//! Tiny `key=value` sidecar files next to the audio. The store holds facts about a take, but
//! `Takes::clip` receives only an id, so the layout (which channel is the mic) has to live beside
//! the samples. The same format holds the pointer to a legacy audio file.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dory_core::domain::ChannelLayout;

use crate::sqlite::codec::{layout_from_db, layout_to_db};

pub(crate) fn encode(pairs: &[(&str, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

pub(crate) fn decode(text: &str) -> BTreeMap<&str, &str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .collect()
}

pub(crate) fn layout_text(layout: ChannelLayout) -> String {
    encode(&[("layout", layout_to_db(layout).to_owned())])
}

pub(crate) fn layout_from_text(text: &str) -> Option<ChannelLayout> {
    layout_from_db(decode(text).get("layout")?).ok()
}

/// Writes `contents` so a reader sees the old file or the whole new one, never half of it.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_data()?;
    }
    std::fs::rename(&tmp, path)
}
