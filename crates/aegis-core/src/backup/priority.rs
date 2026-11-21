//! Backup priority tiers and filling the Backup drive in order (spec §7.7.2). Filled in by Task 13.

use crate::error::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TierShortfall {
    #[ts(type = "number")]
    pub tier: i64,
    #[ts(type = "number")]
    pub bytes_needed: i64,
    #[ts(type = "number")]
    pub bytes_skipped: i64,
}

/// Backup tier of a file (1 = most irreplaceable … 3), or `None` when it is never backed up.
pub fn tier_for_file(_conn: &Connection, _file_id: &str) -> Result<Option<i64>> {
    Ok(Some(2))
}
