//! Reminders: findings that reach the owner gently (spec §5).

use super::Finding;
use crate::error::Result;
use rusqlite::Connection;

/// Stores a watch's findings as reminders (filled in by the reminders task).
pub fn sync_findings(_conn: &Connection, _kinds: &[&str], _findings: &[Finding], _now: i64) -> Result<()> {
    Ok(())
}
