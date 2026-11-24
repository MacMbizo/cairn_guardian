//! Pure-Rust GGUF text embedding runner supporting `nomic-bert` and standard `bert` encoders
//! over compact quantized `QMatMul` weights (`K5`, `R4`, `R9`).

use crate::discovery::{gguf_f32, gguf_str, gguf_u32, gguf_usize};
use crate::fallback::l2_normalize;
use crate::tokenizer::WordPieceTokenizer;
use crate::{EmbedError, EmbeddingProvider};
use candle_core::quantized::gguf_file::{self, Value as GgufValue};
use candle_core::quantized::QMatMul;
use candle_core::{D, Device, Module, Result as CResult, Tensor};
use candle_nn::LayerNorm;
use std::fs::File;
use std::path::Path;

const MAX_SEQ_CAP: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    NomicBert,
    Bert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolMode {
    Mean,
    Cls,
}

struct NomicLayer {
    attn_qkv: QMatMul,
    attn_qkv_bias: Option<Tensor>,
    attn_output: QMatMul,
    attn_output_bias: Option<Tensor>,
    attn_norm: LayerNorm,
    ffn_gate: QMatMul,
    ffn_up: QMatMul,
    ffn_down: QMatMul,
    ffn_norm: LayerNorm,
}

struct BertLayer {
    attn_q: QMatMul,
    attn_q_bias: Option<Tensor>,
    attn_k: QMatMul,
    attn_k_bias: Option<Tensor>,
    attn_v: QMatMul,
    attn_v_bias: Option<Tensor>,
    attn_output: QMatMul,
    attn_output_bias: Option<Tensor>,
    attn_norm: LayerNorm,
    ffn_up: QMatMul,
    ffn_up_bias: Option<Tensor>,
    ffn_down: QMatMul,
    ffn_down_bias: Option<Tensor>,
    ffn_norm: LayerNorm,
}

enum Layers {
    Nomic(Vec<NomicLayer>),
    Bert(Vec<BertLayer>),
}

/// Loaded quantized/FP16 GGUF embedding encoder held in memory only while needed (`R4`).
pub struct GgufEmbedder {
    model_id: String,
    display_name: String,
    arch: Arch,
    pool: PoolMode,
    post_layer_norm: bool,
    query_prefix: &'static str,
    doc_prefix: &'static str,
    hidden_size: usize,
    n_heads: usize,
    head_dim: usize,
    rope_cos: Option<Tensor>,
    rope_sin: Option<Tensor>,
    token_embd: Tensor,
    position_embd: Option<Tensor>,
    token_type_0: Option<Tensor>,
    embd_norm: LayerNorm,
    layers: Layers,
    tokenizer: WordPieceTokenizer,
}

impl GgufEmbedder {
    pub fn load(path: &Path, override_name: Option<&str>) -> Result<Self, EmbedError> {
        Self::load_inner(path, override_name).map_err(Into::into)
    }

    fn load_inner(path: &Path, override_name: Option<&str>) -> anyhow_like::WrapResult<Self> {
        let mut file = File::open(path)?;
        let content = gguf_file::Content::read(&mut file)?;

        let arch_str = gguf_str(&content, "general.architecture")
            .ok_or_else(|| EmbedError::ModelLoad("GGUF missing general.architecture".into()))?;
        let arch = match arch_str.as_str() {
            "nomic-bert" => Arch::NomicBert,
            "bert" => Arch::Bert,
            other => {
                return Err(EmbedError::ModelLoad(format!(
                    "unsupported GGUF embedding architecture: {other}"
                ))
                .into());
            }
        };

        let gname = gguf_str(&content, "general.name").unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "gguf-embed".into())
        });
        let display_name = override_name.map(str::to_string).unwrap_or_else(|| gname.clone());
        let slug: String = gname
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let model_id = format!("gguf:{slug}");

        let prefix = arch_str.as_str();
        let n_layers = gguf_usize(&content, &format!("{prefix}.block_count"))
            .ok_or_else(|| EmbedError::ModelLoad("GGUF missing block_count".into()))?;
        let hidden_size = gguf_usize(&content, &format!("{prefix}.embedding_length"))
            .ok_or_else(|| EmbedError::ModelLoad("GGUF missing embedding_length".into()))?;
        let n_heads = gguf_usize(&content, &format!("{prefix}.attention.head_count"))
            .ok_or_else(|| EmbedError::ModelLoad("GGUF missing attention.head_count".into()))?;
        let ctx_len = gguf_usize(&content, &format!("{prefix}.context_length")).unwrap_or(512);
        let max_len = ctx_len.clamp(8, MAX_SEQ_CAP);
        let raw_eps = gguf_f32(&content, &format!("{prefix}.attention.layer_norm_epsilon")).unwrap_or(1e-12);
        let eps = if raw_eps > 0.0 { f64::from(raw_eps) } else { 1e-12 };
        let pooling_type = gguf_u32(&content, &format!("{prefix}.pooling_type")).unwrap_or(1);
        let pool = if pooling_type == 2 {
            PoolMode::Cls
        } else {
            PoolMode::Mean
        };

        let tokens = extract_vocab(&content)?;
        let tokenizer = WordPieceTokenizer::from_tokens(
            tokens,
            gguf_u32(&content, "tokenizer.ggml.cls_token_id")
                .or_else(|| gguf_u32(&content, "tokenizer.ggml.bos_token_id")),
            gguf_u32(&content, "tokenizer.ggml.seperator_token_id")
                .or_else(|| gguf_u32(&content, "tokenizer.ggml.separator_token_id"))
                .or_else(|| gguf_u32(&content, "tokenizer.ggml.eos_token_id")),
            gguf_u32(&content, "tokenizer.ggml.unknown_token_id"),
            gguf_u32(&content, "tokenizer.ggml.padding_token_id"),
            max_len,
        );

        let dev = Device::Cpu;
        let token_embd = content.tensor(&mut file, "token_embd.weight", &dev)?.dequantize(&dev)?;
        let position_embd = if content.tensor_infos.contains_key("position_embd.weight") {
            Some(
                content
                    .tensor(&mut file, "position_embd.weight", &dev)?
                    .dequantize(&dev)?,
            )
        } else {
            None
        };
        let token_type_0 = if content.tensor_infos.contains_key("token_types.weight") {
            let tt = content
                .tensor(&mut file, "token_types.weight", &dev)?
                .dequantize(&dev)?;
            Some(tt.narrow(0, 0, 1)?.unsqueeze(0)?)
        } else {
            None
        };
        let embd_norm = load_layer_norm(
            &content,
            &mut file,
            "token_embd_norm.weight",
            "token_embd_norm.bias",
            eps,
            &dev,
        )?;

        let head_dim = hidden_size / n_heads;
        let (rope_cos, rope_sin) = if arch == Arch::NomicBert {
            let freq_base = f64::from(
                gguf_f32(&content, "nomic-bert.rope.freq_base").unwrap_or(1000.0),
            );
            let (cos, sin) = build_rope_cache(head_dim, max_len, freq_base, &dev)?;
            (Some(cos), Some(sin))
        } else {
            (None, None)
        };

        let layers = match arch {
            Arch::NomicBert => {
                let mut vec = Vec::with_capacity(n_layers);
                for i in 0..n_layers {
                    vec.push(NomicLayer {
                        attn_qkv: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_qkv.weight"), &dev)?,
                        attn_qkv_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_qkv.bias"), &dev)?,
                        attn_output: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_output.weight"), &dev)?,
                        attn_output_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_output.bias"), &dev)?,
                        attn_norm: load_layer_norm(
                            &content,
                            &mut file,
                            &format!("blk.{i}.attn_output_norm.weight"),
                            &format!("blk.{i}.attn_output_norm.bias"),
                            eps,
                            &dev,
                        )?,
                        ffn_gate: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_gate.weight"), &dev)?,
                        ffn_up: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_up.weight"), &dev)?,
                        ffn_down: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_down.weight"), &dev)?,
                        ffn_norm: load_layer_norm(
                            &content,
                            &mut file,
                            &format!("blk.{i}.layer_output_norm.weight"),
                            &format!("blk.{i}.layer_output_norm.bias"),
                            eps,
                            &dev,
                        )?,
                    });
                }
                Layers::Nomic(vec)
            }
            Arch::Bert => {
                let mut vec = Vec::with_capacity(n_layers);
                for i in 0..n_layers {
                    vec.push(BertLayer {
                        attn_q: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_q.weight"), &dev)?,
                        attn_q_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_q.bias"), &dev)?,
                        attn_k: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_k.weight"), &dev)?,
                        attn_k_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_k.bias"), &dev)?,
                        attn_v: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_v.weight"), &dev)?,
                        attn_v_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_v.bias"), &dev)?,
                        attn_output: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_output.weight"), &dev)?,
                        attn_output_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_output.bias"), &dev)?,
                        attn_norm: load_layer_norm(
                            &content,
                            &mut file,
                            &format!("blk.{i}.attn_output_norm.weight"),
                            &format!("blk.{i}.attn_output_norm.bias"),
                            eps,
                            &dev,
                        )?,
                        ffn_up: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_up.weight"), &dev)?,
                        ffn_up_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.ffn_up.bias"), &dev)?,
                        ffn_down: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_down.weight"), &dev)?,
                        ffn_down_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.ffn_down.bias"), &dev)?,
                        ffn_norm: load_layer_norm(
                            &content,
                            &mut file,
                            &format!("blk.{i}.layer_output_norm.weight"),
                            &format!("blk.{i}.layer_output_norm.bias"),
                            eps,
                            &dev,
                        )?,
                    });
                }
                Layers::Bert(vec)
            }
        };

        let lower_name = gname.to_ascii_lowercase();
        let (query_prefix, doc_prefix, post_layer_norm) = match arch {
            Arch::NomicBert => (
                "search_query: ",
                "search_document: ",
                lower_name.contains("v1.5"),
            ),
            Arch::Bert => {
                let qp = if lower_name.contains("mxbai") || lower_name.contains("bge") {
                    "Represent this sentence for searching relevant passages: "
                } else {
                    ""
                };
                (qp, "", false)
            }
        };

        Ok(Self {
            model_id,
            display_name,
            arch,
            pool,
            post_layer_norm,
            query_prefix,
            doc_prefix,
            hidden_size,
            n_heads,
            head_dim,
            rope_cos,
            rope_sin,
            token_embd,
            position_embd,
            token_type_0,
            embd_norm,
            layers,
            tokenizer,
        })
    }

    fn forward_text(&self, prefix: &str, text: &str) -> CResult<Vec<f32>> {
        let prefixed = if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}")
        };
        let ids = self.tokenizer.encode(&prefixed);
        let seq = ids.len();
        let dev = Device::Cpu;
        let ids_t = Tensor::from_vec(ids, (seq,), &dev)?;
        let mut x = self.token_embd.index_select(&ids_t, 0)?.unsqueeze(0)?;
        if let Some(pos) = &self.position_embd {
            let max_pos = pos.dim(0)?;
            let slice_len = seq.min(max_pos);
            let p = pos.narrow(0, 0, slice_len)?.unsqueeze(0)?;
            x = x.narrow(1, 0, slice_len)?.broadcast_add(&p)?;
        }
        if let Some(tt) = &self.token_type_0 {
            x = x.broadcast_add(tt)?;
        }
        x = self.embd_norm.forward(&x)?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        match &self.layers {
            Layers::Nomic(layers) => {
                let cos = self
                    .rope_cos
                    .as_ref()
                    .expect("nomic rope cos")
                    .narrow(2, 0, seq)?;
                let sin = self
                    .rope_sin
                    .as_ref()
                    .expect("nomic rope sin")
                    .narrow(2, 0, seq)?;
                for layer in layers {
                    let mut qkv = layer.attn_qkv.forward(&x)?;
                    if let Some(b) = &layer.attn_qkv_bias {
                        qkv = qkv.broadcast_add(b)?;
                    }
                    let qkv_parts = qkv.chunk(3, D::Minus1)?;
                    let q = qkv_parts[0]
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let k = qkv_parts[1]
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let v = qkv_parts[2]
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let q = apply_neox_rope(&q, &cos, &sin)?;
                    let k = apply_neox_rope(&k, &cos, &sin)?;
                    let attn = bidirectional_attention(&q, &k, &v, scale, seq, self.hidden_size)?;
                    let mut proj = layer.attn_output.forward(&attn)?;
                    if let Some(b) = &layer.attn_output_bias {
                        proj = proj.broadcast_add(b)?;
                    }
                    x = layer.attn_norm.forward(&(x + proj)?)?;

                    let gate = candle_nn::ops::silu(&layer.ffn_gate.forward(&x)?)?;
                    let up = layer.ffn_up.forward(&x)?;
                    let ffn = layer.ffn_down.forward(&(gate * up)?)?;
                    x = layer.ffn_norm.forward(&(x + ffn)?)?;
                }
            }
            Layers::Bert(layers) => {
                for layer in layers {
                    let mut q = layer.attn_q.forward(&x)?;
                    if let Some(b) = &layer.attn_q_bias {
                        q = q.broadcast_add(b)?;
                    }
                    let mut k = layer.attn_k.forward(&x)?;
                    if let Some(b) = &layer.attn_k_bias {
                        k = k.broadcast_add(b)?;
                    }
                    let mut v = layer.attn_v.forward(&x)?;
                    if let Some(b) = &layer.attn_v_bias {
                        v = v.broadcast_add(b)?;
                    }
                    let q = q
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let k = k
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let v = v
                        .reshape((1, seq, self.n_heads, self.head_dim))?
                        .transpose(1, 2)?
                        .contiguous()?;
                    let attn = bidirectional_attention(&q, &k, &v, scale, seq, self.hidden_size)?;
                    let mut proj = layer.attn_output.forward(&attn)?;
                    if let Some(b) = &layer.attn_output_bias {
                        proj = proj.broadcast_add(b)?;
                    }
                    x = layer.attn_norm.forward(&(x + proj)?)?;

                    let mut up = layer.ffn_up.forward(&x)?;
                    if let Some(b) = &layer.ffn_up_bias {
                        up = up.broadcast_add(b)?;
                    }
                    let act = up.gelu()?;
                    let mut down = layer.ffn_down.forward(&act)?;
                    if let Some(b) = &layer.ffn_down_bias {
                        down = down.broadcast_add(b)?;
                    }
                    x = layer.ffn_norm.forward(&(x + down)?)?;
                }
            }
        }

        let pooled = match self.pool {
            PoolMode::Cls => x.narrow(1, 0, 1)?.squeeze(1)?,
            PoolMode::Mean => x.mean(1)?,
        };
        let mut out = pooled.squeeze(0)?.to_vec1::<f32>()?;
        if self.post_layer_norm {
            unweighted_layer_norm(&mut out, 1e-12);
        }
        l2_normalize(&mut out);
        Ok(out)
    }
}

impl EmbeddingProvider for GgufEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn backend_kind(&self) -> &str {
        match self.arch {
            Arch::NomicBert => "candle_gguf_nomic_bert",
            Arch::Bert => "candle_gguf_bert",
        }
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
                self.forward_text(self.doc_prefix, t)
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

fn extract_vocab(content: &gguf_file::Content) -> Result<Vec<String>, EmbedError> {
    let Some(GgufValue::Array(arr)) = content.metadata.get("tokenizer.ggml.tokens") else {
        return Err(EmbedError::ModelLoad(
            "GGUF missing tokenizer.ggml.tokens array".into(),
        ));
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let GgufValue::String(s) = item else {
            return Err(EmbedError::ModelLoad(
                "tokenizer.ggml.tokens contains non-string entry".into(),
            ));
        };
        out.push(s.clone());
    }
    Ok(out)
}

fn load_qmatmul(
    content: &gguf_file::Content,
    file: &mut File,
    name: &str,
    dev: &Device,
) -> CResult<QMatMul> {
    let qt = content.tensor(file, name, dev)?;
    QMatMul::from_qtensor(qt)
}

fn try_tensor_f32(
    content: &gguf_file::Content,
    file: &mut File,
    name: &str,
    dev: &Device,
) -> CResult<Option<Tensor>> {
    if !content.tensor_infos.contains_key(name) {
        return Ok(None);
    }
    Ok(Some(content.tensor(file, name, dev)?.dequantize(dev)?))
}

fn load_layer_norm(
    content: &gguf_file::Content,
    file: &mut File,
    w_name: &str,
    b_name: &str,
    eps: f64,
    dev: &Device,
) -> CResult<LayerNorm> {
    let w = content.tensor(file, w_name, dev)?.dequantize(dev)?;
    let b = content.tensor(file, b_name, dev)?.dequantize(dev)?;
    Ok(LayerNorm::new(w, b, eps))
}

fn build_rope_cache(
    head_dim: usize,
    max_seq: usize,
    freq_base: f64,
    dev: &Device,
) -> CResult<(Tensor, Tensor)> {
    let half = head_dim / 2;
    let mut cos_vals = Vec::with_capacity(max_seq * half);
    let mut sin_vals = Vec::with_capacity(max_seq * half);
    for pos in 0..max_seq {
        for j in 0..half {
            let theta = 1.0 / freq_base.powf((2 * j) as f64 / head_dim as f64);
            let angle = (pos as f64) * theta;
            cos_vals.push(angle.cos() as f32);
            sin_vals.push(angle.sin() as f32);
        }
    }
    let cos = Tensor::from_vec(cos_vals, (1, 1, max_seq, half), dev)?;
    let sin = Tensor::from_vec(sin_vals, (1, 1, max_seq, half), dev)?;
    Ok((cos, sin))
}

/// NeoX split-half RoPE (`[first_half, second_half]`), matching `LLAMA_ROPE_TYPE_NEOX`.
fn apply_neox_rope(x: &Tensor, cos: &Tensor, sin: &Tensor) -> CResult<Tensor> {
    let parts = x.chunk(2, D::Minus1)?;
    let x1 = &parts[0];
    let x2 = &parts[1];
    let r1 = (x1.broadcast_mul(cos)? - x2.broadcast_mul(sin)?)?;
    let r2 = (x2.broadcast_mul(cos)? + x1.broadcast_mul(sin)?)?;
    Tensor::cat(&[&r1, &r2], D::Minus1)
}

pub(crate) fn bidirectional_attention(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    scale: f64,
    seq: usize,
    hidden_size: usize,
) -> CResult<Tensor> {
    let scores = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * scale)?;
    let probs = candle_nn::ops::softmax_last_dim(&scores)?;
    let ctx = probs.matmul(v)?;
    ctx.transpose(1, 2)?.contiguous()?.reshape((1, seq, hidden_size))
}

fn unweighted_layer_norm(v: &mut [f32], eps: f64) {
    if v.is_empty() {
        return;
    }
    let n = v.len() as f64;
    let mean = v.iter().map(|x| f64::from(*x)).sum::<f64>() / n;
    let var = v
        .iter()
        .map(|x| {
            let d = f64::from(*x) - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    let inv_std = 1.0 / (var + eps).sqrt();
    for x in v.iter_mut() {
        *x = ((f64::from(*x) - mean) * inv_std) as f32;
    }
}

mod anyhow_like {
    use crate::EmbedError;
    pub type WrapResult<T> = Result<T, WrapErr>;
    pub struct WrapErr(String);
    impl From<EmbedError> for WrapErr {
        fn from(e: EmbedError) -> Self {
            Self(e.to_string())
        }
    }
    impl From<std::io::Error> for WrapErr {
        fn from(e: std::io::Error) -> Self {
            Self(e.to_string())
        }
    }
    impl From<candle_core::Error> for WrapErr {
        fn from(e: candle_core::Error) -> Self {
            Self(e.to_string())
        }
    }
    impl From<WrapErr> for EmbedError {
        fn from(e: WrapErr) -> Self {
            EmbedError::ModelLoad(e.0)
        }
    }
}
