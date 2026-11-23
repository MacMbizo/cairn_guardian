use std::env;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let src_dir = sqlcipher_src::source_dir();
    let ltc_inc = src_dir.join(sqlcipher_src::LIBTOMCRYPT_INCLUDE_DIR);
    let sqlite_src = src_dir.join(sqlcipher_src::SOURCE_FILE);

    let msvc_static_crt = env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|v| v.split(',').any(|tf| tf == "crt-static"))
        && env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");

    // 1. Copy pre-generated SQLCipher bindings to $OUT_DIR/bindgen.rs.
    let bindings_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("sqlcipher")
        .join("bindgen_bundled_version.rs");
    std::fs::copy(&bindings_src, out_dir.join("bindgen.rs")).expect("copy bindgen.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=sqlcipher/bindgen_bundled_version.rs");
    println!("cargo:include={}", src_dir.display());
    println!("cargo:lib_dir={}", out_dir.display());

    // 2. Compile LibTomCrypt sources without inline asm so MSVC builds cleanly with zero Perl/OpenSSL.
    let mut ltc = cc::Build::new();
    ltc.include(&ltc_inc)
        .define("LTC_SOURCE", None)
        .define("LTC_NO_ASM", None)
        .define("LTC_NO_TEST", None)
        .warnings(false);
    if msvc_static_crt {
        ltc.static_crt(true);
    }
    for rel in sqlcipher_src::LIBTOMCRYPT_SOURCES {
        ltc.file(src_dir.join(rel));
    }
    ltc.compile("tomcrypt");

    // 3. Compile SQLCipher amalgamation wired to LibTomCrypt.
    let mut sql = cc::Build::new();
    sql.file(&sqlite_src)
        .include(src_dir)
        .include(&ltc_inc)
        .warnings(false)
        .define("SQLITE_CORE", None)
        .define("SQLITE_DEFAULT_FOREIGN_KEYS", Some("1"))
        .define("SQLITE_ENABLE_API_ARMOR", None)
        .define("SQLITE_ENABLE_COLUMN_METADATA", None)
        .define("SQLITE_ENABLE_DBSTAT_VTAB", None)
        .define("SQLITE_ENABLE_FTS3", None)
        .define("SQLITE_ENABLE_FTS3_PARENTHESIS", None)
        .define("SQLITE_ENABLE_FTS5", None)
        .define("SQLITE_ENABLE_JSON1", None)
        .define("SQLITE_ENABLE_LOAD_EXTENSION", Some("1"))
        .define("SQLITE_ENABLE_MEMORY_MANAGEMENT", None)
        .define("SQLITE_ENABLE_RTREE", None)
        .define("SQLITE_ENABLE_STAT4", None)
        .define("SQLITE_SOUNDEX", None)
        .define("SQLITE_THREADSAFE", Some("1"))
        .define("SQLITE_USE_URI", None)
        .define("HAVE_USLEEP", Some("1"))
        .define("HAVE_ISNAN", None)
        .define("HAVE_STDINT_H", Some("1"))
        .define("SQLITE_HAS_CODEC", None)
        .define("SQLCIPHER_CRYPTO_LIBTOMCRYPT", None)
        .define("SQLITE_TEMP_STORE", Some("2"))
        .define("SQLITE_EXTRA_INIT", Some("sqlcipher_extra_init"))
        .define("SQLITE_EXTRA_SHUTDOWN", Some("sqlcipher_extra_shutdown"));

    if cfg!(feature = "unlock_notify") {
        sql.define("SQLITE_ENABLE_UNLOCK_NOTIFY", None);
    }
    if msvc_static_crt {
        sql.static_crt(true);
    }
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        // LibTomCrypt's _rng_win32 links CryptAcquireContext / CryptGenRandom from advapi32.
        println!("cargo:rustc-link-lib=advapi32");
    } else {
        sql.define("HAVE_LOCALTIME_R", None);
    }

    sql.compile("sqlite3");
}
