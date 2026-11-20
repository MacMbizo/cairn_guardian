//! Hardware profile detection (`ADR-002`) and Sprint-1 embedding/vector interfaces.

use aegis_core::contracts::HardwareProfile;
use serde::{Deserialize, Serialize};
use sysinfo::System;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("embedding model not loaded: {0}")]
    ModelNotLoaded(String),
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

/// Trait implemented in Sprint 1 by `ort` / `fastembed` in-process ONNX engine (`ADR-001`).
pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError>;
}

/// Trait implemented in Sprint 1 by embedded `qdrant-edge` HNSW shard manager (`B4` §4).
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

#[cfg(test)]
mod tests {
    use super::detect_hardware_capabilities;

    #[test]
    fn detects_host_profile() {
        let caps = detect_hardware_capabilities();
        assert!(caps.logical_cores >= 1);
        assert!(caps.total_ram_bytes > 0);
        assert!(caps.recommended_embed_dim == 384 || caps.recommended_embed_dim == 1024);
    }
}
