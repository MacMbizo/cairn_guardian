//! Pure-Rust BERT WordPiece tokenizer supporting GGUF vocabularies and HuggingFace `vocab.txt`.

use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

const MAX_WORD_CHARS: usize = 200;

/// Minimal pure-Rust WordPiece tokenizer (zero C++ dependencies, zero network calls).
#[derive(Debug, Clone)]
pub struct WordPieceTokenizer {
    vocab: HashMap<String, u32>,
    gguf_sp_style: bool,
    unk_id: u32,
    cls_id: u32,
    sep_id: u32,
    pad_id: u32,
    do_lower_case: bool,
    strip_accents: bool,
    max_len: usize,
}

impl WordPieceTokenizer {
    /// Builds a tokenizer from an ordered vocabulary token table.
    pub fn from_tokens(
        tokens: Vec<String>,
        cls_id: Option<u32>,
        sep_id: Option<u32>,
        unk_id: Option<u32>,
        pad_id: Option<u32>,
        max_len: usize,
    ) -> Self {
        // `convert_hf_to_gguf.py` exports BERT WordPiece tables using `\u{2581}` (`▁`) on word starts
        // and bare continuation subwords (omitting `##`), whereas HuggingFace `vocab.txt` uses `##`.
        let gguf_sp_style = tokens.iter().any(|t| t.starts_with('\u{2581}'));
        let mut vocab = HashMap::with_capacity(tokens.len());
        for (i, tok) in tokens.into_iter().enumerate() {
            vocab.insert(tok, i as u32);
        }
        let lookup = |sym: &str, fallback: u32| -> u32 { vocab.get(sym).copied().unwrap_or(fallback) };
        let cls_id = cls_id.unwrap_or_else(|| lookup("[CLS]", 101));
        let sep_id = sep_id.unwrap_or_else(|| lookup("[SEP]", 102));
        let unk_id = unk_id.unwrap_or_else(|| lookup("[UNK]", 100));
        let pad_id = pad_id.unwrap_or_else(|| lookup("[PAD]", 0));
        Self {
            vocab,
            gguf_sp_style,
            unk_id,
            cls_id,
            sep_id,
            pad_id,
            do_lower_case: true,
            strip_accents: true,
            max_len: max_len.max(8),
        }
    }

    /// Loads a standard HuggingFace line-delimited `vocab.txt` file.
    pub fn from_vocab_file(path: &Path, max_len: usize) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let tokens: Vec<String> = raw
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        Ok(Self::from_tokens(tokens, None, None, None, None, max_len))
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    pub fn pad_id(&self) -> u32 {
        self.pad_id
    }

    pub fn max_len(&self) -> usize {
        self.max_len
    }

    /// Encodes `text` with `[CLS] ... [SEP]`, truncated to `self.max_len`.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let max_content = self.max_len.saturating_sub(2);
        let mut ids = Vec::with_capacity(64);
        ids.push(self.cls_id);
        for word in self.basic_tokenize(text) {
            for id in self.wordpiece_tokenize(&word) {
                if ids.len() > max_content {
                    ids.push(self.sep_id);
                    return ids;
                }
                ids.push(id);
            }
        }
        ids.push(self.sep_id);
        ids
    }

    fn basic_tokenize(&self, text: &str) -> Vec<String> {
        let mut cleaned = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch == '\0' || ch == '\u{FFFD}' {
                continue;
            }
            if is_cjk_char(ch) {
                cleaned.push(' ');
                cleaned.push(ch);
                cleaned.push(' ');
            } else if ch.is_whitespace() {
                cleaned.push(' ');
            } else if is_bert_punctuation(ch) {
                cleaned.push(' ');
                cleaned.push(ch);
                cleaned.push(' ');
            } else if self.do_lower_case {
                for lc in ch.to_lowercase() {
                    if self.strip_accents {
                        for d in lc.nfd() {
                            if !unicode_normalization::char::is_combining_mark(d) {
                                cleaned.push(d);
                            }
                        }
                    } else {
                        cleaned.push(lc);
                    }
                }
            } else {
                cleaned.push(ch);
            }
        }
        cleaned
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn wordpiece_tokenize(&self, word: &str) -> Vec<u32> {
        let char_count = word.chars().count();
        if char_count == 0 || char_count > MAX_WORD_CHARS {
            return vec![self.unk_id];
        }
        let mut byte_offsets: Vec<usize> = word.char_indices().map(|(i, _)| i).collect();
        byte_offsets.push(word.len());
        let n = byte_offsets.len() - 1;

        let mut start = 0usize;
        let mut sub_ids = Vec::new();
        while start < n {
            let mut end = n;
            let mut found: Option<u32> = None;
            while start < end {
                let slice = &word[byte_offsets[start]..byte_offsets[end]];
                let probe;
                let candidate = if self.gguf_sp_style {
                    if start == 0 {
                        probe = format!("\u{2581}{slice}");
                        probe.as_str()
                    } else {
                        slice
                    }
                } else if start > 0 {
                    probe = format!("##{slice}");
                    probe.as_str()
                } else {
                    slice
                };
                if let Some(&id) = self.vocab.get(candidate) {
                    found = Some(id);
                    break;
                }
                end -= 1;
            }
            let Some(id) = found else {
                return vec![self.unk_id];
            };
            sub_ids.push(id);
            start = end;
        }
        sub_ids
    }
}

fn is_bert_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() || (!ch.is_alphanumeric() && !ch.is_whitespace() && !is_cjk_char(ch))
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}
