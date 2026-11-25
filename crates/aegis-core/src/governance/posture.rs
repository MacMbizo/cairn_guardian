//! Local Privacy Posture & Drive Encryption Telemetry (`D1`, `R1`, `R2`, `R7`, `R8`, `R9`, `R11`).

use rusqlite::{Connection, OptionalExtension};

use super::{DriveEncryptionStatus, OptInCapabilityAudit, PrivacyPostureReport};

/// Context passed from desktop vault/lock state when building the Privacy Posture dashboard.
#[derive(Debug, Clone, Default)]
pub struct VaultPostureContext {
    pub db_encrypted: bool,
    pub cipher_label: String,
    pub dpapi_sealed: bool,
    pub recovery_confirmed: bool,
    pub sensitive_unlocked: bool,
    pub sensitive_expires_at: Option<i64>,
}

const ZERO_NETWORK_STATUS: &str = "Code-enforced offline engine: Cairn's engine, search-embedding and local-language-model components depend on zero networking crates, verified automatically by `r9_forbids_networking_crates_in_engine_and_models`.";
/// A PowerShell rule blocking outbound traffic for the running Cairn executable.
fn firewall_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| r"$env:LOCALAPPDATA\Cairn Guardian\cairn-guardian.exe".into());
    format!(r#"New-NetFirewallRule -DisplayName "Cairn Guardian Block Outbound" -Direction Outbound -Program "{exe}" -Action Block"#)
}
const SHREDDING_POLICY: &str = "Multi-pass shredding disabled: SSD wear-leveling prevents deterministic overwrites and breaks 30-day quarantine undo. Use Windows BitLocker for data at rest; Cairn purge performs clean normal OS unlinking.";

/// Probes drive BitLocker protection status using Windows Shell non-admin properties (`R1`),
/// gracefully falling back to `unknown_requires_admin` without requesting UAC elevation.
#[cfg(windows)]
pub fn probe_windows_bitlocker_status(mounts: &[String]) -> Vec<DriveEncryptionStatus> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$shell = New-Object -ComObject Shell.Application
$out = foreach ($d in Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DriveType=3 OR DriveType=2") {
  $mp = ($d.DeviceID.TrimEnd('\') + '\')
  $code = $null
  try { $code = $shell.NameSpace($mp).Self.ExtendedProperty('System.Volume.BitLockerProtection') } catch {}
  [PSCustomObject]@{
    Mount = $mp
    Label = [string]$d.VolumeName
    Code = $code
  }
}
$out | ConvertTo-Json -Compress -Depth 3
"#;

    let parsed: Option<Vec<(String, String, Option<i64>)>> = (|| {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if raw.is_empty() {
            return None;
        }
        let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let arr = match val {
            serde_json::Value::Array(a) => a,
            obj @ serde_json::Value::Object(_) => vec![obj],
            _ => return None,
        };
        let mut rows = Vec::new();
        for item in arr {
            let m = item
                .get("Mount")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let l = item
                .get("Label")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let c = item.get("Code").and_then(|v| v.as_i64());
            if !m.is_empty() {
                rows.push((m, l, c));
            }
        }
        Some(rows)
    })();

    let map_status = |code: Option<i64>| -> (&'static str, &'static str, String) {
        match code {
            Some(1 | 6) => (
                "protected",
                "Windows BitLocker (Enabled)",
                "Volume is encrypted at rest with BitLocker.".into(),
            ),
            Some(3) => (
                "encrypting",
                "Windows BitLocker (Encrypting)",
                "BitLocker encryption is in progress.".into(),
            ),
            Some(2 | 5) => (
                "suspended",
                "Windows BitLocker (Suspended)",
                "Resume BitLocker protection in Windows Settings -> Device encryption.".into(),
            ),
            Some(0) => (
                "unencrypted",
                "Unencrypted NTFS/exFAT",
                "Enable BitLocker / Device Encryption in Windows Settings for at-rest protection.".into(),
            ),
            _ => (
                "unknown_requires_admin",
                "Standard OS volume (non-admin probe)",
                "Check Windows Settings -> Privacy & security -> Device encryption to verify BitLocker.".into(),
            ),
        }
    };

    if let Some(rows) = parsed {
        return mounts
            .iter()
            .map(|m| {
                let norm = m.trim_end_matches(['\\', '/']).to_ascii_lowercase();
                let found = rows.iter().find(|(rm, _, _)| {
                    rm.trim_end_matches(['\\', '/']).to_ascii_lowercase() == norm
                });
                let (label, code) = match found {
                    Some((_, lbl, c)) => (lbl.clone(), *c),
                    None => (String::new(), None),
                };
                let (st, method, rec) = map_status(code);
                DriveEncryptionStatus {
                    mount_point: m.clone(),
                    label,
                    protection_status: st.into(),
                    encryption_method: method.into(),
                    recommended_action: rec,
                }
            })
            .collect();
    }

    fallback_drive_statuses(mounts)
}

#[cfg(not(windows))]
pub fn probe_windows_bitlocker_status(mounts: &[String]) -> Vec<DriveEncryptionStatus> {
    fallback_drive_statuses(mounts)
}

fn fallback_drive_statuses(mounts: &[String]) -> Vec<DriveEncryptionStatus> {
    mounts
        .iter()
        .map(|m| DriveEncryptionStatus {
            mount_point: m.clone(),
            label: String::new(),
            protection_status: "unknown_requires_admin".into(),
            encryption_method: "Non-admin user check".into(),
            recommended_action:
                "Open Windows Settings -> Privacy & security -> Device encryption to verify BitLocker.".into(),
        })
        .collect()
}

pub fn build_privacy_posture(
    conn: &Connection,
    ctx: &VaultPostureContext,
    live_probe_bitlocker: bool,
) -> crate::error::Result<PrivacyPostureReport> {
    let mounts: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT mount_point, COALESCE(label, mount_point) FROM volumes WHERE status = 'online' ORDER BY mount_point ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out
    };

    let drive_list: Vec<String> = mounts.iter().map(|(m, _)| m.clone()).collect();
    let mut drives_encryption = if live_probe_bitlocker {
        probe_windows_bitlocker_status(&drive_list)
    } else {
        fallback_drive_statuses(&drive_list)
    };
    for (idx, (_, label)) in mounts.iter().enumerate() {
        if drives_encryption[idx].label.is_empty() {
            drives_encryption[idx].label = label.clone();
        }
    }

    build_privacy_posture_with_drives(conn, ctx, drives_encryption)
}

/// Pure deterministic builder used by both live dashboard & integration tests (`R1`, `R8`, `R9`, `R11`).
pub fn build_privacy_posture_with_drives(
    conn: &Connection,
    ctx: &VaultPostureContext,
    drives_encryption: Vec<DriveEncryptionStatus>,
) -> crate::error::Result<PrivacyPostureReport> {
    let open_findings_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM security_findings WHERE status = 'open'",
        [],
        |r| r.get(0),
    )?;
    let vaulted_findings_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM security_findings WHERE status = 'vaulted'",
        [],
        |r| r.get(0),
    )?;
    let confidential_files_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND state NOT IN ('quarantine', 'missing') AND sensitivity = 'confidential'",
        [],
        |r| r.get(0),
    )?;
    let secret_files_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND state NOT IN ('quarantine', 'missing') AND sensitivity = 'secret'",
        [],
        |r| r.get(0),
    )?;
    let active_rules_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM governance_privacy_rules WHERE enabled = 1",
        [],
        |r| r.get(0),
    )?;
    let quarantined_items_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM quarantine_entries WHERE restored_at IS NULL AND purged_at IS NULL",
        [],
        |r| r.get(0),
    )?;

    let audio_opt_in: bool = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = 'knowledge.audio_opt_in' AND scope = 'global'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|j| serde_json::from_str::<bool>(&j).ok())
        .unwrap_or(false);

    let opt_in_audits = vec![
        OptInCapabilityAudit {
            id: "screen_capture".into(),
            label: "Screen Capture & Active Window Watcher".into(),
            state: "permanently_disabled".into(),
            rule_ref: "R11".into(),
            detail: "Never records screen pixels, active window titles, or desktop focus.".into(),
        },
        OptInCapabilityAudit {
            id: "keystroke_logging".into(),
            label: "Keystroke / Clipboard Observation".into(),
            state: "permanently_disabled".into(),
            rule_ref: "R11".into(),
            detail: "No keyboard hooks or clipboard capture; Librarian & Cross-Project read disk files only.".into(),
        },
        OptInCapabilityAudit {
            id: "browser_history".into(),
            label: "Browser History Ingestion".into(),
            state: "off".into(),
            rule_ref: "K4".into(),
            detail: "Off — Cairn never reads external browser history or profile cookies.".into(),
        },
        OptInCapabilityAudit {
            id: "audio_transcripts".into(),
            label: "Audio Companion Transcripts".into(),
            state: if audio_opt_in { "opted_in" } else { "off" }.into(),
            rule_ref: "K4".into(),
            detail: if audio_opt_in {
                "Owner-enabled: pairs local `.vtt`/`.srt` companion transcripts on Knowledge scan."
            } else {
                "Off by default until enabled in Knowledge Base."
            }
            .into(),
        },
        OptInCapabilityAudit {
            id: "backup_isolation".into(),
            label: "Backup Volume Content Exclusion".into(),
            state: "enforced".into(),
            rule_ref: "K4".into(),
            detail: "Files on `backup`-role volumes are excluded from reading, AI prompts, and PII scans.".into(),
        },
    ];

    let mut score = 100.0f64;
    if !ctx.db_encrypted {
        score -= 35.0;
    }
    if !ctx.dpapi_sealed {
        score -= 10.0;
    }
    if !ctx.recovery_confirmed {
        score -= 10.0;
    }
    let unencrypted_drives = drives_encryption
        .iter()
        .filter(|d| d.protection_status == "unencrypted" || d.protection_status == "suspended")
        .count();
    score -= ((unencrypted_drives as f64) * 8.0).min(16.0);
    score -= ((open_findings_count as f64) * 5.0).min(30.0);
    let privacy_score = (score.clamp(10.0, 100.0) * 10.0).round() / 10.0;

    Ok(PrivacyPostureReport {
        privacy_score,
        db_encrypted: ctx.db_encrypted,
        cipher_label: if ctx.cipher_label.is_empty() {
            "SQLCipher AES-256".into()
        } else {
            ctx.cipher_label.clone()
        },
        dpapi_sealed: ctx.dpapi_sealed,
        recovery_confirmed: ctx.recovery_confirmed,
        sensitive_unlocked: ctx.sensitive_unlocked,
        sensitive_expires_at: ctx.sensitive_expires_at,
        zero_network_status: ZERO_NETWORK_STATUS.into(),
        firewall_command: firewall_command(),
        shredding_policy: SHREDDING_POLICY.into(),
        drives_encryption,
        opt_in_audits,
        open_findings_count,
        vaulted_findings_count,
        confidential_files_count,
        secret_files_count,
        active_rules_count,
        quarantined_items_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;

    #[test]
    fn builds_honest_posture_with_r8_r9_r11_checks() {
        let store = SqliteStore::open_in_memory().unwrap();
        let ctx = VaultPostureContext {
            db_encrypted: true,
            cipher_label: "SQLCipher 4.20.0".into(),
            dpapi_sealed: true,
            recovery_confirmed: true,
            sensitive_unlocked: false,
            sensitive_expires_at: None,
        };
        let report = build_privacy_posture_with_drives(
            store.conn(),
            &ctx,
            vec![DriveEncryptionStatus {
                mount_point: "C:\\".into(),
                label: "Windows".into(),
                protection_status: "protected".into(),
                encryption_method: "Windows BitLocker (Enabled)".into(),
                recommended_action: "OK".into(),
            }],
        )
        .unwrap();
        assert_eq!(report.privacy_score, 100.0);
        assert!(report.zero_network_status.contains("r9_forbids_networking_crates"));
        assert!(!report.zero_network_status.to_lowercase().contains("mathematic"));
        assert!(report.shredding_policy.contains("BitLocker"));
        assert!(report
            .opt_in_audits
            .iter()
            .any(|a| a.id == "screen_capture" && a.state == "permanently_disabled"));
    }
}
