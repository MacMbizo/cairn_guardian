//! Drive identity, passport and online lifecycle (Drives & Duplicates spec §7.1–§7.4).

pub mod passport;
pub mod provider;
pub mod refresh;
pub mod registry;
pub mod usn;

pub use passport::{Passport, read_passport, regenerate_passport, write_passport};
#[cfg(windows)]
pub use provider::WindowsProvider;
pub use provider::{FakeVolumeProvider, MountedVolume, VolumeProvider};
pub use registry::{CloneConflict, SyncReport, rebase_volume_paths, sync_mounted};
pub use refresh::{RefreshMode, RefreshReport, refresh_volume};
