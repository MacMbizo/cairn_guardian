//! Background SMART / storage reliability watch (`K5`, `R1`). Emits an urgent reminder ONLY
//! when Windows Storage reports warning/unhealthy status, predictive failure, >= 70 C, or >= 90% wear.

use super::DriveHealthItem;
use crate::db::SqliteStore;
use crate::engine::{Finding, FindingAction, Level, Screen, Watch};
use crate::error::Result;
use rusqlite::Connection;

const SMART_WARN_TEMP_C: i32 = 70;
const SMART_WARN_WEAR_PCT: u32 = 90;

pub type DriveProbe = Box<dyn Fn(&Connection) -> Vec<DriveHealthItem> + Send>;

pub struct SmartHealthWatch {
    pub probe: DriveProbe,
}

impl Default for SmartHealthWatch {
    fn default() -> Self {
        Self {
            probe: Box::new(super::smart::probe_drive_health),
        }
    }
}

pub fn drive_smart_alert_reason(d: &DriveHealthItem) -> Option<String> {
    let health_low = d.health_status.to_ascii_lowercase();
    let op_low = d.operational_status.to_ascii_lowercase();
    if matches!(health_low.as_str(), "warning" | "unhealthy") {
        return Some(format!(
            "Windows reports {} status ({}) — copy irreplaceable files soon.",
            d.health_status, d.operational_status
        ));
    }
    if op_low.contains("predictive failure") || op_low.contains("degraded") || op_low.contains("error") {
        return Some(format!(
            "Drive operational state is '{}' — check System Care → Drive Health.",
            d.operational_status
        ));
    }
    if let Some(temp) = d.temperature_c
        && temp >= SMART_WARN_TEMP_C
    {
        return Some(format!("High drive temperature ({temp}°C). Ensure cooling/airflow and verify backups."));
    }
    if let Some(wear) = d.wear_pct
        && wear >= SMART_WARN_WEAR_PCT
    {
        return Some(format!("SSD wear reached {wear}% — plan drive replacement and verify backups."));
    }
    None
}

impl Watch for SmartHealthWatch {
    fn name(&self) -> &'static str {
        "smart_health"
    }
    fn every_ms(&self) -> i64 {
        6 * 3_600_000
    }
    fn needs_idle(&self) -> bool {
        false
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["drive_smart"]
    }
    fn run(&mut self, store: &mut SqliteStore, _now: i64) -> Result<Vec<Finding>> {
        let items = (self.probe)(store.conn());
        let mut out = Vec::new();
        for d in &items {
            if let Some(detail) = drive_smart_alert_reason(d) {
                let letters = if d.drive_letters.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", d.drive_letters.join(", "))
                };
                out.push(Finding {
                    kind: "drive_smart".into(),
                    subject: d.device_id.clone(),
                    level: Level::Now,
                    title: format!("{}{} reports a drive health warning", d.model, letters),
                    detail,
                    bytes: None,
                    action: FindingAction::Open { screen: Screen::Care },
                });
            }
        }
        Ok(out)
    }
}
