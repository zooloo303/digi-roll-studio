//! Process-wide storage identity, selected before constructing application services.
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeProfile {
    #[default]
    Stable,
    PluginPreview,
}

static PROFILE: OnceLock<RuntimeProfile> = OnceLock::new();

impl RuntimeProfile {
    /// Select once at startup, before opening recovery, caches or backups.
    pub fn install(self) -> Result<(), Self> {
        PROFILE.set(self)
    }
    pub fn current() -> Self {
        *PROFILE.get_or_init(Self::default)
    }
    pub fn directory_name(self) -> &'static str {
        match self {
            Self::Stable => "digi-roll-studio",
            Self::PluginPreview => "digi-roll-studio-plugin-preview",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Stable => "Digi Roll Studio",
            Self::PluginPreview => "Digi-Roll Studio — Plugin Preview",
        }
    }
    pub fn under(self, platform_data: &Path) -> PathBuf {
        platform_data.join(self.directory_name())
    }
    pub fn hardware_autoconnect(self) -> bool {
        self == Self::Stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_storage_and_port_policy_are_isolated() {
        let base = Path::new("/example/application-data");
        let stable = RuntimeProfile::Stable.under(base);
        let preview = RuntimeProfile::PluginPreview.under(base);
        assert_eq!(stable, base.join("digi-roll-studio"));
        assert_eq!(preview, base.join("digi-roll-studio-plugin-preview"));
        for child in [
            "settings",
            "recovery",
            "backups",
            "preset-index",
            "logs",
            "ipc",
        ] {
            assert_ne!(stable.join(child), preview.join(child));
        }
        assert!(RuntimeProfile::Stable.hardware_autoconnect());
        assert!(!RuntimeProfile::PluginPreview.hardware_autoconnect());
    }
}
