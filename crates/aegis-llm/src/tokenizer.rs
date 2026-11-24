//! Pure-Rust GGUF tokenizer (`llama-bpe`, `qwen2` GPT-2 BPE, and SentencePiece) + chat templating.

use std::collections::HashMap;

use candle_core::quantized::gguf_file::{self, Value as GgufValue};

use crate::discovery::{gguf_bool, gguf_str, gguf_u32};
use crate::LlmError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTemplateKind {
    Llama3,
    ChatMl,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenizerKind {
    Gpt2Bpe,
    SentencePiece,
}

pub struct LlmTokenizer {
    tokens: Vec<String>,
    token_types: Vec<i32>,
    scores: Vec<f32>,
    token_to_id: HashMap<String, u32>,
    bpe_merges: HashMap<(u32, u32), (u32, u32)>,
    byte_tokens: [Option<u32>; 256],
    gpt2_b2u: [char; 256],
    gpt2_u2b: HashMap<char, u8>,
    special_tokens: Vec<(String, u32)>,
    kind: TokenizerKind,
    add_bos_token: bool,
    add_space_prefix: bool,
    pub bos_token_id: Option<u32>,
    pub eos_token_id: Option<u32>,
    pub eos_ids: Vec<u32>,
    pub chat_template: ChatTemplateKind,
}

impl LlmTokenizer {
    pub fn from_gguf(content: &gguf_file::Content, arch: &str) -> Result<Self, LlmError> {
        let tokens = extract_string_array(content, "tokenizer.ggml.tokens")
            .ok_or_else(|| LlmError::ModelLoad("GGUF missing tokenizer.ggml.tokens".into()))?;
        if tokens.is_empty() {
            return Err(LlmError::ModelLoad("GGUF tokenizer vocab is empty".into()));
        }

        let token_types = extract_i32_array(content, "tokenizer.ggml.token_type");
        let scores = extract_f32_array(content, "tokenizer.ggml.scores");
        let merges = extract_string_array(content, "tokenizer.ggml.merges").unwrap_or_default();
        let model_type = gguf_str(content, "tokenizer.ggml.model").unwrap_or_default();

        let bos_token_id = gguf_u32(content, "tokenizer.ggml.bos_token_id");
        let eos_token_id = gguf_u32(content, "tokenizer.ggml.eos_token_id");
        let add_bos_token = gguf_bool(content, "tokenizer.ggml.add_bos_token").unwrap_or(true);
        let add_space_prefix = gguf_bool(content, "tokenizer.ggml.add_space_prefix").unwrap_or(true);

        let (gpt2_b2u, gpt2_u2b) = build_gpt2_byte_tables();

        let mut token_to_id = HashMap::with_capacity(tokens.len());
        let mut special_tokens = Vec::new();
        for (idx, tok) in tokens.iter().enumerate() {
            let id = idx as u32;
            token_to_id.entry(tok.clone()).or_insert(id);
            let tt = token_types.get(idx).copied().unwrap_or(1);
            if tt == 3
                || matches!(
                    tok.as_str(),
                    "<|begin_of_text|>"
                        | "<|end_of_text|>"
                        | "<|start_header_id|>"
                        | "<|end_header_id|>"
                        | "<|eot_id|>"
                        | "<|im_start|>"
                        | "<|im_end|>"
                        | "<|endoftext|>"
                        | "<s>"
                        | "</s>"
                )
            {
                special_tokens.push((tok.clone(), id));
            }
        }
        special_tokens.sort_by_key(|a| std::cmp::Reverse(a.0.len()));

        let kind = if model_type == "gpt2" || !merges.is_empty() {
            TokenizerKind::Gpt2Bpe
        } else {
            TokenizerKind::SentencePiece
        };

        let mut byte_tokens = [None; 256];
        for b in 0u16..=255 {
            if kind == TokenizerKind::Gpt2Bpe {
                let s = gpt2_b2u[b as usize].to_string();
                byte_tokens[b as usize] = token_to_id.get(&s).copied();
            } else {
                let hex = format!("<0x{b:02X}>");
                if let Some(&id) = token_to_id.get(&hex) {
                    byte_tokens[b as usize] = Some(id);
                } else {
                    let lo = format!("<0x{b:02x}>");
                    byte_tokens[b as usize] = token_to_id.get(&lo).copied();
                }
            }
        }

        let mut bpe_merges = HashMap::with_capacity(merges.len());
        for (rank, line) in merges.iter().enumerate() {
            if let Some((left, right)) = line.split_once(' ')
                && let (Some(&l_id), Some(&r_id)) = (token_to_id.get(left), token_to_id.get(right))
            {
                let mut merged = String::with_capacity(left.len() + right.len());
                merged.push_str(left);
                merged.push_str(right);
                if let Some(&m_id) = token_to_id.get(&merged) {
                    bpe_merges
                        .entry((l_id, r_id))
                        .or_insert((rank as u32, m_id));
                }
            }
        }

        let mut eos_ids = Vec::new();
        if let Some(eos) = eos_token_id {
            eos_ids.push(eos);
        }
        for stop_tok in ["<|eot_id|>", "<|end_of_text|>", "<|im_end|>", "<|endoftext|>", "</s>"] {
            if let Some(&id) = token_to_id.get(stop_tok)
                && !eos_ids.contains(&id)
            {
                eos_ids.push(id);
            }
        }

        let template_raw = gguf_str(content, "tokenizer.chat_template").unwrap_or_default();
        let chat_template = if template_raw.contains("<|start_header_id|>")
            || (arch == "llama" && token_to_id.contains_key("<|start_header_id|>"))
        {
            ChatTemplateKind::Llama3
        } else if template_raw.contains("<|im_start|>")
            || (arch == "qwen2" && token_to_id.contains_key("<|im_start|>"))
        {
            ChatTemplateKind::ChatMl
        } else {
            ChatTemplateKind::Plain
        };

        Ok(Self {
            tokens,
            token_types,
            scores,
            token_to_id,
            bpe_merges,
            byte_tokens,
            gpt2_b2u,
            gpt2_u2b,
            special_tokens,
            kind,
            add_bos_token,
            add_space_prefix,
            bos_token_id,
            eos_token_id,
            eos_ids,
            chat_template,
        })
    }

    pub fn format_chat_prompt(&self, system_prompt: &str, user_prompt: &str) -> String {
        match self.chat_template {
            ChatTemplateKind::Llama3 => format!(
                "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{system_prompt}<|eot_id|><|start_header_id|>user<|end_header_id|>\n\n{user_prompt}<|eot_id|><|start_header_id|>assistant<|end_header_id|>\n\n"
            ),
            ChatTemplateKind::ChatMl => format!(
                "<|im_start|>system\n{system_prompt}<|im_end|>\n<|im_start|>user\n{user_prompt}<|im_end|>\n<|im_start|>assistant\n"
            ),
            ChatTemplateKind::Plain => {
                if system_prompt.trim().is_empty() {
                    format!("User: {user_prompt}\nAssistant:")
                } else {
                    format!("System: {system_prompt}\nUser: {user_prompt}\nAssistant:")
                }
            }
        }
    }

    pub fn is_eos(&self, token_id: u32) -> bool {
        self.eos_ids.contains(&token_id)
    }

    pub fn encode(&self, text: &str, add_bos: bool) -> Vec<u32> {
        let mut out = Vec::new();
        let mut rest = text;
        let starts_with_special_bos = rest.starts_with("<|begin_of_text|>") || rest.starts_with("<s>");

        if add_bos
            && self.add_bos_token
            && !starts_with_special_bos
            && let Some(bos) = self.bos_token_id
        {
            out.push(bos);
        }

        let mut is_first_plain = true;
        while !rest.is_empty() {
            if let Some((pos, sp_len, sp_id)) = self.find_next_special(rest) {
                if pos > 0 {
                    self.encode_plain(&rest[..pos], is_first_plain, &mut out);
                }
                out.push(sp_id);
                rest = &rest[pos + sp_len..];
                is_first_plain = false;
            } else {
                self.encode_plain(rest, is_first_plain, &mut out);
                break;
            }
        }
        out
    }

    /// Streams one generated token into valid UTF-8 text while buffering incomplete multi-byte sequences.
    pub fn decode_token_stream(&self, token_id: u32, pending_bytes: &mut Vec<u8>) -> String {
        let idx = token_id as usize;
        let Some(piece) = self.tokens.get(idx) else {
            return String::new();
        };
        let tt = self.token_types.get(idx).copied().unwrap_or(1);
        if tt == 3 || self.is_eos(token_id) {
            return String::new();
        }

        match self.kind {
            TokenizerKind::Gpt2Bpe => {
                for ch in piece.chars() {
                    if let Some(&b) = self.gpt2_u2b.get(&ch) {
                        pending_bytes.push(b);
                    } else {
                        let mut buf = [0u8; 4];
                        let s = ch.encode_utf8(&mut buf);
                        pending_bytes.extend_from_slice(s.as_bytes());
                    }
                }
            }
            TokenizerKind::SentencePiece => {
                if let Some(byte_val) = parse_byte_token(piece) {
                    pending_bytes.push(byte_val);
                } else {
                    let replaced = piece.replace('▁', " ");
                    pending_bytes.extend_from_slice(replaced.as_bytes());
                }
            }
        }

        flush_valid_utf8(pending_bytes)
    }

    pub fn decode_ids(&self, ids: &[u32]) -> String {
        let mut pending = Vec::new();
        let mut out = String::new();
        for &id in ids {
            out.push_str(&self.decode_token_stream(id, &mut pending));
        }
        if !pending.is_empty() {
            out.push_str(&String::from_utf8_lossy(&pending));
        }
        out
    }

    fn find_next_special(&self, text: &str) -> Option<(usize, usize, u32)> {
        if self.special_tokens.is_empty() {
            return None;
        }
        let mut best: Option<(usize, usize, u32)> = None;
        for (tok, id) in &self.special_tokens {
            if let Some(pos) = text.find(tok.as_str()) {
                match best {
                    None => best = Some((pos, tok.len(), *id)),
                    Some((best_pos, best_len, _)) => {
                        if pos < best_pos || (pos == best_pos && tok.len() > best_len) {
                            best = Some((pos, tok.len(), *id));
                        }
                    }
                }
            }
        }
        best
    }

    fn encode_plain(&self, chunk: &str, is_first_plain: bool, out: &mut Vec<u32>) {
        match self.kind {
            TokenizerKind::Gpt2Bpe => {
                for piece in gpt2_pretokenize(chunk) {
                    self.encode_gpt2_piece(piece, out);
                }
            }
            TokenizerKind::SentencePiece => {
                self.encode_spm_piece(chunk, is_first_plain, out);
            }
        }
    }

    fn encode_gpt2_piece(&self, piece: &str, out: &mut Vec<u32>) {
        let bytes = piece.as_bytes();
        if bytes.is_empty() {
            return;
        }

        // Fast exact vocab lookup for already-single-token words.
        if bytes.len() <= 64 {
            let mut mapped = String::with_capacity(bytes.len() * 2);
            for &b in bytes {
                mapped.push(self.gpt2_b2u[b as usize]);
            }
            if let Some(&id) = self.token_to_id.get(&mapped) {
                out.push(id);
                return;
            }
        }

        let mut work: Vec<u32> = Vec::with_capacity(bytes.len());
        for &b in bytes {
            if let Some(id) = self.byte_tokens[b as usize] {
                work.push(id);
            }
        }

        while work.len() >= 2 {
            let mut best_rank = u32::MAX;
            let mut best_pair = None;
            let mut best_merged = 0u32;

            for i in 0..(work.len() - 1) {
                if let Some(&(rank, merged)) = self.bpe_merges.get(&(work[i], work[i + 1]))
                    && rank < best_rank
                {
                    best_rank = rank;
                    best_pair = Some((work[i], work[i + 1]));
                    best_merged = merged;
                }
            }

            let Some((a, b)) = best_pair else {
                break;
            };

            let mut next = Vec::with_capacity(work.len());
            let mut i = 0;
            while i < work.len() {
                if i + 1 < work.len() && work[i] == a && work[i + 1] == b {
                    next.push(best_merged);
                    i += 2;
                } else {
                    next.push(work[i]);
                    i += 1;
                }
            }
            work = next;
        }

        out.extend_from_slice(&work);
    }

    fn encode_spm_piece(&self, chunk: &str, is_first_plain: bool, out: &mut Vec<u32>) {
        if chunk.is_empty() {
            return;
        }
        let mut normalized = String::with_capacity(chunk.len() + 4);
        if is_first_plain && self.add_space_prefix && !chunk.starts_with(' ') {
            normalized.push('▁');
        }
        for ch in chunk.chars() {
            if ch == ' ' {
                normalized.push('▁');
            } else {
                normalized.push(ch);
            }
        }

        let mut syms: Vec<u32> = Vec::new();
        let mut buf = [0u8; 4];
        for ch in normalized.chars() {
            let s = ch.encode_utf8(&mut buf);
            if let Some(&id) = self.token_to_id.get(s) {
                syms.push(id);
            } else {
                for &b in s.as_bytes() {
                    if let Some(bid) = self.byte_tokens[b as usize] {
                        syms.push(bid);
                    }
                }
            }
        }

        while syms.len() >= 2 {
            let mut best_idx = None;
            let mut best_score = f32::NEG_INFINITY;
            let mut best_merged = 0u32;

            for i in 0..(syms.len() - 1) {
                let left = &self.tokens[syms[i] as usize];
                let right = &self.tokens[syms[i + 1] as usize];
                let mut combined = String::with_capacity(left.len() + right.len());
                combined.push_str(left);
                combined.push_str(right);
                if let Some(&mid) = self.token_to_id.get(&combined) {
                    let sc = self.scores.get(mid as usize).copied().unwrap_or(0.0);
                    if sc > best_score {
                        best_score = sc;
                        best_idx = Some(i);
                        best_merged = mid;
                    }
                }
            }

            let Some(pos) = best_idx else {
                break;
            };
            syms[pos] = best_merged;
            syms.remove(pos + 1);
        }

        out.extend_from_slice(&syms);
    }
}

/// Llama-3 / Qwen2 pure-Rust pretokenizer matching the GPT-2/BPE regex branches in priority order.
fn gpt2_pretokenize(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    let len = text.len();

    while i < len {
        let rem = &text[i..];

        // Branch 1: `'(?i:[sdmt]|ll|ve|re)`
        if let Some(clen) = match_contraction(rem) {
            out.push(&rem[..clen]);
            i += clen;
            continue;
        }

        let mut chars = rem.char_indices();
        let (_, c0) = chars.next().expect("non-empty rem");

        // Branch 2: `[^\r\n\p{L}\p{N}]?\p{L}+`
        if c0.is_alphabetic() {
            let mut end = c0.len_utf8();
            for (idx, c) in chars {
                if c.is_alphabetic() {
                    end = idx + c.len_utf8();
                } else {
                    break;
                }
            }
            out.push(&rem[..end]);
            i += end;
            continue;
        }
        if c0 != '\r' && c0 != '\n' && !c0.is_numeric() {
            let after0 = c0.len_utf8();
            let mut rest_chars = rem[after0..].char_indices();
            if let Some((_, c1)) = rest_chars.next()
                && c1.is_alphabetic()
            {
                let mut end = after0 + c1.len_utf8();
                for (idx, c) in rest_chars {
                    if c.is_alphabetic() {
                        end = after0 + idx + c.len_utf8();
                    } else {
                        break;
                    }
                }
                out.push(&rem[..end]);
                i += end;
                continue;
            }
        }

        // Branch 3: `\p{N}{1,3}`
        if c0.is_numeric() {
            let mut end = c0.len_utf8();
            let mut count = 1;
            for (idx, c) in rem[end..].char_indices() {
                if count < 3 && c.is_numeric() {
                    end = c0.len_utf8() + idx + c.len_utf8();
                    count += 1;
                } else {
                    break;
                }
            }
            out.push(&rem[..end]);
            i += end;
            continue;
        }

        // Branch 4: ` ?[^\s\p{L}\p{N}]+[\r\n]*`
        let punct_start = if c0 == ' ' {
            let after = c0.len_utf8();
            rem[after..]
                .chars()
                .next()
                .filter(|c| !c.is_whitespace() && !c.is_alphabetic() && !c.is_numeric())
                .map(|_| after)
        } else if !c0.is_whitespace() && !c0.is_alphabetic() && !c0.is_numeric() {
            Some(0)
        } else {
            None
        };
        if let Some(ps) = punct_start {
            let mut end = ps;
            for (idx, c) in rem[ps..].char_indices() {
                if !c.is_whitespace() && !c.is_alphabetic() && !c.is_numeric() {
                    end = ps + idx + c.len_utf8();
                } else {
                    break;
                }
            }
            let tail = end;
            for (idx, c) in rem[tail..].char_indices() {
                if c == '\r' || c == '\n' {
                    end = tail + idx + c.len_utf8();
                } else {
                    break;
                }
            }
            out.push(&rem[..end]);
            i += end;
            continue;
        }

        // Branch 5/6/7: `\s*[\r\n]+` | `\s+(?!\S)` | `\s+`
        if c0.is_whitespace() {
            let mut ws_chars: Vec<(usize, char)> = Vec::new();
            let mut ws_end = 0;
            let mut has_newline = false;
            let mut last_nl_end = 0;
            for (idx, c) in rem.char_indices() {
                if !c.is_whitespace() {
                    break;
                }
                let ce = idx + c.len_utf8();
                ws_end = ce;
                ws_chars.push((ce, c));
                if c == '\r' || c == '\n' {
                    has_newline = true;
                    last_nl_end = ce;
                } else if has_newline {
                    // Stop extending Branch 5 after newlines end.
                    break;
                }
            }

            if has_newline {
                out.push(&rem[..last_nl_end]);
                i += last_nl_end;
                continue;
            }

            // `\s+(?!\S)`: if followed by non-whitespace and there are >= 2 spaces, emit all but last.
            if ws_end < rem.len() && ws_chars.len() >= 2 {
                let emit_end = ws_chars[ws_chars.len() - 2].0;
                out.push(&rem[..emit_end]);
                i += emit_end;
                continue;
            }

            out.push(&rem[..ws_end]);
            i += ws_end;
            continue;
        }

        // Defensive single-char advance.
        let step = c0.len_utf8();
        out.push(&rem[..step]);
        i += step;
    }

    out
}

fn match_contraction(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first().copied() != Some(b'\'') {
        return None;
    }
    let b1 = bytes.get(1)?.to_ascii_lowercase();
    if let Some(&b2) = bytes.get(2) {
        let pair = [b1, b2.to_ascii_lowercase()];
        if matches!(&pair, b"ll" | b"ve" | b"re") {
            return Some(3);
        }
    }
    if matches!(b1, b's' | b'd' | b'm' | b't') {
        return Some(2);
    }
    None
}

fn build_gpt2_byte_tables() -> ([char; 256], HashMap<char, u8>) {
    let mut b2u = ['\0'; 256];
    let mut assigned = [false; 256];
    let mut n = 0u32;

    for b in 0u16..=255 {
        let ch_ok = matches!(b, 0x21..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF);
        if ch_ok {
            let ch = char::from_u32(b as u32).expect("valid latin-1 scalar");
            b2u[b as usize] = ch;
            assigned[b as usize] = true;
        }
    }
    for b in 0u16..=255 {
        if !assigned[b as usize] {
            let ch = char::from_u32(256 + n).expect("valid extended scalar");
            b2u[b as usize] = ch;
            n += 1;
        }
    }

    let mut u2b = HashMap::with_capacity(256);
    for (byte, &ch) in b2u.iter().enumerate() {
        u2b.insert(ch, byte as u8);
    }
    (b2u, u2b)
}

fn parse_byte_token(piece: &str) -> Option<u8> {
    let hex = piece.strip_prefix("<0x")?.strip_suffix('>')?;
    if hex.len() != 2 {
        return None;
    }
    u8::from_str_radix(hex, 16).ok()
}

fn flush_valid_utf8(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                out.push_str(valid);
                pending.clear();
                break;
            }
            Err(err) => {
                let valid_len = err.valid_up_to();
                if valid_len > 0 {
                    let s = std::str::from_utf8(&pending[..valid_len]).expect("valid utf8 prefix");
                    out.push_str(s);
                    pending.drain(..valid_len);
                }
                match err.error_len() {
                    Some(bad_len) => {
                        out.push('\u{FFFD}');
                        pending.drain(..bad_len);
                    }
                    None => {
                        // Incomplete multi-byte trailing sequence: keep waiting for next token.
                        break;
                    }
                }
            }
        }
    }
    out
}

fn extract_string_array(content: &gguf_file::Content, key: &str) -> Option<Vec<String>> {
    let GgufValue::Array(arr) = content.metadata.get(key)? else {
        return None;
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let GgufValue::String(s) = item else {
            return None;
        };
        out.push(s.clone());
    }
    Some(out)
}

fn extract_i32_array(content: &gguf_file::Content, key: &str) -> Vec<i32> {
    let Some(GgufValue::Array(arr)) = content.metadata.get(key) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| match v {
            GgufValue::I32(n) => Some(*n),
            GgufValue::U32(n) => i32::try_from(*n).ok(),
            _ => None,
        })
        .collect()
}

fn extract_f32_array(content: &gguf_file::Content, key: &str) -> Vec<f32> {
    let Some(GgufValue::Array(arr)) = content.metadata.get(key) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| match v {
            GgufValue::F32(f) => Some(*f),
            GgufValue::F64(f) => Some(*f as f32),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt2_pretokenizer_splits_contractions_words_numbers_and_newlines() {
        let got = gpt2_pretokenize("Hello, world! Vault's pin: 4092\n\nNext.");
        assert_eq!(
            got,
            vec!["Hello", ",", " world", "!", " Vault", "'s", " pin", ":", " ", "409", "2", "\n\n", "Next", "."]
        );
    }

    #[test]
    fn flush_valid_utf8_buffers_split_multibyte_codepoints() {
        let mut pending = vec![0xF0, 0x9F]; // First 2 bytes of 😀 (F0 9F 98 80)
        assert_eq!(flush_valid_utf8(&mut pending), "");
        pending.extend_from_slice(&[0x98, 0x80]);
        assert_eq!(flush_valid_utf8(&mut pending), "😀");
        assert!(pending.is_empty());
    }
}
