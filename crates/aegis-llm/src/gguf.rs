//! Pure-Rust quantized GGUF causal LLM engine (`Llama` + `Qwen2`) built on `candle-core` & `candle-nn` (`K3`, `R9`).

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use candle_core::quantized::{QMatMul, QTensor, gguf_file};
use candle_core::{DType, Device, Module, Result as CResult, Tensor};
use candle_nn::RmsNorm;

use crate::discovery::{
    K5_LLM_RECOMMEND_MAX_BYTES, LlmModelDescriptor, gguf_f32, gguf_str, gguf_usize,
};
use crate::tokenizer::LlmTokenizer;
use crate::{GenerateOutput, GenerateRequest, LlmError, TextGenerator};

const MAX_RUNTIME_CTX: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arch {
    Llama,
    Qwen2,
}

struct TransformerBlock {
    attn_norm: RmsNorm,
    attn_q: QMatMul,
    attn_q_bias: Option<Tensor>,
    attn_k: QMatMul,
    attn_k_bias: Option<Tensor>,
    attn_v: QMatMul,
    attn_v_bias: Option<Tensor>,
    attn_output: QMatMul,
    attn_output_bias: Option<Tensor>,
    ffn_norm: RmsNorm,
    ffn_gate: QMatMul,
    ffn_up: QMatMul,
    ffn_down: QMatMul,
}

struct ModelWeights {
    arch: Arch,
    n_heads: usize,
    n_kv_heads: usize,
    head_dim: usize,
    max_seq_len: usize,
    token_embd: Arc<QTensor>,
    lm_head: QMatMul,
    output_norm: RmsNorm,
    cos: Tensor,
    sin: Tensor,
    layers: Vec<TransformerBlock>,
}

/// Loaded CPU quantized GGUF chat/instruct LLM with stateful KV cache isolated behind a mutex.
pub struct GgufLlm {
    descriptor: LlmModelDescriptor,
    tokenizer: LlmTokenizer,
    weights: ModelWeights,
    inference_lock: Mutex<()>,
}

impl GgufLlm {
    pub fn load(path: &Path, override_name: Option<&str>) -> Result<Self, LlmError> {
        let size_bytes = std::fs::metadata(path)?.len();
        let mut file = File::open(path)?;
        let content = gguf_file::Content::read(&mut file)
            .map_err(|e| LlmError::ModelLoad(format!("invalid GGUF header: {e}")))?;

        let arch_str = gguf_str(&content, "general.architecture")
            .ok_or_else(|| LlmError::ModelLoad("GGUF missing general.architecture".into()))?;
        let arch = match arch_str.as_str() {
            "llama" => Arch::Llama,
            "qwen2" => Arch::Qwen2,
            other => {
                return Err(LlmError::ModelLoad(format!(
                    "unsupported GGUF LLM architecture: {other}"
                )));
            }
        };

        let n_layers = gguf_usize(&content, &format!("{arch_str}.block_count"))
            .ok_or_else(|| LlmError::ModelLoad("GGUF missing block_count".into()))?;
        let hidden_size = gguf_usize(&content, &format!("{arch_str}.embedding_length"))
            .ok_or_else(|| LlmError::ModelLoad("GGUF missing embedding_length".into()))?;
        let n_heads = gguf_usize(&content, &format!("{arch_str}.attention.head_count"))
            .ok_or_else(|| LlmError::ModelLoad("GGUF missing attention.head_count".into()))?;
        let n_kv_heads = gguf_usize(&content, &format!("{arch_str}.attention.head_count_kv"))
            .unwrap_or(n_heads);
        let head_dim = gguf_usize(&content, &format!("{arch_str}.attention.key_length"))
            .unwrap_or(hidden_size / n_heads.max(1));
        if n_heads == 0 || n_kv_heads == 0 || head_dim == 0 || n_heads % n_kv_heads != 0 {
            return Err(LlmError::ModelLoad(format!(
                "invalid attention head configuration: heads={n_heads}, kv={n_kv_heads}, head_dim={head_dim}"
            )));
        }

        let raw_ctx = gguf_usize(&content, &format!("{arch_str}.context_length")).unwrap_or(2048);
        let max_seq_len = raw_ctx.clamp(16, MAX_RUNTIME_CTX);
        let rms_eps = f64::from(
            gguf_f32(&content, &format!("{arch_str}.attention.layer_norm_rms_epsilon"))
                .filter(|v| *v > 0.0)
                .unwrap_or(1e-5),
        );
        let freq_base = f64::from(
            gguf_f32(&content, &format!("{arch_str}.rope.freq_base"))
                .filter(|v| *v > 0.0)
                .unwrap_or(10_000.0),
        );

        let tokenizer = LlmTokenizer::from_gguf(&content, &arch_str)?;
        let dev = Device::Cpu;

        let rope_freqs = if content.tensor_infos.contains_key("rope_freqs.weight") {
            Some(
                content
                    .tensor(&mut file, "rope_freqs.weight", &dev)?
                    .dequantize(&dev)?
                    .to_vec1::<f32>()?,
            )
        } else {
            None
        };
        let (cos, sin) =
            build_rope_tables(head_dim, max_seq_len, freq_base, rope_freqs.as_deref(), &dev)?;

        let tok_embd_qt = Arc::new(content.tensor(&mut file, "token_embd.weight", &dev)?);
        let lm_head = if content.tensor_infos.contains_key("output.weight") {
            load_qmatmul(&content, &mut file, "output.weight", &dev)?
        } else {
            QMatMul::from_arc(tok_embd_qt.clone())?
        };
        let output_norm = load_rms_norm(&content, &mut file, "output_norm.weight", rms_eps, &dev)?;

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            layers.push(TransformerBlock {
                attn_norm: load_rms_norm(
                    &content,
                    &mut file,
                    &format!("blk.{i}.attn_norm.weight"),
                    rms_eps,
                    &dev,
                )?,
                attn_q: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_q.weight"), &dev)?,
                attn_q_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_q.bias"), &dev)?,
                attn_k: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_k.weight"), &dev)?,
                attn_k_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_k.bias"), &dev)?,
                attn_v: load_qmatmul(&content, &mut file, &format!("blk.{i}.attn_v.weight"), &dev)?,
                attn_v_bias: try_tensor_f32(&content, &mut file, &format!("blk.{i}.attn_v.bias"), &dev)?,
                attn_output: load_qmatmul(
                    &content,
                    &mut file,
                    &format!("blk.{i}.attn_output.weight"),
                    &dev,
                )?,
                attn_output_bias: try_tensor_f32(
                    &content,
                    &mut file,
                    &format!("blk.{i}.attn_output.bias"),
                    &dev,
                )?,
                ffn_norm: load_rms_norm(
                    &content,
                    &mut file,
                    &format!("blk.{i}.ffn_norm.weight"),
                    rms_eps,
                    &dev,
                )?,
                ffn_gate: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_gate.weight"), &dev)?,
                ffn_up: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_up.weight"), &dev)?,
                ffn_down: load_qmatmul(&content, &mut file, &format!("blk.{i}.ffn_down.weight"), &dev)?,
            });
        }

        let gname = gguf_str(&content, "general.name").unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "local-llm".into())
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

        let descriptor = LlmModelDescriptor {
            id: format!("gguf:{slug}"),
            name: display,
            path: path.to_string_lossy().to_string(),
            backend: format!("candle_gguf_{arch_str}"),
            architecture: arch_str,
            context_length: raw_ctx,
            size_bytes,
            source: "loaded".to_string(),
            recommended: size_bytes <= K5_LLM_RECOMMEND_MAX_BYTES,
        };

        Ok(Self {
            descriptor,
            tokenizer,
            weights: ModelWeights {
                arch,
                n_heads,
                n_kv_heads,
                head_dim,
                max_seq_len,
                token_embd: tok_embd_qt,
                lm_head,
                output_norm,
                cos,
                sin,
                layers,
            },
            inference_lock: Mutex::new(()),
        })
    }

    /// Runs raw token-id generation without wrapping in a chat template (useful for unit tests).
    pub fn generate_from_tokens(
        &self,
        prompt_ids: &[u32],
        req: &GenerateRequest,
        cancel: &AtomicBool,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateOutput, LlmError> {
        let _guard = self.inference_lock.lock().unwrap_or_else(|p| p.into_inner());
        let started = Instant::now();

        if prompt_ids.is_empty() {
            return Err(LlmError::Inference("prompt tokenized to zero tokens".into()));
        }

        // Reserve room for generation inside max_seq_len.
        let max_gen = req.max_tokens.max(1).min(self.weights.max_seq_len.saturating_sub(4));
        let max_prompt = self.weights.max_seq_len.saturating_sub(max_gen).max(1);
        let prompt_slice = if prompt_ids.len() > max_prompt {
            &prompt_ids[prompt_ids.len() - max_prompt..]
        } else {
            prompt_ids
        };

        let mut kv_cache: Vec<Option<(Tensor, Tensor)>> = vec![None; self.weights.layers.len()];
        let mut logits = self.weights.forward(prompt_slice, 0, &mut kv_cache)?;

        let mut history: Vec<u32> = prompt_slice.to_vec();
        let mut generated_text = String::new();
        let mut pending_bytes = Vec::new();
        let mut tokens_generated = 0usize;
        let mut rng_state = if req.seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { req.seed };
        let mut cancelled = false;

        for pos in (prompt_slice.len()..).take(max_gen) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            if pos >= self.weights.max_seq_len {
                break;
            }

            let logits_vec = logits.to_vec1::<f32>()?;
            let next_id = sample_next_token(
                &logits_vec,
                &history,
                req.temperature,
                req.top_p,
                req.repetition_penalty,
                &mut rng_state,
            );
            if self.tokenizer.is_eos(next_id) {
                break;
            }

            tokens_generated += 1;
            history.push(next_id);

            let piece = self.tokenizer.decode_token_stream(next_id, &mut pending_bytes);
            if !piece.is_empty() {
                let candidate_total = format!("{generated_text}{piece}");
                if let Some(stop_idx) = find_stop_sequence(&candidate_total, &req.stop_sequences) {
                    let tail = &candidate_total[generated_text.len()..stop_idx];
                    if !tail.is_empty() {
                        on_token(tail);
                        generated_text.push_str(tail);
                    }
                    break;
                }
                on_token(&piece);
                generated_text.push_str(&piece);
            }

            logits = self.weights.forward(&[next_id], pos, &mut kv_cache)?;
        }

        if !pending_bytes.is_empty() && !cancelled {
            let tail = String::from_utf8_lossy(&pending_bytes).to_string();
            if !tail.is_empty() {
                on_token(&tail);
                generated_text.push_str(&tail);
            }
        }

        Ok(GenerateOutput {
            text: generated_text.trim().to_string(),
            tokens_generated,
            prompt_tokens: prompt_slice.len(),
            duration_ms: started.elapsed().as_millis() as u64,
            cancelled,
            model_id: self.descriptor.id.clone(),
            backend: self.descriptor.backend.clone(),
        })
    }
}

impl TextGenerator for GgufLlm {
    fn descriptor(&self) -> &LlmModelDescriptor {
        &self.descriptor
    }

    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &AtomicBool,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateOutput, LlmError> {
        let formatted = self
            .tokenizer
            .format_chat_prompt(&req.system_prompt, &req.user_prompt);
        let prompt_ids = self.tokenizer.encode(&formatted, true);
        self.generate_from_tokens(&prompt_ids, req, cancel, on_token)
    }
}

impl ModelWeights {
    fn forward(
        &self,
        tokens: &[u32],
        start_pos: usize,
        kv_cache: &mut [Option<(Tensor, Tensor)>],
    ) -> CResult<Tensor> {
        let dev = Device::Cpu;
        let seq = tokens.len();
        let ids_t = Tensor::from_vec(tokens.to_vec(), (1, seq), &dev)?;
        let mut x = self.token_embd.embedding(&ids_t)?;
        let cos = self.cos.narrow(0, start_pos, seq)?.contiguous()?;
        let sin = self.sin.narrow(0, start_pos, seq)?.contiguous()?;
        let n_rep = self.n_heads / self.n_kv_heads;
        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let total_seq = start_pos + seq;
        let causal_mask = if seq > 1 {
            Some(build_causal_mask(seq, start_pos, total_seq, &dev)?)
        } else {
            None
        };

        for (layer_idx, layer) in self.layers.iter().enumerate() {
            let norm_x = layer.attn_norm.forward(&x)?;
            let mut q = layer.attn_q.forward(&norm_x)?;
            if let Some(b) = &layer.attn_q_bias {
                q = q.broadcast_add(b)?;
            }
            let mut k = layer.attn_k.forward(&norm_x)?;
            if let Some(b) = &layer.attn_k_bias {
                k = k.broadcast_add(b)?;
            }
            let mut v = layer.attn_v.forward(&norm_x)?;
            if let Some(b) = &layer.attn_v_bias {
                v = v.broadcast_add(b)?;
            }

            let q = q
                .reshape((1, seq, self.n_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;
            let k = k
                .reshape((1, seq, self.n_kv_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;
            let v = v
                .reshape((1, seq, self.n_kv_heads, self.head_dim))?
                .transpose(1, 2)?
                .contiguous()?;

            let (q, k) = match self.arch {
                Arch::Llama => (
                    candle_nn::rotary_emb::rope_i(&q, &cos, &sin)?,
                    candle_nn::rotary_emb::rope_i(&k, &cos, &sin)?,
                ),
                Arch::Qwen2 => (
                    candle_nn::rotary_emb::rope(&q, &cos, &sin)?,
                    candle_nn::rotary_emb::rope(&k, &cos, &sin)?,
                ),
            };

            let (k_cat, v_cat) = if let Some((prev_k, prev_v)) = kv_cache[layer_idx].take() {
                (
                    Tensor::cat(&[&prev_k, &k], 2)?,
                    Tensor::cat(&[&prev_v, &v], 2)?,
                )
            } else {
                (k, v)
            };
            kv_cache[layer_idx] = Some((k_cat.clone(), v_cat.clone()));

            let k_rep = repeat_kv(k_cat, n_rep)?;
            let v_rep = repeat_kv(v_cat, n_rep)?;

            let mut scores = (q.matmul(&k_rep.transpose(2, 3)?.contiguous()?)? * scale)?;
            if let Some(mask) = &causal_mask {
                scores = scores.broadcast_add(mask)?;
            }
            let probs = candle_nn::ops::softmax_last_dim(&scores)?;
            let attn = probs
                .matmul(&v_rep.contiguous()?)?
                .transpose(1, 2)?
                .contiguous()?
                .reshape((1, seq, self.n_heads * self.head_dim))?;
            let mut attn_out = layer.attn_output.forward(&attn)?;
            if let Some(b) = &layer.attn_output_bias {
                attn_out = attn_out.broadcast_add(b)?;
            }
            x = (x + attn_out)?;

            let ffn_in = layer.ffn_norm.forward(&x)?;
            let gate = candle_nn::Activation::Silu.forward(&layer.ffn_gate.forward(&ffn_in)?)?;
            let up = layer.ffn_up.forward(&ffn_in)?;
            let ffn_out = layer.ffn_down.forward(&(gate * up)?)?;
            x = (x + ffn_out)?;
        }

        let last_hidden = x.narrow(1, seq - 1, 1)?;
        let normed = self.output_norm.forward(&last_hidden)?;
        let logits = self.lm_head.forward(&normed)?;
        logits.squeeze(0)?.squeeze(0)?.to_dtype(DType::F32)
    }
}

fn repeat_kv(x: Tensor, n_rep: usize) -> CResult<Tensor> {
    if n_rep == 1 {
        return Ok(x);
    }
    let (b, kv_heads, seq, head_dim) = x.dims4()?;
    x.unsqueeze(2)?
        .expand((b, kv_heads, n_rep, seq, head_dim))?
        .reshape((b, kv_heads * n_rep, seq, head_dim))
}

fn build_causal_mask(seq: usize, start_pos: usize, total_seq: usize, dev: &Device) -> CResult<Tensor> {
    let mut mask = vec![0.0f32; seq * total_seq];
    for i in 0..seq {
        let query_pos = start_pos + i;
        for j in (query_pos + 1)..total_seq {
            mask[i * total_seq + j] = f32::NEG_INFINITY;
        }
    }
    Tensor::from_vec(mask, (1, 1, seq, total_seq), dev)
}

fn build_rope_tables(
    head_dim: usize,
    max_seq: usize,
    freq_base: f64,
    rope_freqs: Option<&[f32]>,
    dev: &Device,
) -> CResult<(Tensor, Tensor)> {
    let half = head_dim / 2;
    let mut cos_vals = Vec::with_capacity(max_seq * half);
    let mut sin_vals = Vec::with_capacity(max_seq * half);
    let mut inv_freqs = Vec::with_capacity(half);
    for j in 0..half {
        let factor = rope_freqs
            .and_then(|rf| rf.get(j).copied())
            .filter(|f| *f > 0.0)
            .map(f64::from)
            .unwrap_or(1.0);
        let base_inv = 1.0 / (freq_base.powf((2 * j) as f64 / head_dim as f64) * factor);
        inv_freqs.push(base_inv);
    }

    for pos in 0..max_seq {
        let p = pos as f64;
        for &theta in &inv_freqs {
            let angle = p * theta;
            cos_vals.push(angle.cos() as f32);
            sin_vals.push(angle.sin() as f32);
        }
    }
    let cos = Tensor::from_vec(cos_vals, (max_seq, half), dev)?;
    let sin = Tensor::from_vec(sin_vals, (max_seq, half), dev)?;
    Ok((cos, sin))
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

fn load_rms_norm(
    content: &gguf_file::Content,
    file: &mut File,
    name: &str,
    eps: f64,
    dev: &Device,
) -> CResult<RmsNorm> {
    let w = content.tensor(file, name, dev)?.dequantize(dev)?;
    Ok(RmsNorm::new(w, eps))
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

fn sample_next_token(
    logits: &[f32],
    history: &[u32],
    temperature: f32,
    top_p: f32,
    repetition_penalty: f32,
    rng_state: &mut u64,
) -> u32 {
    if logits.is_empty() {
        return 0;
    }

    let mut adjusted = logits.to_vec();
    if (repetition_penalty - 1.0).abs() > 1e-4 {
        let window_start = history.len().saturating_sub(64);
        for &tid in &history[window_start..] {
            if let Some(slot) = adjusted.get_mut(tid as usize) {
                if *slot > 0.0 {
                    *slot /= repetition_penalty;
                } else {
                    *slot *= repetition_penalty;
                }
            }
        }
    }

    if temperature <= 1e-4 {
        let mut best_id = 0usize;
        let mut best_val = f32::NEG_INFINITY;
        for (i, &val) in adjusted.iter().enumerate() {
            if val.is_finite() && val > best_val {
                best_val = val;
                best_id = i;
            }
        }
        return best_id as u32;
    }

    let inv_t = 1.0 / temperature;
    let mut indexed: Vec<(usize, f32)> = adjusted
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .map(|(i, v)| (i, *v * inv_t))
        .collect();
    if indexed.is_empty() {
        return 0;
    }

    indexed.sort_by(|a, b| b.1.total_cmp(&a.1));
    indexed.truncate(64);
    let max_logit = indexed[0].1;
    let mut probs: Vec<(usize, f32)> = Vec::with_capacity(indexed.len());
    let mut sum = 0.0f32;
    for (idx, val) in indexed {
        let p = (val - max_logit).exp();
        sum += p;
        probs.push((idx, p));
    }
    if sum <= 0.0 {
        return probs[0].0 as u32;
    }

    let nucleus = top_p.clamp(0.05, 1.0);
    let mut cum = 0.0f32;
    let mut cutoff = probs.len();
    for (i, (_, p)) in probs.iter_mut().enumerate() {
        *p /= sum;
        cum += *p;
        if cum >= nucleus {
            cutoff = i + 1;
            break;
        }
    }
    probs.truncate(cutoff);

    let r = next_unit_f32(rng_state) * cum.max(1e-6);
    let mut running = 0.0f32;
    for &(idx, p) in &probs {
        running += p;
        if running >= r {
            return idx as u32;
        }
    }
    probs[0].0 as u32
}

fn next_unit_f32(state: &mut u64) -> f32 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    let mixed = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    ((mixed >> 40) as f32) / ((1u64 << 24) as f32)
}

fn find_stop_sequence(text: &str, stops: &[String]) -> Option<usize> {
    let mut earliest = None;
    for stop in stops {
        if !stop.is_empty()
            && let Some(pos) = text.find(stop.as_str())
        {
            earliest = Some(earliest.map_or(pos, |e: usize| e.min(pos)));
        }
    }
    earliest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    fn write_gguf_string(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    fn write_kv_str(buf: &mut Vec<u8>, key: &str, val: &str) {
        write_gguf_string(buf, key);
        buf.extend_from_slice(&8u32.to_le_bytes()); // String
        write_gguf_string(buf, val);
    }

    fn write_kv_u32(buf: &mut Vec<u8>, key: &str, val: u32) {
        write_gguf_string(buf, key);
        buf.extend_from_slice(&4u32.to_le_bytes()); // U32
        buf.extend_from_slice(&val.to_le_bytes());
    }

    fn write_kv_f32(buf: &mut Vec<u8>, key: &str, val: f32) {
        write_gguf_string(buf, key);
        buf.extend_from_slice(&6u32.to_le_bytes()); // F32
        buf.extend_from_slice(&val.to_le_bytes());
    }

    fn write_kv_str_array(buf: &mut Vec<u8>, key: &str, vals: &[String]) {
        write_gguf_string(buf, key);
        buf.extend_from_slice(&9u32.to_le_bytes()); // Array
        buf.extend_from_slice(&8u32.to_le_bytes()); // element = String
        buf.extend_from_slice(&(vals.len() as u64).to_le_bytes());
        for v in vals {
            write_gguf_string(buf, v);
        }
    }

    fn write_kv_i32_array(buf: &mut Vec<u8>, key: &str, vals: &[i32]) {
        write_gguf_string(buf, key);
        buf.extend_from_slice(&9u32.to_le_bytes());
        buf.extend_from_slice(&5u32.to_le_bytes()); // I32
        buf.extend_from_slice(&(vals.len() as u64).to_le_bytes());
        for v in vals {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn write_synthetic_gguf(path: &Path, arch: &str, vocab: usize, with_qkv_bias: bool) {
        let hidden = 32usize;
        let n_heads = 2usize;
        let n_kv_heads = 2usize;
        let ffn_hidden = 64usize;

        let mut kv = Vec::new();
        let mut kv_count = 0u64;
        write_kv_str(&mut kv, "general.architecture", arch); kv_count += 1;
        write_kv_str(&mut kv, "general.name", &format!("Synthetic-{arch}")); kv_count += 1;
        write_kv_u32(&mut kv, &format!("{arch}.block_count"), 1); kv_count += 1;
        write_kv_u32(&mut kv, &format!("{arch}.embedding_length"), hidden as u32); kv_count += 1;
        write_kv_u32(&mut kv, &format!("{arch}.attention.head_count"), n_heads as u32); kv_count += 1;
        write_kv_u32(&mut kv, &format!("{arch}.attention.head_count_kv"), n_kv_heads as u32); kv_count += 1;
        write_kv_u32(&mut kv, &format!("{arch}.context_length"), 64); kv_count += 1;
        write_kv_f32(&mut kv, &format!("{arch}.attention.layer_norm_rms_epsilon"), 1e-5); kv_count += 1;
        write_kv_f32(&mut kv, &format!("{arch}.rope.freq_base"), 10000.0); kv_count += 1;

        let mut toks: Vec<String> = (0..vocab)
            .map(|i| match i {
                0 => "<unk>".to_string(),
                1 => "<s>".to_string(),
                2 => "</s>".to_string(),
                _ => format!("t{i}"),
            })
            .collect();
        toks[3] = " ".to_string();
        toks[4] = "hello".to_string();
        toks[5] = "world".to_string();
        let types: Vec<i32> = (0..vocab).map(|i| if i <= 2 { 3 } else { 1 }).collect();
        write_kv_str(&mut kv, "tokenizer.ggml.model", "llama"); kv_count += 1;
        write_kv_str_array(&mut kv, "tokenizer.ggml.tokens", &toks); kv_count += 1;
        write_kv_i32_array(&mut kv, "tokenizer.ggml.token_type", &types); kv_count += 1;
        write_kv_u32(&mut kv, "tokenizer.ggml.bos_token_id", 1); kv_count += 1;
        write_kv_u32(&mut kv, "tokenizer.ggml.eos_token_id", 2); kv_count += 1;

        // Note: GGUF stores dims innermost-first [cols, rows] for 2D weights!
        let mut specs: Vec<(&str, Vec<u64>)> = vec![
            ("token_embd.weight", vec![hidden as u64, vocab as u64]),
            ("output_norm.weight", vec![hidden as u64]),
            ("blk.0.attn_norm.weight", vec![hidden as u64]),
            ("blk.0.attn_q.weight", vec![hidden as u64, hidden as u64]),
            ("blk.0.attn_k.weight", vec![hidden as u64, hidden as u64]),
            ("blk.0.attn_v.weight", vec![hidden as u64, hidden as u64]),
            ("blk.0.attn_output.weight", vec![hidden as u64, hidden as u64]),
            ("blk.0.ffn_norm.weight", vec![hidden as u64]),
            ("blk.0.ffn_gate.weight", vec![hidden as u64, ffn_hidden as u64]),
            ("blk.0.ffn_up.weight", vec![hidden as u64, ffn_hidden as u64]),
            ("blk.0.ffn_down.weight", vec![ffn_hidden as u64, hidden as u64]),
        ];
        if with_qkv_bias {
            specs.push(("blk.0.attn_q.bias", vec![hidden as u64]));
            specs.push(("blk.0.attn_k.bias", vec![hidden as u64]));
            specs.push(("blk.0.attn_v.bias", vec![hidden as u64]));
        }

        let mut tensor_infos = Vec::new();
        let mut data_payload = Vec::new();
        for (idx, (name, dims)) in specs.iter().enumerate() {
            let align_pad = (32 - (data_payload.len() % 32)) % 32;
            data_payload.resize(data_payload.len() + align_pad, 0);
            let offset = data_payload.len() as u64;

            write_gguf_string(&mut tensor_infos, name);
            tensor_infos.extend_from_slice(&(dims.len() as u32).to_le_bytes());
            for d in dims {
                tensor_infos.extend_from_slice(&d.to_le_bytes());
            }
            tensor_infos.extend_from_slice(&0u32.to_le_bytes()); // F32
            tensor_infos.extend_from_slice(&offset.to_le_bytes());

            let elems: usize = dims.iter().map(|d| *d as usize).product();
            for j in 0..elems {
                let val = if name.contains("norm") {
                    1.0f32
                } else {
                    let s = ((idx * 31 + j * 17) % 97) as f32 - 48.0;
                    s * 0.005
                };
                data_payload.extend_from_slice(&val.to_le_bytes());
            }
        }

        let mut file_buf = Vec::new();
        file_buf.extend_from_slice(b"GGUF");
        file_buf.extend_from_slice(&3u32.to_le_bytes()); // version 3
        file_buf.extend_from_slice(&(specs.len() as u64).to_le_bytes());
        file_buf.extend_from_slice(&kv_count.to_le_bytes());
        file_buf.extend_from_slice(&kv);
        file_buf.extend_from_slice(&tensor_infos);
        let pad = (32 - (file_buf.len() % 32)) % 32;
        file_buf.resize(file_buf.len() + pad, 0);
        file_buf.extend_from_slice(&data_payload);

        let mut f = File::create(path).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&file_buf).unwrap();
    }

    #[test]
    fn synthetic_llama_and_qwen2_gguf_forward_and_kv_cache_consistency() {
        let dir = tempfile::tempdir().unwrap();
        for (arch, bias) in [("llama", false), ("qwen2", true)] {
            let path = dir.path().join(format!("{arch}.gguf"));
            write_synthetic_gguf(&path, arch, 32, bias);

            let llm = GgufLlm::load(&path, None).expect("load synthetic gguf");
            let prompt = [1u32, 4, 5];

            // Verify KV-cache step prediction matches full non-cached forward logits.
            let mut kv_cached: Vec<Option<(Tensor, Tensor)>> = vec![None; llm.weights.layers.len()];
            let _ = llm.weights.forward(&prompt[..2], 0, &mut kv_cached).unwrap();
            let step_logits = llm
                .weights
                .forward(&prompt[2..], 2, &mut kv_cached)
                .unwrap()
                .to_vec1::<f32>()
                .unwrap();

            let mut kv_full: Vec<Option<(Tensor, Tensor)>> = vec![None; llm.weights.layers.len()];
            let full_logits = llm
                .weights
                .forward(&prompt, 0, &mut kv_full)
                .unwrap()
                .to_vec1::<f32>()
                .unwrap();

            for (a, b) in step_logits.iter().zip(full_logits.iter()) {
                assert!(
                    (a - b).abs() < 1e-4,
                    "{arch} KV cache mismatch: {a} vs {b}"
                );
            }

            let cancel = AtomicBool::new(false);
            let req = GenerateRequest {
                max_tokens: 4,
                temperature: 0.0,
                ..Default::default()
            };
            let mut streamed = String::new();
            let out = llm
                .generate_from_tokens(&prompt, &req, &cancel, &mut |tok| {
                    streamed.push_str(tok);
                })
                .unwrap();
            assert!(!out.cancelled);
            assert!(out.tokens_generated <= 4);
            assert_eq!(streamed.trim(), out.text);
        }
    }

    #[test]
    #[ignore = "requires local Ollama llama3.2 model on disk (~1.9 GB)"]
    fn live_llama32_grounded_rag_answer() {
        let default_dir = crate::manager::OnDemandLlm::default_ollama_models_dir();
        let discovered = crate::discover_local_llms(&default_dir);
        let llama = discovered
            .iter()
            .find(|m| m.name.to_ascii_lowercase().contains("llama3.2"))
            .expect("llama3.2 should be discovered in local Ollama models");

        let llm = GgufLlm::load(Path::new(&llama.path), Some(&llama.name))
            .expect("load llama3.2 GGUF");
        let cancel = AtomicBool::new(false);
        let req = GenerateRequest {
            system_prompt: "Answer strictly from the provided snippets and cite [1]. Keep your reply under 40 words.".to_string(),
            user_prompt: "Context:\n[1] vault_notes.txt: The Atlas workshop gate code is 4092 and the backup key is in locker B.\n\nQuestion: What is the Atlas workshop gate code?".to_string(),
            max_tokens: 64,
            temperature: 0.0,
            top_p: 0.9,
            repetition_penalty: 1.05,
            seed: 42,
            stop_sequences: Vec::new(),
            context_snippets: Vec::new(),
        };

        let mut streamed = Vec::new();
        let out = llm
            .generate(&req, &cancel, &mut |tok| streamed.push(tok.to_string()))
            .expect("generate live answer");
        eprintln!("LIVE LLAMA3.2 OUTPUT ({} tokens, {} ms): {}", out.tokens_generated, out.duration_ms, out.text);
        assert!(!out.cancelled);
        assert!(out.tokens_generated > 0);
        assert!(
            out.text.contains("4092"),
            "expected grounded answer containing 4092, got: {:?}",
            out.text
        );
    }
}
