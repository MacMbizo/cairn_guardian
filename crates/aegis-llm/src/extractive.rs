//! Zero-RAM grounded extractive summarizer fallback (`K5` Eco mode / GGUF fallback).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::discovery::builtin_extractive_descriptor;
use crate::{ContextSnippet, GenerateOutput, GenerateRequest, LlmError, LlmModelDescriptor, TextGenerator};

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "what", "when", "where", "which", "who",
    "why", "how", "are", "was", "were", "will", "would", "could", "should", "have", "has", "had",
    "does", "did", "can", "about", "into", "your", "file", "files", "find", "tell", "show",
];

pub struct ExtractiveGenerator {
    descriptor: LlmModelDescriptor,
}

impl ExtractiveGenerator {
    pub fn new() -> Self {
        Self {
            descriptor: builtin_extractive_descriptor(),
        }
    }
}

impl Default for ExtractiveGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl TextGenerator for ExtractiveGenerator {
    fn descriptor(&self) -> &LlmModelDescriptor {
        &self.descriptor
    }

    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &AtomicBool,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateOutput, LlmError> {
        let started = Instant::now();
        if cancel.load(Ordering::Relaxed) {
            return Err(LlmError::Cancelled);
        }

        let snippets = if !req.context_snippets.is_empty() {
            req.context_snippets.clone()
        } else {
            parse_prompt_contexts(&req.user_prompt)
        };

        let answer = synthesize_extractive_answer(&req.user_prompt, &snippets);

        // Emit word-level chunks to exercise the same streaming UI contract as local GGUF LLMs.
        let mut emitted = String::new();
        let mut token_count = 0usize;
        for chunk in split_stream_chunks(&answer) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(GenerateOutput {
                    text: emitted,
                    tokens_generated: token_count,
                    prompt_tokens: req.user_prompt.split_whitespace().count(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    cancelled: true,
                    model_id: self.descriptor.id.clone(),
                    backend: self.descriptor.backend.clone(),
                });
            }
            emitted.push_str(&chunk);
            token_count += 1;
            on_token(&chunk);
        }

        Ok(GenerateOutput {
            text: emitted,
            tokens_generated: token_count,
            prompt_tokens: req.user_prompt.split_whitespace().count(),
            duration_ms: started.elapsed().as_millis() as u64,
            cancelled: false,
            model_id: self.descriptor.id.clone(),
            backend: self.descriptor.backend.clone(),
        })
    }
}

fn synthesize_extractive_answer(question: &str, snippets: &[ContextSnippet]) -> String {
    if snippets.is_empty() {
        return "I could not find any indexed non-sensitive snippets matching your question."
            .to_string();
    }

    let query_terms = extract_terms(question);
    let mut ranked: Vec<(usize, usize, String, String)> = Vec::new();

    for snippet in snippets {
        let mut best_line = String::new();
        let mut best_score = 0usize;
        for segment in split_candidate_sentences(&snippet.text) {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }
            let seg_lower = trimmed.to_ascii_lowercase();
            let mut score = 0usize;
            for term in &query_terms {
                if seg_lower.contains(term) {
                    score += 3;
                }
            }
            if trimmed.chars().any(|c| c.is_ascii_digit()) {
                score += 1;
            }
            if score > best_score || best_line.is_empty() {
                best_score = score;
                best_line = trimmed.to_string();
            }
        }
        if best_line.is_empty() {
            best_line = snippet.text.trim().chars().take(240).collect();
        }
        ranked.push((best_score, snippet.ref_id, snippet.label.clone(), best_line));
    }

    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut lines = Vec::new();
    let mut seen_refs = BTreeSet::new();
    for (_, ref_id, label, line) in ranked.into_iter().take(4) {
        if !seen_refs.insert(ref_id) {
            continue;
        }
        let cleaned = line.replace(['\r', '\n'], " ");
        let short: String = cleaned.trim().chars().take(260).collect();
        if label.is_empty() {
            lines.push(format!("- [{ref_id}] {short}"));
        } else {
            lines.push(format!("- [{ref_id}] ({label}) {short}"));
        }
    }

    format!(
        "Based on the indexed local files (extractive mode):\n{}",
        lines.join("\n")
    )
}

fn parse_prompt_contexts(prompt: &str) -> Vec<ContextSnippet> {
    let mut out = Vec::new();
    let mut current_id: Option<usize> = None;
    let mut current_label = String::new();
    let mut current_body = Vec::new();

    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[')
            && let Some(end) = trimmed.find(']')
            && let Ok(id) = trimmed[1..end].trim().parse::<usize>()
        {
            if let Some(prev_id) = current_id.take() {
                out.push(ContextSnippet {
                    ref_id: prev_id,
                    label: std::mem::take(&mut current_label),
                    text: current_body.join(" "),
                });
                current_body.clear();
            }
            current_id = Some(id);
            current_label = trimmed[end + 1..].trim().to_string();
            continue;
        }
        if trimmed.starts_with("Question:") {
            break;
        }
        if current_id.is_some() && !trimmed.is_empty() {
            current_body.push(trimmed.to_string());
        }
    }

    if let Some(prev_id) = current_id {
        out.push(ContextSnippet {
            ref_id: prev_id,
            label: current_label,
            text: current_body.join(" "),
        });
    }
    out
}

fn extract_terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.len() >= 2 && !STOPWORDS.contains(&lower.as_str()) && seen.insert(lower.clone()) {
            out.push(lower);
        }
    }
    out
}

fn split_candidate_sentences(text: &str) -> Vec<&str> {
    text.split(['\n', ';', '。', '.'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn split_stream_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if ch.is_whitespace() && !buf.is_empty() {
            chunks.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        chunks.push(buf);
    }
    chunks
}
