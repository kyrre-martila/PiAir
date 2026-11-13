//! Presentation metadata only. Original errors remain in the anyhow chain;
//! transport decisions and cleanup behavior do not depend on these labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Discovery,
    Connection,
    Playback,
}
impl Stage {
    pub fn message(self) -> &'static str {
        match self {
            Self::Discovery => "PiAir couldn't find or validate your selected speaker.\nCheck that it is powered on and connected to the same network, then try again. If it changed, select it again.",
            Self::Connection => "PiAir found the speaker but couldn't connect.\nCheck the connection method and try again.",
            Self::Playback => "PiAir connected to the speaker but couldn't play the test audio.\nCheck the speaker and network, then try the test again.",
        }
    }
}
impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for Stage {}
