//! Non-admin drive SMART & storage reliability monitoring (`R1` user-mode only) combined with
//! Aegis's catalog read-error counts per volume.

use super::DriveHealthItem;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct RawDisk {
    #[serde(alias = "DeviceId", alias = "Number")]
    device_id: serde_json::Value,
    #[serde(alias = "FriendlyName")]
    friendly_name: Option<String>,
    #[serde(alias = "MediaType")]
    media_type: Option<serde_json::Value>,
    #[serde(alias = "BusType")]
    bus_type: Option<serde_json::Value>,
    #[serde(alias = "Size")]
    size: Option<serde_json::Value>,
    #[serde(alias = "HealthStatus")]
    health_status: Option<serde_json::Value>,
    #[serde(alias = "OperationalStatus")]
    operational_status: Option<serde_json::Value>,
    #[serde(alias = "DriveLetters")]
    drive_letters: Option<Vec<String>>,
    #[serde(alias = "Temperature")]
    temperature: Option<serde_json::Value>,
    #[serde(alias = "Wear")]
    wear: Option<serde_json::Value>,
    #[serde(alias = "PowerOnHours")]
    power_on_hours: Option<serde_json::Value>,
    #[serde(alias = "ReadErrorsTotal")]
    read_errors_total: Option<serde_json::Value>,
    #[serde(alias = "WriteErrorsTotal")]
    write_errors_total: Option<serde_json::Value>,
    #[serde(alias = "ReliabilityError")]
    reliability_error: Option<String>,
}

fn val_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn val_to_i64(v: Option<&serde_json::Value>) -> Option<i64> {
    let v = v?;
    match v {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn val_to_u64(v: Option<&serde_json::Value>) -> Option<u64> {
    val_to_i64(v).and_then(|n| u64::try_from(n).ok())
}

fn translate_media_type(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(3) => "HDD".into(),
            Some(4) => "SSD".into(),
            Some(5) => "SCM".into(),
            _ => "Unspecified".into(),
        },
        _ => "Unknown".into(),
    }
}

fn translate_bus_type(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(1) => "SCSI".into(),
            Some(3) => "ATA".into(),
            Some(7) => "USB".into(),
            Some(11) => "SATA".into(),
            Some(12) => "SD".into(),
            Some(14) => "Virtual".into(),
            Some(17) => "NVMe".into(),
            Some(other) => format!("Bus({other})"),
            None => "Unknown".into(),
        },
        _ => "Unknown".into(),
    }
}

fn translate_health_status(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(0) => "Healthy".into(),
            Some(1) => "Warning".into(),
            Some(2) => "Unhealthy".into(),
            _ => "Unknown".into(),
        },
        _ => "Unknown".into(),
    }
}

fn op_code_label(code: u64) -> &'static str {
    match code {
        2 => "OK",
        3 => "Degraded",
        5 => "Predictive Failure",
        6 => "Error",
        7 => "Non-Recoverable Error",
        10 => "Stopped",
        0xD011 => "Lost Communication",
        _ => "Other",
    }
}

fn translate_operational_status(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.as_u64().map(op_code_label).unwrap_or("Unknown").into(),
        Some(serde_json::Value::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|x| match x {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Number(n) => n.as_u64().map(|c| op_code_label(c).to_string()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() { "OK".into() } else { parts.join(", ") }
        }
        _ => "OK".into(),
    }
}

/// Parses JSON produced by Windows Storage CIM / `Get-PhysicalDisk` into `DriveHealthItem` records.
pub fn parse_smart_json(raw: &str, catalog_errors: &HashMap<String, usize>) -> Vec<DriveHealthItem> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let list: Vec<RawDisk> = match serde_json::from_str::<Vec<RawDisk>>(trimmed) {
        Ok(v) => v,
        Err(_) => match serde_json::from_str::<RawDisk>(trimmed) {
            Ok(one) => vec![one],
            Err(_) => return Vec::new(),
        },
    };

    let mut out = Vec::with_capacity(list.len());
    for d in list {
        let letters: Vec<String> = d
            .drive_letters
            .unwrap_or_default()
            .into_iter()
            .map(|l| {
                let c = l.trim().trim_end_matches([':', '\\', '/']).to_uppercase();
                if c.len() == 1 { format!("{c}:") } else { l.trim().to_string() }
            })
            .filter(|l| !l.is_empty())
            .collect();

        let catalog_read_errors: usize = letters
            .iter()
            .filter_map(|l| catalog_errors.get(&l.to_lowercase()).copied())
            .sum();

        let temperature_c = val_to_i64(d.temperature.as_ref()).and_then(|v| i32::try_from(v).ok());
        let wear_pct = val_to_u64(d.wear.as_ref()).and_then(|v| u32::try_from(v).ok());
        let power_on_hours = val_to_u64(d.power_on_hours.as_ref());
        let read_errors = val_to_u64(d.read_errors_total.as_ref());
        let write_errors = val_to_u64(d.write_errors_total.as_ref());

        let smart_detail_available = temperature_c.is_some()
            || wear_pct.is_some()
            || power_on_hours.is_some()
            || read_errors.is_some()
            || write_errors.is_some();

        let bus_type = translate_bus_type(d.bus_type.as_ref());
        let smart_note = if smart_detail_available {
            None
        } else if let Some(err) = d.reliability_error.filter(|s| !s.trim().is_empty()) {
            Some(format!(
                "Windows Storage health is active; detailed SMART counters require administrator permission or direct controller passthrough ({err})."
            ))
        } else if bus_type.eq_ignore_ascii_case("USB") {
            Some("USB enclosure health only (bridge may not pass raw SMART attributes without admin).".into())
        } else {
            Some("Windows Storage overall health active; raw vendor SMART counters unavailable in user mode (R1).".into())
        };

        out.push(DriveHealthItem {
            device_id: val_to_string(&d.device_id),
            model: d
                .friendly_name
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Physical Disk".into()),
            media_type: translate_media_type(d.media_type.as_ref()),
            bus_type,
            size_bytes: val_to_i64(d.size.as_ref()).unwrap_or(0),
            health_status: translate_health_status(d.health_status.as_ref()),
            operational_status: translate_operational_status(d.operational_status.as_ref()),
            drive_letters: letters,
            temperature_c,
            wear_pct,
            power_on_hours,
            read_errors,
            write_errors,
            smart_detail_available,
            smart_note,
            catalog_read_errors,
        });
    }
    out
}

/// Counts non-`permission_denied` read errors from the latest completed scan per online volume.
pub fn catalog_errors_by_letter(conn: &Connection) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    let Ok(mut st) = conn.prepare("SELECT mount_point FROM volumes WHERE status = 'online'") else {
        return map;
    };
    let Ok(mounts) = st.query_map([], |r| r.get::<_, String>(0)) else {
        return map;
    };
    for m in mounts.flatten() {
        let errors: Option<String> = conn
            .query_row(
                "SELECT errors_json FROM scan_jobs
                 WHERE lower(root_path) = lower(?1) AND finished_at IS NOT NULL
                 ORDER BY finished_at DESC LIMIT 1",
                [&m],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let n = errors
            .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(&j).ok())
            .map_or(0, |v| {
                v.iter()
                    .filter(|e| e.get("code").and_then(|c| c.as_str()) != Some("permission_denied"))
                    .count()
            });
        let letter = m.trim_end_matches(['\\', '/']).to_lowercase();
        if !letter.is_empty() {
            map.insert(letter, n);
        }
    }
    map
}

#[cfg(windows)]
fn query_windows_storage_json() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$parts = @{}
foreach ($p in Get-CimInstance -Namespace root\Microsoft\Windows\Storage -ClassName MSFT_Partition) {
  $lt = ([string]$p.DriveLetter).Trim([char]0)
  if ($lt -match '^[A-Za-z]$') {
    $dn = [string]$p.DiskNumber
    if (-not $parts.ContainsKey($dn)) { $parts[$dn] = @() }
    $parts[$dn] += ($lt.ToUpper() + ':')
  }
}
$disks = foreach ($d in Get-PhysicalDisk) {
  $c = $null
  $err = $null
  try { $c = $d | Get-StorageReliabilityCounter -ErrorAction Stop } catch { $err = $_.Exception.Message }
  $dn = [string]$d.DeviceId
  [PSCustomObject]@{
    DeviceId = $dn
    FriendlyName = [string]$d.FriendlyName
    MediaType = [string]$d.MediaType
    BusType = [string]$d.BusType
    Size = [int64]$d.Size
    HealthStatus = [string]$d.HealthStatus
    OperationalStatus = [string]$d.OperationalStatus
    DriveLetters = $(if ($parts.ContainsKey($dn)) { @($parts[$dn]) } else { @() })
    Temperature = if ($c) { $c.Temperature } else { $null }
    Wear = if ($c) { $c.Wear } else { $null }
    PowerOnHours = if ($c) { $c.PowerOnHours } else { $null }
    ReadErrorsTotal = if ($c) { $c.ReadErrorsTotal } else { $null }
    WriteErrorsTotal = if ($c) { $c.WriteErrorsTotal } else { $null }
    ReliabilityError = $err
  }
}
$disks | ConvertTo-Json -Compress -Depth 4
"#;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Probes local drives without administrator elevation (`R1`), falling back cleanly to catalog-only
/// status if CIM/PowerShell is unavailable.
pub fn probe_drive_health(conn: &Connection) -> Vec<DriveHealthItem> {
    let errors = catalog_errors_by_letter(conn);
    #[cfg(windows)]
    if let Some(raw) = query_windows_storage_json() {
        let parsed = parse_smart_json(&raw, &errors);
        if !parsed.is_empty() {
            return parsed;
        }
    }

    let Ok(mut st) = conn.prepare(
        "SELECT id, COALESCE(label, mount_point), mount_point FROM volumes WHERE status = 'online' ORDER BY mount_point",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .map(|(id, label, mount)| {
            let letter = mount.trim_end_matches(['\\', '/']).to_string();
            let err_n = errors.get(&letter.to_lowercase()).copied().unwrap_or(0);
            DriveHealthItem {
                device_id: id,
                model: label,
                media_type: "Unknown".into(),
                bus_type: "Unknown".into(),
                size_bytes: 0,
                health_status: if err_n >= 10 { "Warning".into() } else { "Healthy".into() },
                operational_status: "OK".into(),
                drive_letters: vec![letter],
                temperature_c: None,
                wear_pct: None,
                power_on_hours: None,
                read_errors: None,
                write_errors: None,
                smart_detail_available: false,
                smart_note: Some("Telemetry from Aegis catalog only.".into()),
                catalog_read_errors: err_n,
            }
        })
        .collect()
}
