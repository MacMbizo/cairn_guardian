//! Always-available zero-dependency lexical hash embedder (`< 1 MB` RAM fallback).
//!
//! Uses deterministic BLAKE3 signed feature hashing over lowercase tokens, camelCase/snake_case
//! splits, word bigrams, and character 3-grams so semantic search still degrades gracefully when no
//! neural model file is installed on disk.

use crate::{EmbedError, EmbeddingProvider};

pub const LEXICAL_MODEL_ID: &str = "builtin:lexical-hash-384";
pub const LEXICAL_DIM: usize = 384;

#[derive(Debug, Default, Clone, Copy)]
pub struct LexicalHashEmbedder;

impl LexicalHashEmbedder {
    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; LEXICAL_DIM];
        let mut words: Vec<String> = Vec::new();
        for tok in split_lexical_tokens(text) {
            if tok.len() >= 2 {
                add_feature(&mut vec, "w", &tok, 2.0);
                let chars: Vec<char> = tok.chars().collect();
                if chars.len() >= 3 {
                    for win in chars.windows(3) {
                        let tri: String = win.iter().collect();
                        add_feature(&mut vec, "c3", &tri, 0.7);
                    }
                }
                words.push(tok);
            }
        }
        for pair in words.windows(2) {
            let bigram = format!("{}_{}", pair[0], pair[1]);
            add_feature(&mut vec, "b2", &bigram, 1.4);
        }
        l2_normalize(&mut vec);
        vec
    }
}

impl EmbeddingProvider for LexicalHashEmbedder {
    fn model_id(&self) -> &str {
        LEXICAL_MODEL_ID
    }

    fn display_name(&self) -> &str {
        "Built-in Lexical Hash (384-d, < 1 MB)"
    }

    fn backend_kind(&self) -> &str {
        "lexical_fallback"
    }

    fn dimensions(&self) -> usize {
        LEXICAL_DIM
    }

    fn embed_batch(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}

fn split_lexical_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut raw = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            raw.push(ch);
        } else if !raw.is_empty() {
            push_subtokens(&raw, &mut out);
            raw.clear();
        }
    }
    if !raw.is_empty() {
        push_subtokens(&raw, &mut out);
    }
    out
}

fn push_subtokens(token: &str, out: &mut Vec<String>) {
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in token.chars() {
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(cur.to_lowercase());
            cur.clear();
        }
        prev_lower = ch.is_lowercase();
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur.to_lowercase());
    }
}

fn add_feature(vec: &mut [f32], ns: &str, feat: &str, weight: f32) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ns.as_bytes());
    hasher.update(b":");
    hasher.update(feat.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let idx = (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize) % LEXICAL_DIM;
    let sign = if (bytes[4] & 1) == 0 { 1.0f32 } else { -1.0f32 };
    vec[idx] += sign * weight;
}

pub(crate) fn l2_normalize(v: &mut [f32]) {
    let sum_sq: f64 = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
    if sum_sq > 0.0 {
        let inv = (1.0 / sum_sq.sqrt()) as f32;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}
