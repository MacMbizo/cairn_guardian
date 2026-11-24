//! Pure-Rust loader for local HuggingFace Safetensors BERT directories (`config.json`, `vocab.txt`,
//! `model.safetensors`), memory-mapped via `candle_nn::VarBuilder::from_mmaped_safetensors`.

use crate::fallback::l2_normalize;
use crate::gguf::bidirectional_attention;
use crate::tokenizer::WordPieceTokenizer;
use crate::{EmbedError, EmbeddingProvider};
use candle_core::{DType, Device, Module, Result as CResult, Tensor};
use candle_nn::{LayerNorm, Linear, VarBuilder, layer_norm, linear};
use serde::Deserialize;
use std::path::Path;

const MAX_SEQ_CAP: usize = 512;

#[derive(Deserialize)]
struct BertJsonConfig {
    hidden_size: usize,
    num_hidden_layers: usize,
    num_attention_heads: usize,
    intermediate_size: usize,
    #[serde(default = "default_eps")]
    layer_norm_eps: f64,
    #[serde(default = "default_max_pos")]
    max_position_embeddings: usize,
    #[serde(default)]
    vocab_size: usize,
    #[serde(default = "default_type_vocab")]
    type_vocab_size: usize,
}

fn default_eps() -> f64 {
    1e-12
}
fn default_max_pos() -> usize {
    512
}
fn default_type_vocab() -> usize {
    2
}

struct SafetensorsBertLayer {
    q: Linear,
    k: Linear,
    v: Linear,
    out_proj: Linear,
    attn_norm: LayerNorm,
    ffn_up: Linear,
    ffn_down: Linear,
    ffn_norm: LayerNorm,
}

/// Memory-mapped local BERT embedding encoder (`model.safetensors`).
pub struct SafetensorsBertEmbedder {
    model_id: String,
    display_name: String,
    use_cls_pool: bool,
    query_prefix: &'static str,
    hidden_size: usize,
    n_heads: usize,
    head_dim: usize,
    word_embeddings: Tensor,
    position_embeddings: Tensor,
    token_type_0: Option<Tensor>,
    emb_norm: LayerNorm,
    layers: Vec<SafetensorsBertLayer>,
    tokenizer: WordPieceTokenizer,
}

impl SafetensorsBertEmbedder {
    pub fn load(dir: &Path) -> Result<Self, EmbedError> {
        Self::load_inner(dir).map_err(|e| EmbedError::ModelLoad(e.to_string()))
    }

    fn load_inner(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let cfg_raw = std::fs::read_to_string(dir.join("config.json"))?;
        let cfg: BertJsonConfig = serde_json::from_str(&cfg_raw)?;
        let max_len = cfg.max_position_embeddings.clamp(8, MAX_SEQ_CAP);
        let tokenizer = WordPieceTokenizer::from_vocab_file(&dir.join("vocab.txt"), max_len)?;
        let vocab_size = if cfg.vocab_size > 0 {
            cfg.vocab_size
        } else {
            tokenizer.vocab_size()
        };

        let weights = [dir.join("model.safetensors")];
        // SAFETY: `model.safetensors` is an immutable local checkpoint directory owned on disk.
        let root = unsafe { VarBuilder::from_mmaped_safetensors(&weights, DType::F32, &Device::Cpu)? };
        let vb = if root.contains_tensor("bert.embeddings.word_embeddings.weight") {
            root.pp("bert")
        } else {
            root
        };

        let emb = vb.pp("embeddings");
        let word_embeddings = emb.get((vocab_size, cfg.hidden_size), "word_embeddings.weight")?;
        let position_embeddings = emb.get(
            (cfg.max_position_embeddings, cfg.hidden_size),
            "position_embeddings.weight",
        )?;
        let token_type_0 = if cfg.type_vocab_size > 0
            && emb.contains_tensor("token_type_embeddings.weight")
        {
            let tt = emb.get((cfg.type_vocab_size, cfg.hidden_size), "token_type_embeddings.weight")?;
            Some(tt.narrow(0, 0, 1)?.unsqueeze(0)?)
        } else {
            None
        };
        let emb_norm = layer_norm(cfg.hidden_size, cfg.layer_norm_eps, emb.pp("LayerNorm"))?;

        let enc = vb.pp("encoder.layer");
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let l = enc.pp(i.to_string());
            let sa = l.pp("attention.self");
            let ao = l.pp("attention.output");
            let im = l.pp("intermediate");
            let ot = l.pp("output");
            layers.push(SafetensorsBertLayer {
                q: linear(cfg.hidden_size, cfg.hidden_size, sa.pp("query"))?,
                k: linear(cfg.hidden_size, cfg.hidden_size, sa.pp("key"))?,
                v: linear(cfg.hidden_size, cfg.hidden_size, sa.pp("value"))?,
                out_proj: linear(cfg.hidden_size, cfg.hidden_size, ao.pp("dense"))?,
                attn_norm: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, ao.pp("LayerNorm"))?,
                ffn_up: linear(cfg.hidden_size, cfg.intermediate_size, im.pp("dense"))?,
                ffn_down: linear(cfg.intermediate_size, cfg.hidden_size, ot.pp("dense"))?,
                ffn_norm: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, ot.pp("LayerNorm"))?,
            });
        }

        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "local-bert".into());
        let lower = name.to_ascii_lowercase();
        let use_cls_pool = lower.contains("bge") || lower.contains("mxbai");
        let query_prefix = if lower.contains("bge") {
            "Represent this sentence for searching relevant passages: "
        } else {
            ""
        };
        let head_dim = cfg.hidden_size / cfg.num_attention_heads;

        Ok(Self {
            model_id: format!("safetensors:{lower}"),
            display_name: name,
            use_cls_pool,
            query_prefix,
            hidden_size: cfg.hidden_size,
            n_heads: cfg.num_attention_heads,
            head_dim,
            word_embeddings,
            position_embeddings,
            token_type_0,
            emb_norm,
            layers,
            tokenizer,
        })
    }

    fn forward_text(&self, prefix: &str, text: &str) -> CResult<Vec<f32>> {
        let full = if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}")
        };
        let ids = self.tokenizer.encode(&full);
        let seq = ids.len();
        let dev = Device::Cpu;
        let ids_t = Tensor::from_vec(ids, (seq,), &dev)?;
        let pos = self.position_embeddings.narrow(0, 0, seq)?.unsqueeze(0)?;
        let mut x = self
            .word_embeddings
            .index_select(&ids_t, 0)?
            .unsqueeze(0)?
            .broadcast_add(&pos)?;
        if let Some(tt) = &self.token_type_0 {
            x = x.broadcast_add(tt)?;
        }
        x = self.emb_norm.forward(&x)?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        for layer in &self.layers {
            let q = layer
                .q
                .forward(&x)?
                .reshape((1, seq, self.n_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;
            let k = layer
                .k
                .forward(&x)?
                .reshape((1, seq, self.n_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;
            let v = layer
                .v
                .forward(&x)?
                .reshape((1, seq, self.n_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;
            let attn = bidirectional_attention(&q, &k, &v, scale, seq, self.hidden_size)?;
            let proj = layer.out_proj.forward(&attn)?;
            x = layer.attn_norm.forward(&(x + proj)?)?;
            let act = layer.ffn_up.forward(&x)?.gelu()?;
            let down = layer.ffn_down.forward(&act)?;
            x = layer.ffn_norm.forward(&(x + down)?)?;
        }

        let pooled = if self.use_cls_pool {
            x.narrow(1, 0, 1)?.squeeze(1)?
        } else {
            x.mean(1)?
        };
        let mut out = pooled.squeeze(0)?.to_vec1::<f32>()?;
        l2_normalize(&mut out);
        Ok(out)
    }
}

impl EmbeddingProvider for SafetensorsBertEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn backend_kind(&self) -> &str {
        "candle_safetensors_bert"
    }

    fn dimensions(&self) -> usize {
        self.hidden_size
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.embed_documents(texts)
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(
                self.forward_text("", t)
                    .map_err(|e| EmbedError::Inference(e.to_string()))?,
            );
        }
        Ok(out)
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>, EmbedError> {
        self.forward_text(self.query_prefix, query)
            .map_err(|e| EmbedError::Inference(e.to_string()))
    }
}
