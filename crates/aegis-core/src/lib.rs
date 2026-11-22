//! Aegis Guardian core engine (`B4 v2.3.1`, `C1 v1.2`, `C4 v1.1`, `E4 v1.2`).

pub mod backup;
pub mod context;
pub mod contracts;
pub mod db;
pub mod doctor;
pub mod drives;
pub mod dupes;
pub mod engine;
pub mod error;
pub mod filing;
pub mod fingerprint;
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

/// Formats a UTC epoch-milliseconds timestamp as `YYYY-MM-DD` (civil calendar).
pub fn format_date_ms(ms: i64) -> String {
    // Howard Hinnant's days-to-civil algorithm.
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Generates a time-ordered UUIDv7 string identifier.
pub fn new_id() -> String {
    Uuid::now_v7().to_string()
}
