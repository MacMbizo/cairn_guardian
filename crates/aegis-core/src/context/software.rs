//! Software packages module (spec 2026-11-22 §1A, §16 step 7): recognises folders holding
//! executables with runtimes/configs/installers (`DDU v18.1.5.7`, `Armoury_Crate_Uninstall_Tool`,
//! `drivers`, Office/Acrobat installers) so they stay whole instead of being split by extension,
//! and routes them in Fresh to `Software\Tools\<name>` or `Software\Installers\<name>`.

use super::ClaimedIndex;
use super::profile::FolderProfile;
use super::projects::{AREA_NAMES, LIFE_AREAS};
use std::collections::HashMap;
use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Subfolder names that are internal components of one software package, installer bundle, or SDK
/// rather than standalone packages of their own (`Microsoft Office ...\files\x64`,
/// `Adobe CS6\payloads`, `Windows Kits\10\bin`, `android-studio\jre\bin`).
const INTERNAL_SW_DIRS: [&str; 29] = [
    "bin", "lib", "libs", "jre", "jdk", "plugins", "plugin", "payloads", "payload",
    "files", "office", "package", "packages", "catalogs", "remote", "testing",
    "x64", "x86", "win32", "win64", "arm", "arm64", "amd64",
    "common", "redist", "runtime", "runtimes", "resources", "support",
];

fn is_internal_sw_dir(name: &str) -> bool {
    INTERNAL_SW_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d))
}

/// Recognised software package folder on one volume.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftwareCandidate {
    pub root: String,
    pub volume_id: String,
    pub name: String,
    /// Destination subfolder under `Software`: `"tools"` or `"installers"`.
    pub bucket: &'static str,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
}

/// True when `p` looks like a compiled software tool, driver pack, or multi-file installer
/// rather than source code: holds executables/installers alongside runtimes, configs, or setup
/// files, without git or code manifests.
pub(crate) fn looks_like_software_package(p: &FolderProfile) -> bool {
    if p.has_git
        || p.atomic_unit
        || p.manifests.iter().any(|(m, _)| !m.eq_ignore_ascii_case("index.html"))
        || p.source_files >= 3
        || p.files < 2
    {
        return false;
    }
    (p.exe_files >= 1 && (p.runtime_files >= 1 || p.installer_files >= 1))
        || (p.exe_files >= 3 && p.exe_files * 10 >= p.files * 7)
        || (p.installer_files >= 3 && (p.installer_files + p.runtime_files) * 10 >= p.files * 5)
}

fn key(p: &str) -> String {
    p.to_lowercase()
}

fn bucket_for(p: &FolderProfile) -> &'static str {
    let n = p.name().to_lowercase();
    let portable_or_tool = n.contains("portable") || n.contains("uninstall");
    let installer_name = !portable_or_tool
        && ["driver", "installer", "setup", "repack", "activator", "cracked", "office", "acrobat"]
            .iter()
            .any(|w| n.contains(w));
    if p.installer_files >= 1 || p.has_setup_file || (p.exe_files >= 3 && p.runtime_files == 0) || installer_name {
        "installers"
    } else {
        "tools"
    }
}

fn is_software_shelf(p: &FolderProfile, roots: &[String]) -> bool {
    let n = p.name().to_lowercase();
    if p.depth == 0 || roots.iter().any(|r| r.eq_ignore_ascii_case(&p.path)) {
        return true;
    }
    if p.depth <= 1 && (AREA_NAMES.contains(&n.as_str()) || LIFE_AREAS.contains(&n.as_str())) {
        return true;
    }
    if matches!(n.as_str(), "tools" | "installers" | "fonts")
        && let Some(parent_name) = Path::new(&p.path).parent().and_then(|d| d.file_name()).and_then(|s| s.to_str())
    {
        return parent_name.eq_ignore_ascii_case("software");
    }
    false
}

struct SwWalker<'a> {
    by_path: HashMap<String, &'a FolderProfile>,
    roots: Vec<String>,
    claimed: ClaimedIndex,
    container_cache: HashMap<String, bool>,
}

impl<'a> SwWalker<'a> {
    fn get(&self, path: &str) -> Option<&'a FolderProfile> {
        self.by_path.get(&key(path)).copied()
    }

    fn children(&self, p: &FolderProfile) -> Vec<&'a FolderProfile> {
        p.child_dirs.iter().filter_map(|c| self.get(c)).collect()
    }

    fn is_container(&mut self, p: &FolderProfile) -> bool {
        if let Some(c) = self.container_cache.get(&key(&p.path)) {
            return *c;
        }
        if !is_software_shelf(p, &self.roots) && p.depth >= 2 && is_internal_sw_dir(p.name()) {
            self.container_cache.insert(key(&p.path), false);
            return false;
        }
        let mut qualifying = 0usize;
        for c in self.children(p) {
            if self.claimed.overlaps(&c.volume_id, &c.path).0 {
                continue;
            }
            if !is_software_shelf(c, &self.roots) && c.depth >= 2 && is_internal_sw_dir(c.name()) {
                continue;
            }
            if looks_like_software_package(c) || self.is_container(c) {
                qualifying += 1;
            }
        }
        let res = qualifying >= 2 && p.loose_files <= 5;
        self.container_cache.insert(key(&p.path), res);
        res
    }

    fn visit(&mut self, p: &FolderProfile, out: &mut Vec<SoftwareCandidate>) {
        let (inside_claimed, holds_claimed) = self.claimed.overlaps(&p.volume_id, &p.path);
        if inside_claimed {
            return;
        }
        let shelf = is_software_shelf(p, &self.roots);
        if !shelf && p.depth >= 2 && is_internal_sw_dir(p.name()) {
            return;
        }
        if holds_claimed || shelf || self.is_container(p) {
            for c in self.children(p) {
                self.visit(c, out);
            }
            return;
        }
        if looks_like_software_package(p) {
            let bucket = bucket_for(p);
            let strong = p.exe_files >= 1 && (p.runtime_files + p.installer_files >= 1 || p.has_setup_file || p.exe_files >= 3);
            let confidence = if strong { 0.90 } else { 0.80 };
            let mut parts = Vec::new();
            if p.exe_files > 0 {
                parts.push(format!("{} executable{}", p.exe_files, if p.exe_files == 1 { "" } else { "s" }));
            }
            if p.runtime_files > 0 {
                parts.push(format!("{} runtime/config", p.runtime_files));
            }
            if p.installer_files > 0 {
                parts.push(format!("{} installer package files", p.installer_files));
            }
            let evidence = vec![format!(
                "software {} ({}): keeps {} files together",
                if bucket == "installers" { "installer" } else { "tool" },
                parts.join(", "),
                p.files
            )];
            let name = p.path.rsplit(SEP).next().unwrap_or(&p.path).to_string();
            out.push(SoftwareCandidate {
                root: p.path.clone(),
                volume_id: p.volume_id.clone(),
                name,
                bucket,
                confidence,
                evidence,
                files: p.files,
                bytes: p.bytes,
                newest_mtime: p.newest_mtime,
            });
            return;
        }
        for c in self.children(p) {
            self.visit(c, out);
        }
    }
}

/// Finds all software package folders under `roots` outside already-claimed `claimed_roots`
/// (projects and old copies). Never touches disk.
pub fn find_software_packages(
    profiles: &[FolderProfile],
    known_roots: &[String],
    claimed_roots: &[(Option<String>, String)],
) -> Vec<SoftwareCandidate> {
    let mut walker = SwWalker {
        by_path: profiles.iter().map(|p| (key(&p.path), p)).collect(),
        roots: known_roots.to_vec(),
        claimed: ClaimedIndex::new(claimed_roots),
        container_cache: HashMap::new(),
    };
    let mut out = Vec::new();
    for p in profiles.iter().filter(|p| p.depth == 0) {
        walker.visit(p, &mut out);
    }
    out.sort_by_key(|a| a.root.to_lowercase());
    out
}
