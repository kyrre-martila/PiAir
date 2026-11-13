//! Shared data is external to the executable; no network retrieval.
use std::path::{Path, PathBuf};
const WAV: &str = "Pachelbel_Canon_audio_test.wav";
pub(super) fn locate_in(directories: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    directories.into_iter().find_map(|dir| {
        let wav = dir.join(WAV);
        (wav.is_file() && dir.join("LICENSE.md").is_file()).then_some(wav)
    })
}
pub(super) fn locate() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PIAIR_AUDIO_DATA_DIR") {
        // Explicit override is authoritative, including when missing.
        return locate_in([dir.into()]);
    }
    let mut dirs = vec![
        PathBuf::from("/usr/local/share/piair/audio"),
        PathBuf::from("/usr/share/piair/audio"),
    ];
    // Development checkout; never embed its data in the binary.
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets/audio"));
    locate_in(dirs)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_data_or_attribution_is_skippable() {
        let d = tempfile::tempdir().unwrap();
        assert!(locate_in([d.path().into()]).is_none());
        std::fs::write(d.path().join(WAV), b"wav").unwrap();
        assert!(locate_in([d.path().into()]).is_none());
        std::fs::write(d.path().join("LICENSE.md"), b"license").unwrap();
        assert_eq!(locate_in([d.path().into()]), Some(d.path().join(WAV)));
    }
}
