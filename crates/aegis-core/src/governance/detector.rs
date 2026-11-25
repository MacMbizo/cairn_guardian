//! Pure-Rust high-precision PII & secret detector with strict validation and redaction (`R7`, `R9`).
//!
//! Raw matched secrets are NEVER stored or emitted in snippets — only `blake3` fingerprints and
//! redacted previews leave this module.

use super::{GovernanceFindingType, GovernanceSeverity};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct DetectedSecret {
    pub finding_type: GovernanceFindingType,
    pub severity: GovernanceSeverity,
    pub context_risk_score: f64,
    pub line_number: i64,
    pub fingerprint: [u8; 32],
    pub redacted_snippet: String,
    pub summary: String,
}

pub fn fingerprint_secret(finding_type: GovernanceFindingType, normalized: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aegis-gov-v1:");
    hasher.update(finding_type.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(normalized.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Checks if a path lies inside an exposed user inbox directory (`Downloads`/`Desktop`/`Temp`).
pub fn is_exposed_context(path: &str, in_atomic_unit: bool) -> bool {
    let lower = path.to_ascii_lowercase().replace('\\', "/");
    !in_atomic_unit
        && (lower.contains("/downloads/")
            || lower.contains("/desktop/")
            || lower.contains("/temp/")
            || lower.contains("/public/"))
}

fn adjust_risk(base: f64, exposed: bool, in_atomic_unit: bool) -> f64 {
    let delta = if exposed {
        0.08
    } else if in_atomic_unit {
        -0.05
    } else {
        0.0
    };
    (base + delta).clamp(0.10, 1.0)
}

pub fn scan_file_for_secrets(
    path: &str,
    name: &str,
    ext: &str,
    text_opt: Option<&str>,
    in_atomic_unit: bool,
) -> Vec<DetectedSecret> {
    let mut out = Vec::new();
    let exposed = is_exposed_context(path, in_atomic_unit);

    if let Some(cred) = detect_credential_filename(path, name, ext, exposed, in_atomic_unit) {
        out.push(cred);
    }

    let Some(text) = text_opt else {
        return out;
    };

    let mut seen_line_fp: HashSet<(i64, [u8; 32])> = HashSet::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() || line.len() > 8192 {
            continue;
        }
        for hit in scan_line(line, line_no, exposed, in_atomic_unit) {
            if seen_line_fp.insert((hit.line_number, hit.fingerprint)) {
                out.push(hit);
            }
        }
    }
    out
}

fn detect_credential_filename(
    path: &str,
    name: &str,
    ext: &str,
    exposed: bool,
    in_atomic_unit: bool,
) -> Option<DetectedSecret> {
    let name_low = name.to_ascii_lowercase();
    let ext_low = ext.to_ascii_lowercase();
    let matched_exact = matches!(
        name_low.as_str(),
        ".env"
            | ".env.local"
            | ".env.production"
            | ".env.development"
            | ".env.staging"
            | "id_rsa"
            | "id_ed25519"
            | "id_dsa"
            | "id_ecdsa"
            | "credentials.json"
            | "service-account.json"
            | ".netrc"
            | "_netrc"
            | ".npmrc"
            | ".pypirc"
    );
    let matched_ext = matches!(
        ext_low.as_str(),
        "kdbx" | "p12" | "pfx" | "pem" | "key" | "jks" | "keystore" | "ppk" | "ovpn"
    );
    let matched_recovery = (name_low.contains("recovery-key")
        || name_low.contains("seed-phrase")
        || name_low.contains("wallet-backup"))
        && matches!(ext_low.as_str(), "txt" | "json" | "md" | "");

    if !(matched_exact || matched_ext || matched_recovery) {
        return None;
    }

    let path_norm = path.strip_prefix(r"\\?\").unwrap_or(path).to_ascii_lowercase();
    let sev = if matches!(ext_low.as_str(), "kdbx" | "p12" | "pfx" | "pem" | "key")
        || matches!(name_low.as_str(), "id_rsa" | "id_ed25519" | ".env.production")
    {
        GovernanceSeverity::High
    } else {
        GovernanceSeverity::Medium
    };
    Some(DetectedSecret {
        finding_type: GovernanceFindingType::CredentialFile,
        severity: sev,
        context_risk_score: adjust_risk(0.84, exposed, in_atomic_unit),
        line_number: 0,
        fingerprint: fingerprint_secret(GovernanceFindingType::CredentialFile, &path_norm),
        redacted_snippet: format!("Credential / secret store file `{name}`"),
        summary: format!("Credential or key store file `{name}` detected on disk"),
    })
}

fn scan_line(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
) -> Vec<DetectedSecret> {
    let mut out = Vec::new();
    if let Some(pk) = detect_private_key_header(line, line_no, exposed, in_atomic_unit) {
        out.push(pk);
        return out;
    }
    scan_cloud_api_keys_and_env(line, line_no, exposed, in_atomic_unit, &mut out);
    scan_payment_cards(line, line_no, exposed, in_atomic_unit, &mut out);
    scan_govt_ids(line, line_no, exposed, in_atomic_unit, &mut out);
    scan_iban_and_bank(line, line_no, exposed, in_atomic_unit, &mut out);
    out
}

fn detect_private_key_header(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
) -> Option<DetectedSecret> {
    let start = line.find("-----BEGIN ")?;
    let rest = &line[start..];
    if !rest.contains("PRIVATE KEY-----") {
        return None;
    }
    let end = rest.find("-----END").unwrap_or(rest.len());
    let header = rest[..rest.find("PRIVATE KEY-----").map(|i| i + 16).unwrap_or(end)]
        .chars()
        .take(64)
        .collect::<String>();
    Some(DetectedSecret {
        finding_type: GovernanceFindingType::PrivateKeyBlock,
        severity: GovernanceSeverity::Critical,
        context_risk_score: adjust_risk(0.98, exposed, in_atomic_unit),
        line_number: line_no,
        fingerprint: fingerprint_secret(GovernanceFindingType::PrivateKeyBlock, &header),
        redacted_snippet: format!("{header} [REDACTED KEY MATERIAL]"),
        summary: "Unencrypted PEM/SSH private key block detected".into(),
    })
}

fn is_placeholder_secret(val: &str) -> bool {
    let low = val.to_ascii_lowercase();
    let bad_markers = [
        "example",
        "sample",
        "placeholder",
        "changeme",
        "your_",
        "your-",
        "replace_me",
        "dummy",
        "fake",
        "test_key",
        "sk_test_",
        "rk_test_",
        "pk_test_",
        "xxxx",
        "0000",
        "123456",
        "<",
        "${",
        "{{",
    ];
    if bad_markers.iter().any(|m| low.contains(m)) {
        return true;
    }
    let unique_chars: HashSet<char> = low.chars().collect();
    unique_chars.len() <= 3
}

fn redact_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 8 {
        return "****REDACTED****".into();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{prefix}********{suffix}")
}

fn redact_line_with(line: &str, span_start: usize, span_end: usize, masked: &str) -> String {
    let before = line[..span_start].trim_start();
    let after = line[span_end..].trim_end();
    let before_tail: String = if before.chars().count() > 24 {
        let skip = before.chars().count() - 24;
        format!("...{}", before.chars().skip(skip).collect::<String>())
    } else {
        before.to_string()
    };
    let after_head: String = after.chars().take(16).collect();
    format!("{before_tail}{masked}{after_head}")
}

fn scan_cloud_api_keys_and_env(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
    out: &mut Vec<DetectedSecret>,
) {
    let prefixes: &[(&str, &str, GovernanceSeverity, usize)] = &[
        ("AKIA", "AWS Access Key ID", GovernanceSeverity::Critical, 20),
        ("ASIA", "AWS Temporary Access Key", GovernanceSeverity::Critical, 20),
        ("ghp_", "GitHub Personal Access Token", GovernanceSeverity::Critical, 24),
        ("gho_", "GitHub OAuth Token", GovernanceSeverity::Critical, 24),
        ("ghu_", "GitHub User Token", GovernanceSeverity::Critical, 24),
        ("ghs_", "GitHub Server Token", GovernanceSeverity::Critical, 24),
        ("github_pat_", "GitHub Fine-Grained PAT", GovernanceSeverity::Critical, 36),
        ("sk_live_", "Stripe Live Secret Key", GovernanceSeverity::Critical, 22),
        ("rk_live_", "Stripe Restricted Live Key", GovernanceSeverity::Critical, 22),
        ("xoxb-", "Slack Bot Token", GovernanceSeverity::High, 20),
        ("xoxp-", "Slack User Token", GovernanceSeverity::High, 20),
        ("AIza", "Google API Key", GovernanceSeverity::High, 35),
        ("sk-proj-", "AI Provider Project Key", GovernanceSeverity::High, 28),
        ("sk-ant-", "AI Provider API Key", GovernanceSeverity::High, 28),
    ];

    for (pref, label, sev, min_len) in prefixes {
        let mut search_from = 0usize;
        while let Some(rel) = line[search_from..].find(pref) {
            let start = search_from + rel;
            let boundary_ok = start == 0 || !is_token_char(line.as_bytes()[start - 1]);
            if boundary_ok {
                let mut end = start + pref.len();
                let bytes = line.as_bytes();
                while end < bytes.len() && is_token_char(bytes[end]) {
                    end += 1;
                }
                let tok = &line[start..end];
                if tok.len() >= *min_len && !is_placeholder_secret(tok) {
                    let masked = redact_token(tok);
                    out.push(DetectedSecret {
                        finding_type: GovernanceFindingType::CloudApiKey,
                        severity: *sev,
                        context_risk_score: adjust_risk(0.94, exposed, in_atomic_unit),
                        line_number: line_no,
                        fingerprint: fingerprint_secret(GovernanceFindingType::CloudApiKey, tok),
                        redacted_snippet: redact_line_with(line, start, end, &masked),
                        summary: format!("{label} exposed in plaintext"),
                    });
                }
                search_from = end.max(start + 1);
            } else {
                search_from = start + pref.len();
            }
        }
    }

    // High-entropy secret assignment fallback (KEY=value or KEY: "value").
    if let Some((key_name, val_start, val_end, val_str)) = parse_secret_assignment(line)
        && val_str.len() >= 14
        && !is_placeholder_secret(val_str)
        && shannon_entropy(val_str) >= 3.2
        && has_mixed_char_classes(val_str)
    {
        let masked = redact_token(val_str);
        out.push(DetectedSecret {
            finding_type: GovernanceFindingType::CloudApiKey,
            severity: GovernanceSeverity::High,
            context_risk_score: adjust_risk(0.86, exposed, in_atomic_unit),
            line_number: line_no,
            fingerprint: fingerprint_secret(GovernanceFindingType::CloudApiKey, val_str),
            redacted_snippet: redact_line_with(line, val_start, val_end, &masked),
            summary: format!("High-entropy credential assigned to `{key_name}`"),
        });
    }
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

fn parse_secret_assignment(line: &str) -> Option<(&str, usize, usize, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//") {
        return None;
    }
    let sep_idx = line.find('=').or_else(|| line.find(':'))?;
    let lhs = line[..sep_idx].trim();
    if lhs.is_empty() || lhs.len() > 64 || lhs.contains(' ') {
        return None;
    }
    let lhs_upper = lhs.to_ascii_uppercase();
    let sensitive_key = [
        "API_KEY",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "ACCESS_TOKEN",
        "AUTH_TOKEN",
        "PRIVATE_KEY",
        "CLIENT_SECRET",
        "DB_PASS",
    ]
    .iter()
    .any(|k| lhs_upper.contains(k));
    if !sensitive_key {
        return None;
    }

    let mut s = sep_idx + 1;
    let bytes = line.as_bytes();
    while s < bytes.len() && matches!(bytes[s], b' ' | b'\t') {
        s += 1;
    }
    if s >= bytes.len() {
        return None;
    }
    let quote = if matches!(bytes[s], b'"' | b'\'') {
        let q = bytes[s];
        s += 1;
        Some(q)
    } else {
        None
    };
    let mut e = s;
    while e < bytes.len() {
        if let Some(q) = quote {
            if bytes[e] == q {
                break;
            }
        } else if matches!(bytes[e], b' ' | b'\t' | b',' | b';') {
            break;
        }
        e += 1;
    }
    if s >= e {
        return None;
    }
    Some((lhs, s, e, &line[s..e]))
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts = [0usize; 256];
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return 0.0;
    }
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    let mut h = 0.0f64;
    for &c in &counts {
        if c > 0 {
            let p = (c as f64) / len;
            h -= p * p.log2();
        }
    }
    h
}

fn has_mixed_char_classes(s: &str) -> bool {
    let has_digit = s.bytes().any(|b| b.is_ascii_digit());
    let has_upper = s.bytes().any(|b| b.is_ascii_uppercase());
    let has_lower = s.bytes().any(|b| b.is_ascii_lowercase());
    let has_sym = s.bytes().any(|b| !b.is_ascii_alphanumeric());
    usize::from(has_digit) + usize::from(has_upper) + usize::from(has_lower) + usize::from(has_sym)
        >= 3
}

/// Validates digits with Luhn checksum.
pub fn luhn_valid(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for b in digits.bytes().rev() {
        let mut d = u32::from(b - b'0');
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum.is_multiple_of(10)
}

fn known_card_bin_and_len(digits: &str) -> bool {
    let n = digits.len();
    if !(13..=19).contains(&n) {
        return false;
    }
    let b = digits.as_bytes();
    if b.iter().all(|&c| c == b[0]) {
        return false;
    }
    if b[0] == b'4' && matches!(n, 13 | 16 | 19) {
        return true; // Visa
    }
    if n == 16 && let Ok(p2) = digits[..2].parse::<u32>() && (51..=55).contains(&p2) {
        return true; // Mastercard 51-55
    }
    if n == 16 && let Ok(p4) = digits[..4].parse::<u32>() && (2221..=2720).contains(&p4) {
        return true; // Mastercard 2-series
    }
    if n == 15 && (digits.starts_with("34") || digits.starts_with("37")) {
        return true; // Amex
    }
    if n == 16 && (digits.starts_with("6011") || digits.starts_with("65")) {
        return true; // Discover
    }
    false
}

fn scan_payment_cards(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
    out: &mut Vec<DetectedSecret>,
) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.') {
            i += 1;
            continue;
        }
        let mut end = i;
        let mut digits = String::new();
        while end < bytes.len() {
            let ch = bytes[end];
            if ch.is_ascii_digit() {
                digits.push(ch as char);
                end += 1;
            } else if matches!(ch, b'-' | b' ')
                && end + 1 < bytes.len()
                && bytes[end + 1].is_ascii_digit()
            {
                end += 1;
            } else {
                break;
            }
        }
        let trailing_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if trailing_ok && known_card_bin_and_len(&digits) && luhn_valid(&digits) {
            let last4 = &digits[digits.len() - 4..];
            let first4 = &digits[..4];
            let masked = format!("{first4}-****-****-{last4}");
            let low = line.to_ascii_lowercase();
            let base = if ["card", "visa", "master", "amex", "payment", "cvv", "billing"]
                .iter()
                .any(|w| low.contains(w))
            {
                0.92
            } else {
                0.84
            };
            out.push(DetectedSecret {
                finding_type: GovernanceFindingType::PaymentCard,
                severity: GovernanceSeverity::Critical,
                context_risk_score: adjust_risk(base, exposed, in_atomic_unit),
                line_number: line_no,
                fingerprint: fingerprint_secret(GovernanceFindingType::PaymentCard, &digits),
                redacted_snippet: redact_line_with(line, i, end, &masked),
                summary: format!("Luhn-valid payment card ending in {last4}"),
            });
            i = end;
            continue;
        }
        i += 1;
    }
}

/// Validates ISO 13616 IBAN checksum (`MOD 97 == 1`).
pub fn iban_mod97_valid(compact: &str) -> bool {
    if compact.len() < 15 || compact.len() > 34 {
        return false;
    }
    let bytes = compact.as_bytes();
    if !(bytes[0].is_ascii_uppercase()
        && bytes[1].is_ascii_uppercase()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit())
    {
        return false;
    }
    if !bytes[4..].iter().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    let rearranged = format!("{}{}", &compact[4..], &compact[..4]);
    let mut rem = 0u32;
    for b in rearranged.bytes() {
        if b.is_ascii_digit() {
            rem = (rem * 10 + u32::from(b - b'0')) % 97;
        } else {
            let val = u32::from(b.to_ascii_uppercase() - b'A' + 10);
            rem = (rem * 100 + val) % 97;
        }
    }
    rem == 1
}

fn scan_iban_and_bank(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
    out: &mut Vec<DetectedSecret>,
) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i + 15 <= bytes.len() {
        if bytes[i].is_ascii_uppercase()
            && bytes[i + 1].is_ascii_uppercase()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let mut end = i + 4;
            let mut compact = String::from(&line[i..i + 4]);
            while end < bytes.len() && compact.len() < 34 {
                let c = bytes[end];
                if c.is_ascii_alphanumeric() {
                    compact.push((c as char).to_ascii_uppercase());
                    end += 1;
                } else if c == b' ' && end + 1 < bytes.len() && bytes[end + 1].is_ascii_alphanumeric()
                {
                    end += 1;
                } else {
                    break;
                }
            }
            if (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric()) && iban_mod97_valid(&compact)
            {
                let last4 = &compact[compact.len() - 4..];
                let masked = format!("{}****{last4}", &compact[..4]);
                out.push(DetectedSecret {
                    finding_type: GovernanceFindingType::BankAccountOrIban,
                    severity: GovernanceSeverity::High,
                    context_risk_score: adjust_risk(0.90, exposed, in_atomic_unit),
                    line_number: line_no,
                    fingerprint: fingerprint_secret(
                        GovernanceFindingType::BankAccountOrIban,
                        &compact,
                    ),
                    redacted_snippet: redact_line_with(line, i, end, &masked),
                    summary: format!("International bank account (IBAN) ending in {last4}"),
                });
                i = end;
                continue;
            }
        }
        i += 1;
    }

    let low = line.to_ascii_lowercase();
    let has_bank_ctx = [
        "bank account",
        "routing number",
        "account number",
        "aba routing",
        "swift",
        "acct#",
    ]
    .iter()
    .any(|w| low.contains(w));
    if has_bank_ctx {
        for (s, e, digits) in digit_runs(line, 8, 17) {
            if out.iter().any(|h| h.line_number == line_no) {
                continue;
            }
            let last4 = &digits[digits.len() - 4..];
            let masked = format!("****{last4}");
            out.push(DetectedSecret {
                finding_type: GovernanceFindingType::BankAccountOrIban,
                severity: GovernanceSeverity::Medium,
                context_risk_score: adjust_risk(0.80, exposed, in_atomic_unit),
                line_number: line_no,
                fingerprint: fingerprint_secret(
                    GovernanceFindingType::BankAccountOrIban,
                    &digits,
                ),
                redacted_snippet: redact_line_with(line, s, e, &masked),
                summary: format!("Bank / routing account number ending in {last4}"),
            });
        }
    }
}

fn scan_govt_ids(
    line: &str,
    line_no: i64,
    exposed: bool,
    in_atomic_unit: bool,
    out: &mut Vec<DetectedSecret>,
) {
    let low = line.to_ascii_lowercase();
    let ssn_ctx = ["ssn", "social security", "tax id", "tin", "ein"]
        .iter()
        .any(|w| low.contains(w));
    if ssn_ctx {
        let bytes = line.as_bytes();
        let mut i = 0usize;
        while i + 11 <= bytes.len() {
            if bytes[i..i + 3].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 3] == b'-'
                && bytes[i + 4..i + 6].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 6] == b'-'
                && bytes[i + 7..i + 11].iter().all(|b| b.is_ascii_digit())
            {
                let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
                let after_ok = i + 11 >= bytes.len() || !bytes[i + 11].is_ascii_digit();
                let area: u32 = line[i..i + 3].parse().unwrap_or(0);
                let group: u32 = line[i + 4..i + 6].parse().unwrap_or(0);
                let serial: u32 = line[i + 7..i + 11].parse().unwrap_or(0);
                if before_ok
                    && after_ok
                    && (1..=899).contains(&area)
                    && area != 666
                    && group != 0
                    && serial != 0
                {
                    let raw = &line[i..i + 11];
                    let last4 = &raw[7..11];
                    let masked = format!("***-**-{last4}");
                    out.push(DetectedSecret {
                        finding_type: GovernanceFindingType::GovernmentId,
                        severity: GovernanceSeverity::Critical,
                        context_risk_score: adjust_risk(0.94, exposed, in_atomic_unit),
                        line_number: line_no,
                        fingerprint: fingerprint_secret(
                            GovernanceFindingType::GovernmentId,
                            &format!("ssn:{raw}"),
                        ),
                        redacted_snippet: redact_line_with(line, i, i + 11, &masked),
                        summary: format!("Government tax / SSN identifier ending in {last4}"),
                    });
                    i += 11;
                    continue;
                }
            }
            i += 1;
        }
    }

    let id_ctx = [
        "id number",
        "national id",
        "identity",
        "passport",
        "sa id",
        "id:",
        "id#",
    ]
    .iter()
    .any(|w| low.contains(w));
    if id_ctx {
        for (s, e, digits) in digit_runs(line, 13, 13) {
            if is_valid_sa_id(&digits) {
                let last4 = &digits[9..13];
                let masked = format!("*********{}", last4);
                out.push(DetectedSecret {
                    finding_type: GovernanceFindingType::GovernmentId,
                    severity: GovernanceSeverity::Critical,
                    context_risk_score: adjust_risk(0.95, exposed, in_atomic_unit),
                    line_number: line_no,
                    fingerprint: fingerprint_secret(
                        GovernanceFindingType::GovernmentId,
                        &format!("za-id:{digits}"),
                    ),
                    redacted_snippet: redact_line_with(line, s, e, &masked),
                    summary: format!("Luhn-valid national ID number ending in {last4}"),
                });
            }
        }
    }
}

/// Validates 13-digit South African ID numbers (`YYMMDDSSSSCAZ` month/day + Luhn).
pub fn is_valid_sa_id(digits: &str) -> bool {
    if digits.len() != 13 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mm: u32 = digits[2..4].parse().unwrap_or(0);
    let dd: u32 = digits[4..6].parse().unwrap_or(0);
    let citizenship = digits.as_bytes()[10];
    (1..=12).contains(&mm)
        && (1..=31).contains(&dd)
        && matches!(citizenship, b'0' | b'1')
        && luhn_valid(digits)
}

fn digit_runs(line: &str, min_len: usize, max_len: usize) -> Vec<(usize, usize, String)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let mut end = i;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        let len = end - i;
        if before_ok && after_ok && len >= min_len && len <= max_len {
            out.push((i, end, line[i..end].to_string()));
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_luhn_iban_and_sa_id_checksums() {
        assert!(luhn_valid("4532015112830366"));
        assert!(!luhn_valid("4532015112830367"));
        assert!(iban_mod97_valid("GB82WEST12345698765432"));
        assert!(!iban_mod97_valid("GB82WEST12345698765433"));
        assert!(is_valid_sa_id("8001015009087"));
        assert!(!is_valid_sa_id("8001015009088"));
    }

    #[test]
    fn detects_and_redacts_secrets_while_skipping_placeholders() {
        // Assemble test secrets dynamically so literal tokens don't appear intact in source.
        let aws_tok = ["AKIA", "Z9876543210ABCDE"].concat();
        let gh_tok = ["ghp_", "9xK2mP7qL4nV8wR1tY5uJ3hF6gD0sA"].concat();
        let card = ["4532", "-0151", "-1283", "-0366"].concat();
        let ssn = ["123", "-45", "-6789"].concat();
        let text = format!(
            "\
aws_access_key_id = {aws_tok}
github = {gh_tok}
Billing card: {card}
Tax SSN: {ssn}
IBAN: GB82 WEST 1234 5698 7654 32
Identity Number: 8001015009087
stripe_test = sk_test_51ExamplePlaceholderKey123456
API_KEY=your_api_key_placeholder
"
        );
        let hits = scan_file_for_secrets(
            r"C:\Users\Owner\Downloads\tax_2026.txt",
            "tax_2026.txt",
            "txt",
            Some(&text),
            false,
        );
        assert!(hits.len() >= 5, "got {hits:#?}");
        for h in &hits {
            assert!(!h.redacted_snippet.contains(&aws_tok));
            assert!(!h.redacted_snippet.contains(&gh_tok));
            assert!(!h.redacted_snippet.contains("4532-0151"));
            assert!(!h.redacted_snippet.contains("123-45"));
            assert!(!h.redacted_snippet.contains("8001015009087"));
            assert!(!h.redacted_snippet.contains("sk_test_"));
        }
    }
}
