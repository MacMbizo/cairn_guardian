//! D1 at-rest database encryption, Windows DPAPI sealing (`R2`), offline 24-word/hex recovery,
//! Windows user verification, and verified plaintext-to-SQLCipher migration.

use crate::db::SqliteStore;
use crate::error::{AegisError, Result};
use crate::now_ms;
use bip39::{Language, Mnemonic};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const SEALED_MAGIC: &[u8; 8] = b"AEGISK1\0";
const DPAPI_ENTROPY: &[u8] = b"aegis-guardian-sqlcipher-v1";
const CIPHER_LABEL: &str = "sqlcipher-aes256-libtomcrypt";

/// 256-bit SQLCipher raw database encryption key, zeroized automatically when dropped.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct DbKey([u8; 32]);

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey([REDACTED])")
    }
}

impl DbKey {
    /// Constructs a key from exact 32 bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generates a cryptographically strong 256-bit key using the OS CSPRNG.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        fill_random_bytes(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Raw 32-byte key slice.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Non-secret 16-hex-char identifier for display and vault metadata verification.
    pub fn fingerprint(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"aegis-key-fp-v1:");
        h.update(&self.0);
        let hex = h.finalize().to_hex().to_string();
        hex[..16].to_string()
    }

    /// Lowercase 64-character raw hex string.
    pub fn to_hex_compact(&self) -> Zeroizing<String> {
        let mut out = String::with_capacity(64);
        for b in &self.0 {
            out.push_str(&format!("{b:02x}"));
        }
        Zeroizing::new(out)
    }

    /// SQLCipher `PRAGMA key = "x'<64-hex>'";` statement.
    pub fn raw_key_pragma(&self) -> Zeroizing<String> {
        let hex = self.to_hex_compact();
        Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex.as_str()))
    }

    /// SQL string literal (`'x''<64-hex>'''`) for `ATTACH ... KEY`.
    fn attach_key_literal(&self) -> Zeroizing<String> {
        let hex = self.to_hex_compact();
        Zeroizing::new(format!("'x''{}'''", hex.as_str()))
    }

    /// 64 lowercase hex characters formatted into 8 groups of 8 for readable offline backup.
    pub fn to_recovery_hex(&self) -> Zeroizing<String> {
        let raw = self.to_hex_compact();
        let grouped = raw
            .as_bytes()
            .chunks(8)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        Zeroizing::new(grouped)
    }

    /// 24-word BIP-39 English mnemonic representing all 256 key bits + 8-bit checksum.
    pub fn to_recovery_words_vec(&self) -> Result<Vec<String>> {
        let m = Mnemonic::from_entropy(&self.0)
            .map_err(|e| AegisError::Vault(format!("failed to encode recovery phrase: {e}")))?;
        Ok(m.words().map(str::to_string).collect())
    }

    /// Space-separated 24-word BIP-39 English recovery phrase.
    pub fn to_recovery_words(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(self.to_recovery_words_vec()?.join(" ")))
    }

    /// Parses either a 24-word English BIP-39 phrase or a 64-char hex string (spaces/hyphens allowed).
    pub fn from_recovery_input(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(AegisError::InvalidInput(
                "Enter your 24-word recovery phrase or 64-character hex key.".into(),
            ));
        }

        let compact: Zeroizing<String> = Zeroizing::new(
            trimmed
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect(),
        );
        if compact.len() == 64 && compact.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut bytes = [0u8; 32];
            let (chunks, _) = compact.as_bytes().as_chunks::<2>();
            for (i, chunk) in chunks.iter().enumerate() {
                let s = std::str::from_utf8(chunk).unwrap_or("00");
                bytes[i] = u8::from_str_radix(s, 16)
                    .map_err(|_| AegisError::InvalidInput("Invalid hex recovery key.".into()))?;
            }
            return Ok(Self(bytes));
        }

        let words: Vec<String> = trimmed
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        if words.len() != 24 {
            return Err(AegisError::InvalidInput(
                "Enter either 24 recovery words or a 64-character hex recovery key.".into(),
            ));
        }
        let joined = Zeroizing::new(words.join(" "));
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, joined.as_str()).map_err(|_| {
            AegisError::InvalidInput(
                "Invalid 24-word recovery phrase (check spelling and word order).".into(),
            )
        })?;
        let entropy = Zeroizing::new(mnemonic.to_entropy());
        if entropy.len() != 32 {
            return Err(AegisError::InvalidInput(
                "Recovery phrase decoded to an unexpected key length.".into(),
            ));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&entropy[..32]);
        Ok(Self(bytes))
    }
}

#[cfg(windows)]
pub fn fill_random_bytes(buf: &mut [u8]) -> Result<()> {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    // SAFETY: `buf` is a writable byte slice in this stack frame; `pbBuffer` receives `buf.len()` bytes
    // from the Windows system preferred CSPRNG when `hAlgorithm` is NULL.
    let status = unsafe {
        BCryptGenRandom(
            core::ptr::null_mut(),
            buf.as_mut_ptr(),
            u32::try_from(buf.len()).map_err(|_| AegisError::Vault("random buffer too large".into()))?,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(AegisError::Vault(format!(
            "Windows CSPRNG failed with NTSTATUS 0x{status:08x}"
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn fill_random_bytes(buf: &mut [u8]) -> Result<()> {
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(buf)?;
    Ok(())
}

/// Non-secret vault metadata saved beside `aegis.db` (`aegis.vault.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultMeta {
    pub version: u32,
    pub cipher: String,
    pub key_fingerprint: String,
    pub created_at: i64,
    pub migrated_from_plaintext_at: Option<i64>,
    pub recovery_confirmed_at: Option<i64>,
}

/// Sidecar DPAPI-sealed key file next to `db_path`.
pub fn sealed_key_path(db_path: &Path) -> PathBuf {
    db_path.with_file_name("aegis.key.dpapi")
}

/// Sidecar non-secret vault metadata file next to `db_path`.
pub fn vault_meta_path(db_path: &Path) -> PathBuf {
    db_path.with_file_name("aegis.vault.json")
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "aegis".into())
    ));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_vault_meta(path: &Path) -> Result<Option<VaultMeta>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

pub fn save_vault_meta(path: &Path, meta: &VaultMeta) -> Result<()> {
    let json = serde_json::to_string_pretty(meta)?;
    atomic_write_bytes(path, json.as_bytes())
}

/// Seals `key` with Windows DPAPI (bound to the local Windows user/machine credentials so the
/// engine runs unattended after reboot without interactive sign-in) and writes `path` atomically.
#[cfg(windows)]
pub fn seal_key_to_file(key: &DbKey, path: &Path) -> Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let descr: Vec<u16> = "Cairn Guardian Database Key\0".encode_utf16().collect();
    let data_in = CRYPT_INTEGER_BLOB {
        cbData: 32,
        pbData: key.0.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: DPAPI_ENTROPY.len() as u32,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut data_out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: core::ptr::null_mut(),
    };
    // SAFETY: pointers reference valid stack/static slices for the duration of `CryptProtectData`.
    let ok = unsafe {
        CryptProtectData(
            &data_in,
            descr.as_ptr(),
            &entropy,
            core::ptr::null(),
            core::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut data_out,
        )
    };
    if ok == 0 || data_out.pbData.is_null() {
        let err = std::io::Error::last_os_error();
        return Err(AegisError::Vault(format!("DPAPI CryptProtectData failed: {err}")));
    }
    // SAFETY: `data_out.pbData` was allocated by CryptProtectData with length `data_out.cbData`.
    unsafe {
        let blob = std::slice::from_raw_parts(data_out.pbData, data_out.cbData as usize);
        let mut framed = Vec::with_capacity(SEALED_MAGIC.len() + blob.len());
        framed.extend_from_slice(SEALED_MAGIC);
        framed.extend_from_slice(blob);
        LocalFree(data_out.pbData as _);
        atomic_write_bytes(path, &framed)
    }
}

/// Unseals a DPAPI-protected key written by [`seal_key_to_file`].
#[cfg(windows)]
pub fn unseal_key_from_file(path: &Path) -> Result<DbKey> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let raw = std::fs::read(path)?;
    let Some(cipher_blob) = raw.strip_prefix(SEALED_MAGIC) else {
        return Err(AegisError::Vault("Sealed key file header is invalid or corrupted.".into()));
    };
    if cipher_blob.is_empty() {
        return Err(AegisError::Vault("Sealed key file payload is empty.".into()));
    }
    let data_in = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(cipher_blob.len())
            .map_err(|_| AegisError::Vault("Sealed key file too large.".into()))?,
        pbData: cipher_blob.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: DPAPI_ENTROPY.len() as u32,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut data_out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: core::ptr::null_mut(),
    };
    // SAFETY: `cipher_blob` and `DPAPI_ENTROPY` remain valid throughout `CryptUnprotectData`.
    let ok = unsafe {
        CryptUnprotectData(
            &data_in,
            core::ptr::null_mut(),
            &entropy,
            core::ptr::null(),
            core::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut data_out,
        )
    };
    if ok == 0 || data_out.pbData.is_null() {
        let err = std::io::Error::last_os_error();
        return Err(AegisError::Vault(format!("DPAPI CryptUnprotectData failed: {err}")));
    }
    // SAFETY: `data_out.pbData` is a valid buffer of `data_out.cbData` bytes; zeroize before `LocalFree`.
    unsafe {
        let slice = std::slice::from_raw_parts_mut(data_out.pbData, data_out.cbData as usize);
        let out = if slice.len() == 32 {
            let mut key_bytes = [0u8; 32];
            key_bytes.copy_from_slice(slice);
            Ok(DbKey(key_bytes))
        } else {
            Err(AegisError::Vault("Unsealed key had an unexpected length.".into()))
        };
        slice.zeroize();
        LocalFree(data_out.pbData as _);
        out
    }
}

#[cfg(not(windows))]
pub fn seal_key_to_file(key: &DbKey, path: &Path) -> Result<()> {
    let mut framed = Vec::with_capacity(SEALED_MAGIC.len() + 32);
    framed.extend_from_slice(SEALED_MAGIC);
    framed.extend_from_slice(&key.0);
    atomic_write_bytes(path, &framed)
}

#[cfg(not(windows))]
pub fn unseal_key_from_file(path: &Path) -> Result<DbKey> {
    let raw = std::fs::read(path)?;
    let Some(body) = raw.strip_prefix(SEALED_MAGIC) else {
        return Err(AegisError::Vault("Sealed key file header is invalid.".into()));
    };
    if body.len() != 32 {
        return Err(AegisError::Vault("Sealed key payload has unexpected length.".into()));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(body);
    Ok(DbKey(bytes))
}

/// Prompts Windows Security (Windows Hello / PIN / Windows password) before sensitive views or
/// revealing the 24-word recovery phrase (`R2`). Returns `Ok(false)` when the user cancels.
#[cfg(windows)]
pub fn verify_windows_user(caption: &str, message: &str) -> Result<bool> {
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Credentials::{
        CREDUI_INFOW, CREDUIWIN_ENUMERATE_CURRENT_USER, CredUIPromptForWindowsCredentialsW,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;

    let cap_w: Vec<u16> = caption.encode_utf16().chain(Some(0)).collect();
    let msg_w: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        hwndParent: core::ptr::null_mut(),
        pszMessageText: msg_w.as_ptr(),
        pszCaptionText: cap_w.as_ptr(),
        hbmBanner: core::ptr::null_mut(),
    };
    let mut auth_pkg: u32 = 0;
    let mut out_buf: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut out_sz: u32 = 0;
    // SAFETY: `info` strings live through the synchronous modal prompt; `out_buf` is zeroed and
    // freed with `CoTaskMemFree` immediately afterward.
    let rc = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut auth_pkg,
            core::ptr::null(),
            0,
            &mut out_buf,
            &mut out_sz,
            core::ptr::null_mut(),
            CREDUIWIN_ENUMERATE_CURRENT_USER,
        )
    };
    if !out_buf.is_null() {
        // SAFETY: `out_buf` was allocated by CredUI with length `out_sz`.
        unsafe {
            if out_sz > 0 {
                std::slice::from_raw_parts_mut(out_buf.cast::<u8>(), out_sz as usize).zeroize();
            }
            CoTaskMemFree(out_buf);
        }
    }
    match rc {
        ERROR_SUCCESS => Ok(true),
        ERROR_CANCELLED => Ok(false),
        other => Err(AegisError::Vault(format!(
            "Windows credential check returned error code {other}"
        ))),
    }
}

#[cfg(not(windows))]
pub fn verify_windows_user(_caption: &str, _message: &str) -> Result<bool> {
    Ok(true)
}

/// True when `path` exists and begins with the unencrypted SQLite 3 file header (`SQLite format 3\0`).
pub fn is_plaintext_sqlite(path: &Path) -> Result<bool> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = [0u8; 16];
    let n = f.read(&mut buf)?;
    Ok(n >= 16 && &buf == b"SQLite format 3\0")
}

fn wal_sidecars(path: &Path) -> [PathBuf; 2] {
    let base = path.as_os_str().to_string_lossy();
    [PathBuf::from(format!("{base}-wal")), PathBuf::from(format!("{base}-shm"))]
}

fn remove_sqlite_and_wal(path: &Path) {
    let _ = std::fs::remove_file(path);
    for side in wal_sidecars(path) {
        let _ = std::fs::remove_file(side);
    }
}

fn collect_table_counts(conn: &Connection) -> Result<(Vec<(String, i64)>, bool)> {
    let mut counts = Vec::new();
    for tbl in ["files", "chunks", "collections", "audit_log"] {
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [tbl],
            |r| r.get(0),
        )?;
        if exists != 0 {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))?;
            counts.push((tbl.to_string(), n));
        }
    }
    let has_fts: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chunks_fts')",
        [],
        |r| r.get(0),
    )?;
    Ok((counts, has_fts != 0))
}

/// Losslessly converts an existing unencrypted SQLite database at `db_path` to SQLCipher using
/// `key`, verifying integrity, encryption header, FTS5, and exact row counts before replacing the
/// original file and removing the temporary plaintext backup (`R8`).
pub fn migrate_plaintext_to_encrypted(db_path: &Path, key: &DbKey) -> Result<()> {
    if !is_plaintext_sqlite(db_path)? {
        return Ok(());
    }
    let leaf = db_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "aegis.db".into());
    let tmp = db_path.with_file_name(format!("{leaf}.encrypting"));
    let bak = db_path.with_file_name(format!("{leaf}.plaintext.bak"));
    remove_sqlite_and_wal(&tmp);
    remove_sqlite_and_wal(&bak);

    let (expected_counts, has_fts, user_version) = {
        let plain = Connection::open(db_path)?;
        let _ = plain.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        let uv: i64 = plain.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let (counts, has_fts) = collect_table_counts(&plain)?;
        let key_lit = key.attach_key_literal();
        let attach_sql = Zeroizing::new(format!("ATTACH DATABASE ?1 AS encrypted KEY {}", key_lit.as_str()));
        plain.execute(attach_sql.as_str(), params![tmp.to_string_lossy().as_ref()])?;
        plain.execute_batch("SELECT sqlcipher_export('encrypted'); DETACH DATABASE encrypted;")?;
        (counts, has_fts, uv)
    };

    // Verify encrypted clone before touching the live filename.
    {
        if is_plaintext_sqlite(&tmp)? {
            remove_sqlite_and_wal(&tmp);
            return Err(AegisError::Vault(
                "Migrated database still had a plaintext SQLite header.".into(),
            ));
        }
        let verify = SqliteStore::open_encrypted_raw(&tmp, key)?;
        let quick: String = verify.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if !quick.eq_ignore_ascii_case("ok") {
            drop(verify);
            remove_sqlite_and_wal(&tmp);
            return Err(AegisError::Vault(format!(
                "Encrypted migration integrity check failed: {quick}"
            )));
        }
        let vec_ver: String = verify.query_row("SELECT vec_version()", [], |r| r.get(0))?;
        if vec_ver.is_empty() {
            drop(verify);
            remove_sqlite_and_wal(&tmp);
            return Err(AegisError::Vault("sqlite-vec extension did not register.".into()));
        }
        for (tbl, expected) in &expected_counts {
            let actual: i64 = verify.query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))?;
            if actual != *expected {
                drop(verify);
                remove_sqlite_and_wal(&tmp);
                return Err(AegisError::Vault(format!(
                    "Encrypted migration row count mismatch for {tbl}: expected {expected}, got {actual}"
                )));
            }
        }
        if has_fts
            && verify
                .execute("INSERT INTO chunks_fts(chunks_fts) VALUES('integrity-check')", [])
                .is_err()
        {
            verify.execute("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild')", [])?;
        }
        if user_version > 0 {
            verify.execute_batch(&format!("PRAGMA user_version = {user_version};"))?;
        }
    }

    std::fs::rename(db_path, &bak)?;
    for side in wal_sidecars(db_path) {
        let _ = std::fs::remove_file(side);
    }
    if let Err(e) = std::fs::rename(&tmp, db_path) {
        let _ = std::fs::rename(&bak, db_path);
        remove_sqlite_and_wal(&tmp);
        return Err(AegisError::Io(e));
    }
    if let Err(e) = SqliteStore::open_encrypted_raw(db_path, key) {
        let _ = std::fs::rename(&bak, db_path);
        return Err(e);
    }
    remove_sqlite_and_wal(&bak);
    Ok(())
}

/// Outcome of opening the application vault on startup. Returned once at boot.
#[allow(clippy::large_enum_variant)]
pub enum VaultOpenResult {
    Ready {
        store: SqliteStore,
        key: DbKey,
        meta: VaultMeta,
        migrated: bool,
    },
    NeedsRecoveryKey {
        reason: String,
    },
}

/// Opens an existing encrypted database with DPAPI-unsealed key, creates and seals a new key on
/// fresh installs, migrates an existing plaintext `aegis.db` once, or requests recovery when an
/// encrypted DB has no usable local DPAPI key.
pub fn open_or_migrate_encrypted(db_path: &Path) -> Result<VaultOpenResult> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let key_file = sealed_key_path(db_path);
    let meta_file = vault_meta_path(db_path);
    let exists_nonempty = std::fs::metadata(db_path).map(|m| m.len() > 0).unwrap_or(false);

    if !exists_nonempty {
        let key = if key_file.is_file() {
            match unseal_key_from_file(&key_file) {
                Ok(k) => k,
                Err(_) => {
                    let k = DbKey::generate()?;
                    seal_key_to_file(&k, &key_file)?;
                    k
                }
            }
        } else {
            let k = DbKey::generate()?;
            seal_key_to_file(&k, &key_file)?;
            k
        };
        let store = SqliteStore::open_encrypted(db_path, &key)?;
        let fp = key.fingerprint();
        let meta = match load_vault_meta(&meta_file)? {
            Some(mut m) if m.key_fingerprint == fp => {
                m.cipher = CIPHER_LABEL.into();
                m
            }
            _ => VaultMeta {
                version: 1,
                cipher: CIPHER_LABEL.into(),
                key_fingerprint: fp,
                created_at: now_ms(),
                migrated_from_plaintext_at: None,
                recovery_confirmed_at: None,
            },
        };
        save_vault_meta(&meta_file, &meta)?;
        return Ok(VaultOpenResult::Ready {
            store,
            key,
            meta,
            migrated: false,
        });
    }

    if is_plaintext_sqlite(db_path)? {
        let key = if key_file.is_file() {
            match unseal_key_from_file(&key_file) {
                Ok(k) => k,
                Err(_) => {
                    let k = DbKey::generate()?;
                    seal_key_to_file(&k, &key_file)?;
                    k
                }
            }
        } else {
            let k = DbKey::generate()?;
            seal_key_to_file(&k, &key_file)?;
            k
        };
        migrate_plaintext_to_encrypted(db_path, &key)?;
        let store = SqliteStore::open_encrypted(db_path, &key)?;
        let now = now_ms();
        let meta = VaultMeta {
            version: 1,
            cipher: CIPHER_LABEL.into(),
            key_fingerprint: key.fingerprint(),
            created_at: now,
            migrated_from_plaintext_at: Some(now),
            recovery_confirmed_at: None,
        };
        save_vault_meta(&meta_file, &meta)?;
        return Ok(VaultOpenResult::Ready {
            store,
            key,
            meta,
            migrated: true,
        });
    }

    if !key_file.is_file() {
        return Ok(VaultOpenResult::NeedsRecoveryKey {
            reason: "This Cairn database is encrypted and has no sealed key on this PC. Enter your 24-word recovery phrase or 64-character hex key.".into(),
        });
    }

    let key = match unseal_key_from_file(&key_file) {
        Ok(k) => k,
        Err(_) => {
            return Ok(VaultOpenResult::NeedsRecoveryKey {
                reason: "The sealed database key belongs to another Windows account or PC. Enter your 24-word recovery key to unlock.".into(),
            });
        }
    };

    let store = match SqliteStore::open_encrypted(db_path, &key) {
        Ok(s) => s,
        Err(_) => {
            return Ok(VaultOpenResult::NeedsRecoveryKey {
                reason: "The stored sealed key did not unlock this encrypted database. Enter your 24-word recovery key.".into(),
            });
        }
    };

    let fp = key.fingerprint();
    let meta = match load_vault_meta(&meta_file)? {
        Some(mut m) if m.key_fingerprint == fp => {
            m.cipher = CIPHER_LABEL.into();
            m
        }
        old => VaultMeta {
            version: 1,
            cipher: CIPHER_LABEL.into(),
            key_fingerprint: fp,
            created_at: old.as_ref().map_or_else(now_ms, |m| m.created_at),
            migrated_from_plaintext_at: old.as_ref().and_then(|m| m.migrated_from_plaintext_at),
            recovery_confirmed_at: old.as_ref().and_then(|m| m.recovery_confirmed_at),
        },
    };
    save_vault_meta(&meta_file, &meta)?;
    Ok(VaultOpenResult::Ready {
        store,
        key,
        meta,
        migrated: false,
    })
}

/// Unlocks an encrypted database using a 24-word BIP-39 phrase or 64-char hex recovery key,
/// re-seals the key for this PC/Windows user (`R2`), and marks recovery backup confirmed.
pub fn recover_vault_with_key(db_path: &Path, input: &str) -> Result<(SqliteStore, DbKey, VaultMeta)> {
    let key = DbKey::from_recovery_input(input)?;
    let store = SqliteStore::open_encrypted(db_path, &key)?;
    seal_key_to_file(&key, &sealed_key_path(db_path))?;
    let meta_file = vault_meta_path(db_path);
    let now = now_ms();
    let mut meta = load_vault_meta(&meta_file)?.unwrap_or(VaultMeta {
        version: 1,
        cipher: CIPHER_LABEL.into(),
        key_fingerprint: key.fingerprint(),
        created_at: now,
        migrated_from_plaintext_at: None,
        recovery_confirmed_at: Some(now),
    });
    meta.cipher = CIPHER_LABEL.into();
    meta.key_fingerprint = key.fingerprint();
    if meta.recovery_confirmed_at.is_none() {
        meta.recovery_confirmed_at = Some(now);
    }
    save_vault_meta(&meta_file, &meta)?;
    Ok((store, key, meta))
}

/// Verifies at least two distinct 1-based word spot checks (`1..=24`) against `key` before marking
/// the owner's offline recovery key backup as confirmed.
pub fn confirm_recovery_backup(db_path: &Path, key: &DbKey, checks: &[(usize, String)]) -> Result<VaultMeta> {
    if checks.len() < 2 {
        return Err(AegisError::InvalidInput(
            "Verify at least two recovery words to confirm your offline backup.".into(),
        ));
    }
    let words = key.to_recovery_words_vec()?;
    let mut seen = HashSet::new();
    for (idx, answer) in checks {
        if *idx == 0 || *idx > words.len() {
            return Err(AegisError::InvalidInput(format!("Invalid word position #{idx}.")));
        }
        if !seen.insert(*idx) {
            return Err(AegisError::InvalidInput("Check two distinct word numbers.".into()));
        }
        let expected = &words[*idx - 1];
        if !answer.trim().eq_ignore_ascii_case(expected) {
            return Err(AegisError::InvalidInput(format!(
                "Word #{idx} does not match your recovery phrase."
            )));
        }
    }
    let meta_file = vault_meta_path(db_path);
    let now = now_ms();
    let mut meta = load_vault_meta(&meta_file)?.unwrap_or(VaultMeta {
        version: 1,
        cipher: CIPHER_LABEL.into(),
        key_fingerprint: key.fingerprint(),
        created_at: now,
        migrated_from_plaintext_at: None,
        recovery_confirmed_at: None,
    });
    meta.key_fingerprint = key.fingerprint();
    meta.recovery_confirmed_at = Some(now);
    save_vault_meta(&meta_file, &meta)?;
    Ok(meta)
}
