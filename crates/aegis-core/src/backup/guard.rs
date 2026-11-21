//! Mass-change / ransomware guard for backup top-ups (spec §7.7.1). Filled in by Task 12.

use super::topup::TopUpPlan;
use crate::error::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A top-up halts for a folder when more than this share of its backed-up files would be
/// overwritten or retired at once.
pub const MASS_CHANGE_FRACTION: f64 = 0.20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GuardHalt {
    pub source_folder: String,
    #[ts(type = "number")]
    pub changed: i64,
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub suspect_encrypted: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GuardVerdict {
    Proceed,
    Halt(Vec<GuardHalt>),
}

pub fn check(_conn: &Connection, _plan: &TopUpPlan) -> Result<GuardVerdict> {
    Ok(GuardVerdict::Proceed)
}
