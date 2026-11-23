//! Resources module (spec 2026-11-22 §1A, §16 step 7): recognises downloaded UI kits, skills,
//! templates, and design packs (`glassmorphism`, `antigravity-ui-motion-design-expert`,
//! `stitch_pwa_screen_interface_design (2)`), keeps each whole, and routes it under Fresh to
//! `Documents\Resources\<topic>`.

use super::ClaimedIndex;
use super::profile::FolderProfile;
use super::projects::{AREA_NAMES, LIFE_AREAS};
use crate::filing::Kind;
use std::collections::HashMap;
use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Recognised reusable resource kit or template folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceCandidate {
    pub root: String,
    pub volume_id: String,
    pub name: String,
    /// Topic subfolder inside `Documents\Resources`: `"Skills"`, `"UI & Design"`, or `"Templates"`.
    pub topic: &'static str,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub files: i64,
    pub bytes: i64,
    pub newest_mtime: i64,
}

fn key(p: &str) -> String {
    p.to_lowercase()
}

fn has_resource_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("stitch_")
        || [
            "ui kit",
            "ui_kit",
            "uikit",
            "design pack",
            "design system",
            "icon pack",
            "wireframe kit",
            "mockup pack",
            "glassmorphism",
            "notion template",
            "templates",
            "presets",
            "brushes",
            "luts",
            "font pack",
        ]
        .iter()
        .any(|w| n.contains(w))
}

fn qualify(p: &FolderProfile) -> Option<(&'static str, f64, &'static str)> {
    if p.has_git || p.atomic_unit || p.exe_files > 0 || p.files == 0 || p.files > 300 {
        return None;
    }
    if !p.manifests.iter().all(|(m, _)| m.eq_ignore_ascii_case("index.html")) {
        return None;
    }
    let media = p.kind_count(Kind::Video) + p.kind_count(Kind::Music) + p.kind_count(Kind::Audio);
    if media * 10 >= p.files * 5 {
        return None;
    }
    if p.has_skill_file && p.files <= 50 {
        return Some(("Skills", 0.90, "SKILL.md instruction pack"));
    }
    if p.has_design_kit_file {
        return Some(("UI & Design", 0.90, "design kit with spec or tokens"));
    }
    if has_resource_name(p.name()) {
        let usable = p.kind_count(Kind::Document) + p.asset_files + p.kind_count(Kind::Font) + p.source_files;
        if usable > 0 {
            let topic = if p.name().to_lowercase().contains("template") {
                "Templates"
            } else {
                "UI & Design"
            };
            return Some((topic, 0.85, "reusable kit or template pack"));
        }
    }
    None
}

fn is_shelf(p: &FolderProfile, roots: &[String]) -> bool {
    if p.depth == 0 || roots.iter().any(|r| r.eq_ignore_ascii_case(&p.path)) {
        return true;
    }
    let n = p.name().to_lowercase();
    if n == "resources" {
        return true;
    }
    if p.depth <= 1 && (AREA_NAMES.contains(&n.as_str()) || LIFE_AREAS.contains(&n.as_str())) {
        return true;
    }
    if matches!(n.as_str(), "skills" | "ui & design" | "templates")
        && let Some(pn) = Path::new(&p.path).parent().and_then(|d| d.file_name()).and_then(|s| s.to_str())
    {
        return pn.eq_ignore_ascii_case("resources");
    }
    false
}

struct ResWalker<'a> {
    by_path: HashMap<String, &'a FolderProfile>,
    roots: Vec<String>,
    claimed: ClaimedIndex,
    containers: HashMap<String, bool>,
}

impl<'a> ResWalker<'a> {
    fn get(&self, p: &str) -> Option<&'a FolderProfile> {
        self.by_path.get(&key(p)).copied()
    }

    fn children(&self, p: &FolderProfile) -> Vec<&'a FolderProfile> {
        p.child_dirs.iter().filter_map(|c| self.get(c)).collect()
    }

    fn is_container(&mut self, p: &FolderProfile) -> bool {
        if let Some(c) = self.containers.get(&key(&p.path)) {
            return *c;
        }
        let mut q = 0usize;
        for c in self.children(p) {
            if self.claimed.overlaps(&c.volume_id, &c.path).0 {
                continue;
            }
            if qualify(c).is_some() || self.is_container(c) {
                q += 1;
            }
        }
        let res = q >= 2 && p.loose_files <= 5;
        self.containers.insert(key(&p.path), res);
        res
    }

    fn visit(&mut self, p: &FolderProfile, out: &mut Vec<ResourceCandidate>) {
        let (inside, holds) = self.claimed.overlaps(&p.volume_id, &p.path);
        if inside {
            return;
        }
        if holds || is_shelf(p, &self.roots) || self.is_container(p) {
            for c in self.children(p) {
                self.visit(c, out);
            }
            return;
        }
        if let Some((topic, confidence, why)) = qualify(p) {
            let name = p.path.rsplit(SEP).next().unwrap_or(&p.path).to_string();
            out.push(ResourceCandidate {
                root: p.path.clone(),
                volume_id: p.volume_id.clone(),
                name,
                topic,
                confidence,
                evidence: vec![format!("resource ({why}): keeps {} files together", p.files)],
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

/// Finds reusable resource kits and templates under `known_roots` outside `claimed_roots`.
pub fn find_resources(
    profiles: &[FolderProfile],
    known_roots: &[String],
    claimed_roots: &[(Option<String>, String)],
) -> Vec<ResourceCandidate> {
    let mut walker = ResWalker {
        by_path: profiles.iter().map(|p| (key(&p.path), p)).collect(),
        roots: known_roots.to_vec(),
        claimed: ClaimedIndex::new(claimed_roots),
        containers: HashMap::new(),
    };
    let mut out = Vec::new();
    for p in profiles.iter().filter(|p| p.depth == 0) {
        walker.visit(p, &mut out);
    }
    out.sort_by_key(|r| r.root.to_lowercase());
    out
}
