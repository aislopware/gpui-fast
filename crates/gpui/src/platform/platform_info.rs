//! Runtime identity of the host operating system.
//!
//! Backends provide this identity; consumers read it through the application.

/// The host operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlatformInfo {
    /// The operating system whose input conventions the application follows.
    pub os: OperatingSystem,
}

/// An operating system whose input conventions keystrokes can be parsed and shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperatingSystem {
    /// Apple's desktop operating system.
    Mac,
    /// Microsoft Windows.
    Windows,
    /// Linux.
    Linux,
    /// FreeBSD.
    FreeBsd,
    /// iOS and iPadOS.
    Ios,
    /// Android.
    Android,
    /// The backend could not identify the host operating system.
    Unknown,
}

impl PlatformInfo {
    /// Whether the host operating system is macOS.
    pub fn is_mac(self) -> bool {
        self.os == OperatingSystem::Mac
    }
}
