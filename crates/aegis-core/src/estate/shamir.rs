//! Pure-Rust GF(256) Shamir's Secret Sharing (`5.3`, `D1`, `R9`).
//!
//! Splits a 256-bit `DbKey` into `N` share strings such that any `K` distinct shares
//! (`2 <= K <= N <= 16`) reconstruct the exact `DbKey`, while `< K` shares reveal zero bits of
//! information. All temporary buffers holding key or polynomial bytes are zeroized on drop.

use std::collections::HashSet;

use zeroize::Zeroizing;

use crate::db::crypto::{DbKey, fill_random_bytes};
use crate::error::{AegisError, Result};

const SHARD_PREFIX: &str = "CAIRN-SHARD-V1";
/// Prefix of shares generated before the rename to Cairn; still accepted (same checksum).
const LEGACY_SHARD_PREFIX: &str = "AEGIS-SHARD-V1";

#[inline]
fn gf_add(a: u8, b: u8) -> u8 {
    a ^ b
}

#[inline]
fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    p
}

#[inline]
fn gf_pow(mut base: u8, mut exp: u8) -> u8 {
    let mut acc = 1u8;
    while exp > 0 {
        if (exp & 1) != 0 {
            acc = gf_mul(acc, base);
        }
        base = gf_mul(base, base);
        exp >>= 1;
    }
    acc
}

fn gf_inv(a: u8) -> Result<u8> {
    if a == 0 {
        return Err(AegisError::InvalidInput(
            "GF(256) division by zero (duplicate shard index).".into(),
        ));
    }
    Ok(gf_pow(a, 254))
}

#[inline]
fn gf_div(a: u8, b: u8) -> Result<u8> {
    Ok(gf_mul(a, gf_inv(b)?))
}

fn eval_poly(coeffs: &[u8], x: u8) -> u8 {
    let mut acc = 0u8;
    for &c in coeffs.iter().rev() {
        acc = gf_add(gf_mul(acc, x), c);
    }
    acc
}

fn encode_hex_32(bytes: &[u8; 32]) -> Zeroizing<String> {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    Zeroizing::new(out)
}

fn decode_hex_32(hex: &str) -> Result<Zeroizing<[u8; 32]>> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AegisError::InvalidInput(
            "Invalid shard payload length or hex encoding.".into(),
        ));
    }
    let mut out = Zeroizing::new([0u8; 32]);
    let (chunks, _) = hex.as_bytes().as_chunks::<2>();
    for (i, pair) in chunks.iter().enumerate() {
        let s = std::str::from_utf8(pair)
            .map_err(|_| AegisError::InvalidInput("Invalid UTF-8 in shard hex.".into()))?;
        out[i] = u8::from_str_radix(s, 16)
            .map_err(|_| AegisError::InvalidInput("Invalid hex digit in shard.".into()))?;
    }
    Ok(out)
}

fn shard_checksum8(shard_set_id: &str, k: u8, n: u8, idx: u8, hex_payload: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"aegis-shard-v1:");
    h.update(shard_set_id.as_bytes());
    h.update(format!(":{k}-of-{n}:{idx}:").as_bytes());
    h.update(hex_payload.as_bytes());
    h.finalize().to_hex()[..8].to_string()
}

/// Generated raw Shamir shard returned once to the owner; raw token is never persisted in `aegis.db`.
#[derive(Debug, Clone)]
pub struct GeneratedRawShard {
    pub share_index: u8,
    pub checksum_prefix: String,
    pub shard_token: Zeroizing<String>,
}

/// Parsed and checksum-verified Shamir shard token.
#[derive(Debug, Clone)]
pub struct ParsedShard {
    pub shard_set_id: String,
    pub threshold_k: u8,
    pub total_n: u8,
    pub share_index: u8,
    pub checksum_prefix: String,
    pub payload: Zeroizing<[u8; 32]>,
}

/// Splits `key` into `total_n` Shamir shards requiring any `threshold_k` (`2 <= K <= N <= 16`).
pub fn split_key(
    key: &DbKey,
    threshold_k: u8,
    total_n: u8,
    shard_set_id: &str,
) -> Result<Vec<GeneratedRawShard>> {
    if !(2..=16).contains(&threshold_k)
        || !(threshold_k..=16).contains(&total_n)
        || shard_set_id.trim().is_empty()
    {
        return Err(AegisError::InvalidInput(
            "Shamir quorum must satisfy 2 <= threshold_k <= total_n <= 16.".into(),
        ));
    }

    let k = threshold_k as usize;
    let n = total_n as usize;
    let mut raw_shares = vec![Zeroizing::new([0u8; 32]); n];
    let mut rand_coeffs = Zeroizing::new(vec![0u8; k - 1]);
    let mut coeffs = Zeroizing::new(vec![0u8; k]);

    for byte_idx in 0..32 {
        fill_random_bytes(&mut rand_coeffs)?;
        coeffs[0] = key.as_bytes()[byte_idx];
        coeffs[1..].copy_from_slice(&rand_coeffs);
        for (share_pos, share_buf) in raw_shares.iter_mut().enumerate() {
            let x = (share_pos + 1) as u8;
            share_buf[byte_idx] = eval_poly(&coeffs, x);
        }
    }

    let mut out = Vec::with_capacity(n);
    for (share_pos, share_buf) in raw_shares.iter().enumerate() {
        let idx = (share_pos + 1) as u8;
        let hex_payload = encode_hex_32(share_buf);
        let csum = shard_checksum8(shard_set_id, threshold_k, total_n, idx, &hex_payload);
        let token = Zeroizing::new(format!(
            "{SHARD_PREFIX}:{shard_set_id}:{threshold_k}-of-{total_n}:{idx}:{}:{csum}",
            hex_payload.as_str()
        ));
        out.push(GeneratedRawShard {
            share_index: idx,
            checksum_prefix: csum,
            shard_token: token,
        });
    }
    Ok(out)
}

/// Validates syntax and `blake3` integrity tag of one `CAIRN-SHARD-V1` (or legacy `AEGIS-SHARD-V1`) token.
pub fn parse_shard(token: &str) -> Result<ParsedShard> {
    let parts: Vec<&str> = token.trim().split(':').collect();
    if parts.len() != 6 || (parts[0] != SHARD_PREFIX && parts[0] != LEGACY_SHARD_PREFIX) {
        return Err(AegisError::InvalidInput(
            "Invalid Cairn shard format (expected CAIRN-SHARD-V1:...).".into(),
        ));
    }
    let shard_set_id = parts[1].trim().to_string();
    if shard_set_id.is_empty() {
        return Err(AegisError::InvalidInput("Empty shard_set_id.".into()));
    }
    let (k_str, n_str) = parts[2].split_once("-of-").ok_or_else(|| {
        AegisError::InvalidInput("Invalid shard quorum format (expected K-of-N).".into())
    })?;
    let threshold_k: u8 = k_str
        .parse()
        .map_err(|_| AegisError::InvalidInput("Invalid threshold_k in shard.".into()))?;
    let total_n: u8 = n_str
        .parse()
        .map_err(|_| AegisError::InvalidInput("Invalid total_n in shard.".into()))?;
    let share_index: u8 = parts[3]
        .parse()
        .map_err(|_| AegisError::InvalidInput("Invalid share_index in shard.".into()))?;
    if !(2..=16).contains(&threshold_k)
        || !(threshold_k..=16).contains(&total_n)
        || share_index == 0
        || share_index > total_n
    {
        return Err(AegisError::InvalidInput("Shard parameters out of range.".into()));
    }

    let hex_low = parts[4].to_ascii_lowercase();
    let payload = decode_hex_32(&hex_low)?;
    let expected = shard_checksum8(&shard_set_id, threshold_k, total_n, share_index, &hex_low);
    let given = parts[5].to_ascii_lowercase();
    if given != expected {
        return Err(AegisError::InvalidInput(
            "Shard checksum mismatch — verify no typo occurred during transcription.".into(),
        ));
    }

    Ok(ParsedShard {
        shard_set_id,
        threshold_k,
        total_n,
        share_index,
        checksum_prefix: expected,
        payload,
    })
}

/// Reconstructs a `DbKey` via Lagrange interpolation at `x = 0` in GF(256).
pub fn combine_shards(tokens: &[String]) -> Result<DbKey> {
    if tokens.is_empty() {
        return Err(AegisError::InvalidInput("Provide shards to combine.".into()));
    }
    let mut parsed = Vec::with_capacity(tokens.len());
    let mut seen_idx = HashSet::new();
    for tok in tokens {
        let p = parse_shard(tok)?;
        if !seen_idx.insert(p.share_index) {
            return Err(AegisError::InvalidInput(format!(
                "Duplicate share index {} provided.",
                p.share_index
            )));
        }
        parsed.push(p);
    }

    let first = &parsed[0];
    let k = first.threshold_k as usize;
    for p in &parsed[1..] {
        if p.shard_set_id != first.shard_set_id
            || p.threshold_k != first.threshold_k
            || p.total_n != first.total_n
        {
            return Err(AegisError::InvalidInput(
                "All shards must belong to the same shard set and quorum.".into(),
            ));
        }
    }
    if parsed.len() < k {
        return Err(AegisError::InvalidInput(format!(
            "Need at least {} distinct shards for this set (got {}).",
            k,
            parsed.len()
        )));
    }

    let active = &parsed[..k];
    let mut secret = Zeroizing::new([0u8; 32]);
    for (byte_idx, out_byte) in secret.iter_mut().enumerate() {
        let mut val = 0u8;
        for (m, sm) in active.iter().enumerate() {
            let mut basis = 1u8;
            for (r, sr) in active.iter().enumerate() {
                if m == r {
                    continue;
                }
                // Lagrange basis at x = 0 in GF(256): x_r / (x_m ^ x_r).
                let ratio = gf_div(sr.share_index, gf_add(sm.share_index, sr.share_index))?;
                basis = gf_mul(basis, ratio);
            }
            val = gf_add(val, gf_mul(sm.payload[byte_idx], basis));
        }
        *out_byte = val;
    }

    Ok(DbKey::from_bytes(*secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gf256_field_laws_hold() {
        for a in 1u8..=255 {
            let inv = gf_inv(a).unwrap();
            assert_eq!(gf_mul(a, inv), 1, "inverse failed for {a}");
        }
        assert!(gf_inv(0).is_err());
    }

    #[test]
    fn splits_and_reconstructs_2_of_3_and_3_of_5_and_rejects_bad_inputs() {
        let key = DbKey::generate().unwrap();
        let shards = split_key(&key, 2, 3, "set-a").unwrap();
        assert_eq!(shards.len(), 3);

        // Any 2 of 3 must reconstruct the exact key fingerprint & bytes.
        let r01 = combine_shards(&[
            shards[0].shard_token.to_string(),
            shards[1].shard_token.to_string(),
        ])
        .unwrap();
        let r12 = combine_shards(&[
            shards[1].shard_token.to_string(),
            shards[2].shard_token.to_string(),
        ])
        .unwrap();
        let r02 = combine_shards(&[
            shards[0].shard_token.to_string(),
            shards[2].shard_token.to_string(),
        ])
        .unwrap();
        assert_eq!(r01, key);
        assert_eq!(r12, key);
        assert_eq!(r02, key);

        // Fewer than K fails cleanly.
        assert!(combine_shards(&[shards[0].shard_token.to_string()]).is_err());

        // Duplicate share fails cleanly.
        assert!(combine_shards(&[
            shards[0].shard_token.to_string(),
            shards[0].shard_token.to_string(),
        ])
        .is_err());

        // Tampered checksum fails cleanly.
        let mut tampered = shards[0].shard_token.to_string();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'a' { 'b' } else { 'a' });
        assert!(parse_shard(&tampered).is_err());

        // 3-of-5 quorum test across non-adjacent subset {1, 3, 5}.
        let s5 = split_key(&key, 3, 5, "set-5").unwrap();
        let r3 = combine_shards(&[
            s5[0].shard_token.to_string(),
            s5[2].shard_token.to_string(),
            s5[4].shard_token.to_string(),
        ])
        .unwrap();
        assert_eq!(r3, key);
    }

    #[test]
    fn new_shares_use_cairn_prefix_and_legacy_aegis_shares_still_parse() {
        let key = DbKey::from_bytes([7u8; 32]);
        let shards = split_key(&key, 2, 3, "set-legacy").unwrap();
        let token = shards[0].shard_token.to_string();
        assert!(token.starts_with("CAIRN-SHARD-V1:"));
        let legacy = token.replacen("CAIRN-SHARD-V1", "AEGIS-SHARD-V1", 1);
        let parsed = parse_shard(&legacy).unwrap();
        assert_eq!(parsed.share_index, 1);
        assert!(parse_shard(&token.replacen("CAIRN", "OTHER", 1)).is_err());
    }
}
