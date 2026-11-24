//! Offline discovery of local GGUF & Safetensors embedding models (headers/configs only; `K3`, `R9`).

use crate::fallback::{LEXICAL_DIM, LEXICAL_MODEL_ID};
use candle_core::quantized::gguf_file::{self, Value as GgufValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

/// 350 MB ceiling under North-Star `K5` so auto-selection prefers compact models like `nomic-embed-text`.
const K5_PREFER_MAX_BYTES: u64 = 350 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub name: String,
    pub path: String,
    pub backend: String,
    pub dim: usize,
    pub size_bytes: u64,
    pub source: String,
    pub within_budget: bool,
}

#[derive(Deserialize)]
struct OllamaManifest {
    #[serde(default)]
    layers: Vec<OllamaLayer>,
}

#[derive(Deserialize)]
struct OllamaLayer {
    #[serde(rename = "mediaType", default)]
    media_type: String,
    #[serde(default)]
    digest: String,
}

#[derive(Deserialize)]
struct HfBertConfig {
    #[serde(default)]
    model_type: String,
    hidden_size: usize,
}

/// Discovers local GGUF / Safetensors embedding models under `models_dir` and local Ollama blobs,
/// plus the always-available built-in lexical fallback.
pub fn discover_local_models(models_dir: &Path) -> Vec<ModelDescriptor> {
    let mut found = Vec::new();
    let mut seen_paths = BTreeSet::new();

    if models_dir.is_dir() {
        scan_models_dir(models_dir, &mut found, &mut seen_paths);
    }

    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let ollama_root = PathBuf::from(home).join(".ollama").join("models");
        scan_ollama_manifests(&ollama_root, &mut found, &mut seen_paths);
    }

    found.sort_by(|a, b| {
        b.within_budget
            .cmp(&a.within_budget)
            .then_with(|| a.size_bytes.cmp(&b.size_bytes))
            .then_with(|| a.name.cmp(&b.name))
    });

    found.push(ModelDescriptor {
        id: LEXICAL_MODEL_ID.to_string(),
        name: "Built-in Lexical Hash (fallback)".to_string(),
        path: LEXICAL_MODEL_ID.to_string(),
        backend: "lexical_fallback".to_string(),
        dim: LEXICAL_DIM,
        size_bytes: 0,
        source: "builtin".to_string(),
        within_budget: true,
    });

    found
}

fn scan_models_dir(dir: &Path, out: &mut Vec<ModelDescriptor>, seen: &mut BTreeSet<PathBuf>) {
    if is_safetensors_bert_dir(dir)
        && seen.insert(dir.to_path_buf())
        && let Some(desc) = inspect_safetensors_dir(dir, "app_models")
    {
        out.push(desc);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if is_safetensors_bert_dir(&p)
                && seen.insert(p.clone())
                && let Some(desc) = inspect_safetensors_dir(&p, "app_models")
            {
                out.push(desc);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            && seen.insert(p.clone())
            && let Some(desc) = inspect_gguf_header(&p, None, "app_models")
        {
            out.push(desc);
        }
    }
}

fn scan_ollama_manifests(
    ollama_root: &Path,
    out: &mut Vec<ModelDescriptor>,
    seen: &mut BTreeSet<PathBuf>,
) {
    let lib_dir = ollama_root
        .join("manifests")
        .join("registry.ollama.ai")
        .join("library");
    let blobs_dir = ollama_root.join("blobs");
    let Ok(models) = std::fs::read_dir(&lib_dir) else {
        return;
    };
    for model_entry in models.flatten() {
        let m_path = model_entry.path();
        if !m_path.is_dir() {
            continue;
        }
        let family = model_entry.file_name().to_string_lossy().to_string();
        if !is_likely_embed_family(&family) {
            continue;
        }
        let Ok(tags) = std::fs::read_dir(&m_path) else {
            continue;
        };
        for tag_entry in tags.flatten() {
            let tag_path = tag_entry.path();
            if !tag_path.is_file() {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&tag_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<OllamaManifest>(&raw) else {
                continue;
            };
            let Some(layer) = manifest
                .layers
                .iter()
                .find(|l| l.media_type.ends_with(".image.model"))
            else {
                continue;
            };
            let blob_name = layer.digest.replace(':', "-");
            let blob_path = blobs_dir.join(&blob_name);
            if !blob_path.is_file() || !seen.insert(blob_path.clone()) {
                continue;
            }
            let tag = tag_entry.file_name().to_string_lossy().to_string();
            let display = format!("{family}:{tag}");
            if let Some(desc) = inspect_gguf_header(&blob_path, Some(&display), "ollama_local") {
                out.push(desc);
            }
        }
    }
}

fn is_likely_embed_family(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("embed")
        || lower.contains("minilm")
        || lower.contains("bge")
        || lower.contains("e5")
        || lower.contains("arctic")
}

pub fn is_safetensors_bert_dir(dir: &Path) -> bool {
    dir.is_dir()
        && dir.join("config.json").is_file()
        && dir.join("vocab.txt").is_file()
        && dir.join("model.safetensors").is_file()
}

pub fn inspect_safetensors_dir(dir: &Path, source: &str) -> Option<ModelDescriptor> {
    let cfg_raw = std::fs::read_to_string(dir.join("config.json")).ok()?;
    let cfg: HfBertConfig = serde_json::from_str(&cfg_raw).ok()?;
    let mt = cfg.model_type.to_ascii_lowercase();
    if !mt.is_empty() && mt != "bert" {
        return None;
    }
    if cfg.hidden_size == 0 {
        return None;
    }
    let size_bytes = std::fs::metadata(dir.join("model.safetensors")).ok()?.len();
    let folder = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "local-bert".into());
    Some(ModelDescriptor {
        id: format!("safetensors:{}", folder.to_ascii_lowercase()),
        name: folder,
        path: dir.to_string_lossy().to_string(),
        backend: "candle_safetensors_bert".to_string(),
        dim: cfg.hidden_size,
        size_bytes,
        source: source.to_string(),
        within_budget: size_bytes <= K5_PREFER_MAX_BYTES,
    })
}

pub fn inspect_gguf_header(
    path: &Path,
    override_name: Option<&str>,
    source: &str,
) -> Option<ModelDescriptor> {
    let size_bytes = std::fs::metadata(path).ok()?.len();
    let mut f = File::open(path).ok()?;
    let content = gguf_file::Content::read(&mut f).ok()?;
    let arch = gguf_str(&content, "general.architecture")?;
    if !matches!(arch.as_str(), "nomic-bert" | "bert") {
        return None;
    }
    let dim_key = format!("{arch}.embedding_length");
    let dim = gguf_usize(&content, &dim_key)?;
    if dim == 0 {
        return None;
    }
    let gname = gguf_str(&content, "general.name").unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "gguf-embed".into())
    });
    let display = override_name.map(str::to_string).unwrap_or_else(|| gname.clone());
    let slug = gname
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c } else { '-' })
        .collect::<String>();
    Some(ModelDescriptor {
        id: format!("gguf:{slug}"),
        name: display,
        path: path.to_string_lossy().to_string(),
        backend: format!("candle_gguf_{}", arch.replace('-', "_")),
        dim,
        size_bytes,
        source: source.to_string(),
        within_budget: size_bytes <= K5_PREFER_MAX_BYTES,
    })
}

pub(crate) fn gguf_str(content: &gguf_file::Content, key: &str) -> Option<String> {
    match content.metadata.get(key)? {
        GgufValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn gguf_u32(content: &gguf_file::Content, key: &str) -> Option<u32> {
    match content.metadata.get(key)? {
        GgufValue::U32(v) => Some(*v),
        GgufValue::I32(v) if *v >= 0 => Some(*v as u32),
        GgufValue::U64(v) => u32::try_from(*v).ok(),
        _ => None,
    }
}

pub(crate) fn gguf_usize(content: &gguf_file::Content, key: &str) -> Option<usize> {
    gguf_u32(content, key).map(|v| v as usize)
}

pub(crate) fn gguf_f32(content: &gguf_file::Content, key: &str) -> Option<f32> {
    match content.metadata.get(key)? {
        GgufValue::F32(v) => Some(*v),
        GgufValue::F64(v) => Some(*v as f32),
        GgufValue::U32(v) => Some(*v as f32),
        _ => None,
    }
}
