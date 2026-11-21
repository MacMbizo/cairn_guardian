//! Drive roles, the drive plan and moving copies onto the Backup drive (spec §7.6).

use crate::db::SqliteStore;
use crate::error::{AegisError, Result};

/// Proposes moving the copies at `location_index` of a duplicate group onto the Backup drive.
pub fn propose_move_to_backup(_store: &mut SqliteStore, _group_id: &str, _location_index: usize) -> Result<String> {
    Err(AegisError::UnsupportedFs("moving to the Backup drive arrives in Task 9".into()))
}
