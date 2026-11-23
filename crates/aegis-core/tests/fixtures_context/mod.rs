//! Folder trees shaped like the owner's `Documents\Projectsv2\Dev Set`, `C:\Prj` and ordinary
//! folders (spec §2), for the context-layer tests. Contents are small but distinct, so copies
//! that share a relative path share content too.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Content of a project file: depends only on its path inside the project, so two copies of a
/// project agree. Over 1 KB, so it counts as a non-trivial file.
pub fn body(rel: &str) -> Vec<u8> {
    format!("content of {rel}\n").repeat(60).into_bytes()
}

/// Writes `root/rel` with [`body`] of `key` (defaults to `rel`).
pub fn put_as(root: &Path, rel: &str, key: &str) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body(key)).unwrap();
    p
}

pub fn put(root: &Path, rel: &str) -> PathBuf {
    put_as(root, rel, rel)
}

/// A scanned PC: `me\Documents` (with `Projectsv2\Dev Set`), `me\Downloads`, `me\Pictures` and
/// `Prj`, all roots. Returns (tmp, store, home, prj).
pub fn pc() -> (tempfile::TempDir, aegis_core::db::SqliteStore, PathBuf, PathBuf) {
    let t = tempfile::TempDir::new().unwrap();
    let home = t.path().join("me");
    let prj = t.path().join("Prj");
    for f in ["Documents", "Downloads", "Pictures"] {
        std::fs::create_dir_all(home.join(f)).unwrap();
    }
    dev_set(&home.join("Documents"));
    self::prj(&prj);
    ordinary(&home);
    let mut s = aegis_core::db::SqliteStore::open_in_memory().unwrap();
    for r in [home.join("Documents"), home.join("Downloads"), home.join("Pictures"), prj.clone()] {
        s.add_root(&r).unwrap();
        aegis_core::scanner::scan_root(&mut s, &r, &[1]).unwrap();
    }
    aegis_core::filing::reader::read_pending(&mut s, 5_000, std::time::Duration::from_secs(30)).unwrap();
    private_quarantine(&s, t.path());
    (t, s, home, prj)
}

/// Points every drive's quarantine at a folder inside the test's own temporary folder, so a
/// test never leaves anything in the real `C:\.aegis-quarantine`.
pub fn private_quarantine(s: &aegis_core::db::SqliteStore, tmp: &Path) {
    let q = tmp.join(".quarantine");
    std::fs::create_dir_all(&q).unwrap();
    s.conn().execute("UPDATE volumes SET quarantine_dir = ?1", [q.to_string_lossy().to_string()]).unwrap();
}

pub fn roots_of(s: &aegis_core::db::SqliteStore) -> Vec<String> {
    s.list_roots().unwrap().into_iter().map(|r| r.path).collect()
}

/// Sets a file's modified time to `days` days ago.
pub fn age(p: &Path, days: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400)).unwrap();
}

/// Files of a project, written under `root/<project>/`.
fn project(root: &Path, name: &str, files: &[&str]) -> PathBuf {
    let dir = root.join(name);
    for f in files {
        put(&dir, f);
    }
    dir
}

const FOREX_SHARED: [&str; 9] = [
    "images/favicon_io/favicon-16x16.png",
    "set_files_v2(backup)/fx-favicon-32x32.png",
    "images/favicon_io/favicon-32x32.png",
    "images/favicon_io/apple-touch-icon.png",
    "images/FX Sentinel - logo(F)_v2.png",
    "engine/pyproject.toml",
    "engine/fx/signals.py",
    "engine/fx/backtest.py",
    "engine/fx/data.py",
];

const DRIVING_SHARED: [&str; 10] = [
    "src/index.html",
    "src/styles.css",
    "src/app.js",
    "src/booking.js",
    "src/lessons.js",
    "docs/PRD.md",
    "docs/roadmap.md",
    "images/logo.png",
    "images/hero.jpg",
    "notes.md",
];

/// `Documents\Projectsv2`: the `Dev Set` container of projects, plus `F1` (forexTrader's
/// logos and favicons kept apart). Returns the `Dev Set` folder.
pub fn dev_set(documents: &Path) -> PathBuf {
    let v2 = documents.join("Projectsv2");
    let ds = v2.join("Dev Set");
    let mut forex: Vec<&str> = FOREX_SHARED.to_vec();
    forex.extend([
        "images/favicon_io/android-chrome-192x192.png",
        "set_files_v2(backup)/fx-logo.png",
        "docs/architecture.md",
        "docs/strategy notes.md",
        "engine/fx/live.py",
        "engine/fx/risk.py",
    ]);
    project(&ds, "forexTrader", &forex);
    project(
        &ds,
        "paperPilot",
        &["images/logo_v3.jpg", "set_files/TASKS.md", "set_files/PRD.md", "docs/user journeys.md", "docs/pricing.md"],
    );
    project(
        &ds,
        "discipline_app",
        &["set_files/PRD_additions.md", "src/App.tsx", "src/screens/Home.tsx", "src/screens/Log.tsx", "src/store.ts"],
    );
    project(&ds, "chatbot", &["persona.md", "conversation flows.docx", "ui/welcome.png", "ui/chat.png", "notes.md"]);
    project(
        &ds,
        "AIPrompt",
        &[
            "prompts/system prompt.md",
            "prompts/rules.md",
            "template/prompt-pigeon/.git/HEAD",
            "template/prompt-pigeon/index.ts",
            "template/prompt-pigeon/prompt.ts",
        ],
    );
    project(&ds, "Nutrition", &["Nutrition PRD.md", "meal plan.md", "recipes.md", "research.md"]);
    // No project words at all: only where it sits says it is a project.
    project(&ds, "AgentsM", &["agents overview.docx", "agent roles.md", "orchestration.md"]);
    let mut driving: Vec<&str> = DRIVING_SHARED.to_vec();
    driving.extend(["docs/old pricing.md", "src/legacy.js"]);
    let dd = project(&ds, "DrivingSchool", &driving);
    for f in &driving {
        age(&dd.join(f), if DRIVING_SHARED.contains(f) { 60 } else { 30 });
    }
    // F1 holds the same forexTrader favicons and logos, by themselves.
    put_as(&v2.join("F1"), "fx-favicon-32x32.png", "set_files_v2(backup)/fx-favicon-32x32.png");
    put_as(&v2.join("F1"), "favicon-16x16.png", "images/favicon_io/favicon-16x16.png");
    put_as(&v2.join("F1"), "FX Sentinel - logo(F)_v2.png", "images/FX Sentinel - logo(F)_v2.png");
    ds
}

/// `C:\Prj`: an older forexTrader, a diverged DrivingSchool, the paperPilot code (mostly
/// `node_modules`), and the `DevSet` container holding `mccShepherd` and `wellness`.
pub fn prj(prj: &Path) {
    let fx = project(prj, "forexTrader", &FOREX_SHARED);
    for f in FOREX_SHARED {
        age(&fx.join(f), 90);
    }
    let mut driving: Vec<&str> = DRIVING_SHARED.to_vec();
    driving.extend(["src/payments.js", "src/instructors.js", "docs/launch plan.md"]);
    project(prj, "DrivingSchool", &driving);
    let pp = project(prj, "paperPilot", &["package.json", "README.md", "src/main.tsx", "src/App.tsx", "src/pages/Upload.tsx"]);
    for i in 0..300 {
        put(&pp, &format!("node_modules/pkg{}/index{i}.js", i % 30));
    }
    let devset = prj.join("DevSet");
    project(
        &devset,
        "mccShepherd",
        &[
            "all docs/archive/PRD_additions.md",
            "all docs/others/G5_dual_ai_human_orchestration_playbook.md",
            "all docs/others/G5_roles.md",
            "lib/main.dart",
            "lib/members.dart",
            "pubspec.yaml",
        ],
    );
    project(&devset, "wellness", &["prd.md", "src/app.py", "src/habits.py", "design/logo.png"]);
}

/// Ordinary folders that are not projects: a Downloads dump, a photo month, and a CVs folder.
pub fn ordinary(home: &Path) {
    let dl = home.join("Downloads").join("misc");
    for f in ["vlc-3.0.20-win64.exe", "zoom installer.exe", "bank statement.pdf", "flight ticket.pdf", "song.mp3"] {
        put(&dl, f);
    }
    let month = home.join("Pictures").join("2026").join("2026-10");
    for i in 1..=6 {
        put(&month, &format!("IMG_20261012_{i:03}.jpg"));
    }
    let cvs = home.join("Documents").join("CVs");
    for f in ["CV 2025.docx", "cv_short.docx", "cover letter.docx"] {
        put(&cvs, f);
    }
}
