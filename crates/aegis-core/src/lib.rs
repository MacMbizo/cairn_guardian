//! Aegis Guardian core engine (`B4 v2.3.1`, `C1 v1.2`, `C4 v1.1`, `E4 v1.2`).

pub mod contracts;
pub mod db;
pub mod doctor;
pub mod drives;
pub mod error;
pub mod policy;
pub mod quarantine;
pub mod scanner;

use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Current UTC epoch time in milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Generates a time-ordered UUIDv7 string identifier.
pub fn new_id() -> String {
    Uuid::now_v7().to_string()
}
