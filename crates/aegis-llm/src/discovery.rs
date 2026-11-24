//! Local chat LLM model discovery over Ollama manifests and standalone GGUF files (`K3`, `K5`, `R4`).

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use candle_core::quantized::gguf_file::{self, Value as GgufValue};
use serde::{Deserialize, Serialize};

/// Preferred auto-selection ceiling (`K5` / `R4`): models <= 4,500 MB are marked `recommended`.
pub const K5_LLM_RECOMMEND_MAX_BYTES: u64 = 4_500 * 1024 * 1024;

/// Models larger than 2.5 GB unload immediately after each generation (`K5`).
pub const K5_LLM_KEEP_LOADED_MAX_BYTES: u64 = 2_500 * 1024 * 1024;

/// Built-in zero-RAM extractive fallback ID.
pub const BUILTIN_EXTRACTIVE_ID: &str = "builtin:extractive";

const MAX_MANIFEST_WALK_DEPTH: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelDescriptor {
    pub id: String,
    pub name: String,
    pub path: String,
    pub backend: String,
    pub architecture: String,
    pub context_length: usize,
    pub size_bytes: u64,
    pub source: String,
    pub recommended: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaManifest {
    #[serde(default)]
    layers: Vec<OllamaLayer>,
}

#[derive(Debug, Deserialize)]
struct OllamaLayer {
    #[serde(rename = "mediaType", default)]
    media_type: String,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    size: u64,
}

/// Returns the always-available built-in zero-RAM extractive fallback descriptor.
pub fn builtin_extractive_descriptor() -> LlmModelDescriptor {
    LlmModelDescriptor {
        id: BUILTIN_EXTRACTIVE_ID.to_string(),
        name: "Built-in Extractive Summarizer (0 MB)".to_string(),
        path: BUILTIN_EXTRACTIVE_ID.to_string(),
        backend: "builtin_extractive".to_string(),
        architecture: "extractive".to_string(),
        context_length: 8192,
        size_bytes: 0,
        source: "builtin".to_string(),
        recommended: true,
    }
}

/// Discovers supported local GGUF LLMs under `models_dir` (recursive Ollama `manifests/` + standalone `*.gguf`),
/// plus the built-in zero-RAM extractive fallback at the end.
pub fn discover_local_llms(models_dir: &Path) -> Vec<LlmModelDescriptor> {
    discover_local_llms_in_dirs(&[models_dir.to_path_buf()])
}

/// Discovers supported local GGUF LLMs across multiple directories (deduplicated by path),
/// sorting recommended models (<= 4,500 MB) and smaller models first, and appending `builtin:extractive`.
pub fn discover_local_llms_in_dirs(dirs: &[PathBuf]) -> Vec<LlmModelDescriptor> {
    let mut ggufs = Vec::new();
    let mut seen_paths = BTreeSet::new();

    for models_dir in dirs {
        let manifests_root = models_dir.join("manifests");
        if manifests_root.is_dir() {
            walk_manifest_files(
                &manifests_root,
                &manifests_root,
                models_dir,
                0,
                &mut ggufs,
                &mut seen_paths,
            );
        }

        if models_dir.is_dir()
            && let Ok(entries) = fs::read_dir(models_dir)
        {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("gguf"))
                    && p.is_file()
                {
                    let norm = normalize_path_key(&p);
                    if seen_paths.contains(&norm) {
                        continue;
                    }
                    if let Some(desc) = inspect_llm_gguf_header(&p, None, "local_gguf") {
                        seen_paths.insert(norm);
                        ggufs.push(desc);
                    }
                }
            }
        }
    }

    // Sort recommended (<= 4,500 MB) first, then smallest size_bytes ascending, then name.
    ggufs.sort_by(|a, b| {
        b.recommended
            .cmp(&a.recommended)
            .then_with(|| a.size_bytes.cmp(&b.size_bytes))
            .then_with(|| a.name.cmp(&b.name))
    });

    ggufs.push(builtin_extractive_descriptor());
    ggufs
}

fn walk_manifest_files(
    root: &Path,
    dir: &Path,
    models_dir: &Path,
    depth: usize,
    out: &mut Vec<LlmModelDescriptor>,
    seen_paths: &mut BTreeSet<String>,
) {
    if depth > MAX_MANIFEST_WALK_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();

    for path in paths {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_manifest_files(root, &path, models_dir, depth + 1, out, seen_paths);
        } else if meta.is_file()
            && let Some(desc) = inspect_ollama_manifest(root, &path, models_dir)
        {
            let norm = normalize_path_key(Path::new(&desc.path));
            if seen_paths.insert(norm) {
                out.push(desc);
            }
        }
    }
}

fn inspect_ollama_manifest(
    manifests_root: &Path,
    manifest_path: &Path,
    models_dir: &Path,
) -> Option<LlmModelDescriptor> {
    let raw = fs::read_to_string(manifest_path).ok()?;
    let manifest: OllamaManifest = serde_json::from_str(&raw).ok()?;
    let layer = manifest
        .layers
        .iter()
        .find(|l| l.media_type == "application/vnd.ollama.image.model")?;

    let hex = layer.digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let blob_path = models_dir
        .join("blobs")
        .join(format!("sha256-{}", hex.to_ascii_lowercase()));
    let blob_meta = fs::metadata(&blob_path).ok()?;
    if !blob_meta.is_file() {
        return None;
    }
    if layer.size > 0 && blob_meta.len() != layer.size {
        return None;
    }

    let display = derive_ollama_display_name(manifests_root, manifest_path);
    inspect_llm_gguf_header(&blob_path, Some(&display), "ollama_local")
}

fn derive_ollama_display_name(manifests_root: &Path, manifest_path: &Path) -> String {
    let rel = manifest_path.strip_prefix(manifests_root).unwrap_or(manifest_path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    match parts.as_slice() {
        [host, lib, family, tag]
            if host.eq_ignore_ascii_case("registry.ollama.ai") && lib == "library" =>
        {
            format!("{family}:{tag}")
        }
        [_, rest @ .., tag] if !rest.is_empty() => {
            format!("{}:{tag}", rest.join("/"))
        }
        [family, tag] => format!("{family}:{tag}"),
        [single] => single.clone(),
        _ => manifest_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "local-llm".to_string()),
    }
}

/// Inspects a GGUF file header without loading weights, returning `Some` if it is a supported causal LLM.
pub fn inspect_llm_gguf_header(
    path: &Path,
    override_name: Option<&str>,
    source: &str,
) -> Option<LlmModelDescriptor> {
    let size_bytes = fs::metadata(path).ok()?.len();
    let mut file = File::open(path).ok()?;
    let content = gguf_file::Content::read(&mut file).ok()?;
    let arch = gguf_str(&content, "general.architecture")?;
    if !matches!(arch.as_str(), "llama" | "qwen2") {
        return None;
    }

    // Must look like a causal decoder model (at least 1 block and embedding_length > 0).
    let n_layers = gguf_usize(&content, &format!("{arch}.block_count"))?;
    let hidden = gguf_usize(&content, &format!("{arch}.embedding_length"))?;
    if n_layers == 0 || hidden == 0 {
        return None;
    }

    let ctx_len = gguf_usize(&content, &format!("{arch}.context_length")).unwrap_or(2048);
    let gname = gguf_str(&content, "general.name").unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "local-llm".to_string())
    });
    let display = override_name
        .map(str::to_string)
        .unwrap_or_else(|| gname.clone());
    let slug_src = override_name.unwrap_or(&gname);
    let slug: String = slug_src
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':') {
                c
            } else {
                '-'
            }
        })
        .collect();

    Some(LlmModelDescriptor {
        id: format!("gguf:{slug}"),
        name: display,
        path: path.to_string_lossy().to_string(),
        backend: format!("candle_gguf_{arch}"),
        architecture: arch,
        context_length: ctx_len,
        size_bytes,
        source: source.to_string(),
        recommended: size_bytes <= K5_LLM_RECOMMEND_MAX_BYTES,
    })
}

fn normalize_path_key(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        resolved.to_string_lossy().to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        resolved.to_string_lossy().to_string()
    }
}

pub(crate) fn gguf_str(content: &gguf_file::Content, key: &str) -> Option<String> {
    match content.metadata.get(key)? {
        GgufValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn gguf_bool(content: &gguf_file::Content, key: &str) -> Option<bool> {
    match content.metadata.get(key)? {
        GgufValue::Bool(b) => Some(*b),
        _ => None,
    }
}

pub(crate) fn gguf_u32(content: &gguf_file::Content, key: &str) -> Option<u32> {
    match content.metadata.get(key)? {
        GgufValue::U32(v) => Some(*v),
        GgufValue::U64(v) => u32::try_from(*v).ok(),
        GgufValue::I32(v) => u32::try_from(*v).ok(),
        _ => None,
    }
}

pub(crate) fn gguf_usize(content: &gguf_file::Content, key: &str) -> Option<usize> {
    match content.metadata.get(key)? {
        GgufValue::U32(v) => Some(*v as usize),
        GgufValue::U64(v) => usize::try_from(*v).ok(),
        GgufValue::I32(v) => usize::try_from(*v).ok(),
        _ => None,
    }
}

pub(crate) fn gguf_f32(content: &gguf_file::Content, key: &str) -> Option<f32> {
    match content.metadata.get(key)? {
        GgufValue::F32(v) => Some(*v),
        GgufValue::F64(v) => Some(*v as f32),
        _ => None,
    }
}
