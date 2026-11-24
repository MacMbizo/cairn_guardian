//! Hardware profile detection (`ADR-002`) and pure-Rust local embedding engine (`K3`, `K5`, `R4`, `R9`).

pub mod discovery;
pub mod fallback;
pub mod gguf;
pub mod manager;
pub mod safetensors;
pub mod tokenizer;

use aegis_core::contracts::HardwareProfile;
use serde::{Deserialize, Serialize};
use sysinfo::System;
use thiserror::Error;

pub use discovery::{ModelDescriptor, discover_local_models};
pub use fallback::{LEXICAL_DIM, LEXICAL_MODEL_ID, LexicalHashEmbedder};
pub use gguf::GgufEmbedder;
pub use manager::{ModelSelection, OnDemandEmbedder};
pub use safetensors::SafetensorsBertEmbedder;
pub use tokenizer::WordPieceTokenizer;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("embedding model not loaded: {0}")]
    ModelNotLoaded(String),
    #[error("embedding model load error: {0}")]
    ModelLoad(String),
    #[error("embedding inference error: {0}")]
    Inference(String),
    #[error("vector store error: {0}")]
    VectorStore(String),
}

/// Hardware inspection summary driving `Eco` / `Standard` / `Pro` profile selection (`B1/B2` §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareCapabilities {
    pub profile: HardwareProfile,
    pub total_ram_bytes: u64,
    pub logical_cores: usize,
    pub recommended_embed_dim: usize,
    pub llm_allowed: bool,
}

/// Detects host hardware profile (`Eco <12GB`, `Standard 12-24GB`, `Pro >=24GB` RAM).
pub fn detect_hardware_capabilities() -> HardwareCapabilities {
    let (total_ram_bytes, logical_cores) = match System::new() {
        Ok(mut sys) => {
            sys.refresh_memory();
            sys.refresh_cpu_all();
            (sys.total_memory(), sys.cpus().len().max(1))
        }
        Err(_) => (0, 1),
    };

    let ram_gb = total_ram_bytes / (1024 * 1024 * 1024);

    let (profile, recommended_embed_dim, llm_allowed) = if ram_gb >= 24 {
        (HardwareProfile::Pro, 1024, true)
    } else if ram_gb >= 12 {
        (HardwareProfile::Standard, 384, true)
    } else {
        (HardwareProfile::Eco, 384, false)
    };

    HardwareCapabilities {
        profile,
        total_ram_bytes,
        logical_cores,
        recommended_embed_dim,
        llm_allowed,
    }
}

/// Local text embedding provider trait implemented cleanly without outbound network access (`R9`).
pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn display_name(&self) -> &str {
        self.model_id()
    }
    fn backend_kind(&self) -> &str {
        "custom"
    }
    fn dimensions(&self) -> usize;
    fn embed_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError>;

    fn embed_documents(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        self.embed_batch(texts)
    }

    fn embed_query(&self, query: &str) -> std::result::Result<Vec<f32>, EmbedError> {
        let mut out = self.embed_documents(&[query])?;
        out.pop()
            .ok_or_else(|| EmbedError::Inference("empty query embedding".into()))
    }
}

/// Legacy vector store trait kept for compatibility.
pub trait VectorStore: Send + Sync {
    fn upsert_points(
        &self,
        collection: &str,
        ids: &[String],
        vectors: &[Vec<f32>],
        payloads_json: &[String],
    ) -> std::result::Result<(), EmbedError>;

    fn delete_points(
        &self,
        collection: &str,
        ids: &[String],
    ) -> std::result::Result<(), EmbedError>;
}

/// Cosine similarity between two equal-length L2-normalized (or general) vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let fx = f64::from(x);
        let fy = f64::from(y);
        dot += fx * fy;
        na += fx * fx;
        nb += fy * fy;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom > 0.0 {
        (dot / denom) as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn detects_host_profile() {
        let caps = detect_hardware_capabilities();
        assert!(caps.logical_cores >= 1);
        assert!(caps.total_ram_bytes > 0);
        assert!(caps.recommended_embed_dim == 384 || caps.recommended_embed_dim == 1024);
    }

    #[test]
    fn wordpiece_tokenizer_normalizes_and_bounds_tokens() {
        let vocab = vec![
            "[PAD]".into(),
            "[UNK]".into(),
            "[CLS]".into(),
            "[SEP]".into(),
            "cafe".into(),
            "play".into(),
            "##ing".into(),
            ",".into(),
        ];
        let tok = WordPieceTokenizer::from_tokens(vocab, Some(2), Some(3), Some(1), Some(0), 8);
        let ids = tok.encode("Café, playing!");
        assert_eq!(ids.first(), Some(&2));
        assert_eq!(ids.last(), Some(&3));
        assert!(ids.contains(&4), "expected NFD-stripped lowercase 'cafe': {ids:?}");
        assert!(ids.contains(&5) && ids.contains(&6), "expected 'play'+'##ing': {ids:?}");
        assert!(ids.contains(&7), "expected punctuation split ',': {ids:?}");
        assert!(ids.contains(&1), "expected '[UNK]' for '!': {ids:?}");

        // GGUF `▁` word-start convention with bare continuation subwords.
        let gguf_vocab = vec![
            "[PAD]".into(),
            "[UNK]".into(),
            "[CLS]".into(),
            "[SEP]".into(),
            "\u{2581}play".into(),
            "ing".into(),
            "\u{2581},".into(),
        ];
        let gtok = WordPieceTokenizer::from_tokens(gguf_vocab, Some(2), Some(3), Some(1), Some(0), 8);
        let gids = gtok.encode("playing,");
        assert_eq!(gids, vec![2, 4, 5, 6, 3]);
    }

    #[test]
    fn lexical_hash_embedder_matches_related_subtokens_and_unit_norm() {
        let emb = LexicalHashEmbedder;
        let q = emb.embed_query("invoice March payment").unwrap();
        let rel = emb.embed_documents(&[
            "MarchInvoicePayment_2026 summary",
            "Rust compiler borrow checker notes",
        ]).unwrap();
        assert_eq!(q.len(), LEXICAL_DIM);
        let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");
        assert!(
            cosine_similarity(&q, &rel[0]) > cosine_similarity(&q, &rel[1]) + 0.15,
            "camelCase/subtoken match should dominate unrelated document"
        );
    }

    #[test]
    fn on_demand_embedder_lifecycle_and_unload() {
        let empty = TempDir::new().unwrap();
        let mgr = OnDemandEmbedder::new(empty.path().to_path_buf());
        mgr.set_selection(ModelSelection::FallbackOnly);
        let desc = mgr.active_descriptor();
        assert_eq!(desc.id, LEXICAL_MODEL_ID);
        assert!(!mgr.is_loaded());

        let (d1, vecs) = mgr
            .embed_documents_and_unload(&["project planning document"])
            .unwrap();
        assert_eq!(d1.dim, LEXICAL_DIM);
        assert_eq!(vecs.len(), 1);
        assert!(!mgr.is_loaded());

        let (_d2, qv) = mgr.embed_query("planning").unwrap();
        assert_eq!(qv.len(), LEXICAL_DIM);
        mgr.unload();
        assert!(!mgr.is_loaded());
    }

    #[test]
    fn nomic_gguf_live_semantic_check() {
        let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
            return;
        };
        let manifest = PathBuf::from(home)
            .join(".ollama")
            .join("models")
            .join("manifests")
            .join("registry.ollama.ai")
            .join("library")
            .join("nomic-embed-text")
            .join("latest");
        if !manifest.is_file() {
            return;
        }
        let models = discover_local_models(Path::new("."));
        let Some(nomic) = models
            .into_iter()
            .find(|m| m.name.starts_with("nomic-embed-text") && Path::new(&m.path).is_file())
        else {
            return;
        };
        let mgr = OnDemandEmbedder::new(PathBuf::from("."));
        mgr.set_selection(ModelSelection::Path(PathBuf::from(&nomic.path)));
        let (desc, docs) = mgr
            .embed_documents_and_unload(&[
                "Monthly electric power utility invoice for March",
                "Strawberry banana smoothie dessert recipe",
                "Household kilowatt meter usage and municipal energy statement",
                "Chocolate brownie baking instructions with melted butter",
            ])
            .expect("nomic doc inference");
        assert_eq!(desc.dim, 768);
        assert!(!mgr.is_loaded(), "documents batch must unload model weights immediately");

        let (_qd, q) = mgr.embed_query("electricity bill").expect("nomic query inference");
        assert_eq!(q.len(), 768);
        let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "expected L2-normalized vector, got {norm}");

        let sim_inv = cosine_similarity(&q, &docs[0]);
        let sim_smoothie = cosine_similarity(&q, &docs[1]);
        assert!(
            sim_inv - sim_smoothie > 0.20,
            "expected semantic margin > 0.20 for 'electricity bill' vs invoice/smoothie, got {sim_inv:.4} vs {sim_smoothie:.4}"
        );
        let sim_util = cosine_similarity(&q, &docs[2]);
        let sim_brownie = cosine_similarity(&q, &docs[3]);
        assert!(
            sim_util - sim_brownie > 0.20,
            "expected semantic margin > 0.20 for 'electricity bill' vs utility/brownie, got {sim_util:.4} vs {sim_brownie:.4}"
        );
        mgr.unload();
        assert!(!mgr.is_loaded());
    }
}
