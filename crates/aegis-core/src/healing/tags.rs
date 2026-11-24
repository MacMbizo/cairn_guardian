//! Automated YAML frontmatter & inline `#tag` extraction, typo/case clustering,
//! and hex-color false-positive detection (`#A3E2F1` outside code blocks).

use super::TagWeedKind;
use super::markdown::{
    SpanReplacement, excluded_code_ranges, frontmatter_end, line_for_offset, line_starts,
    offset_in_ranges,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Single extracted tag occurrence across YAML frontmatter or inline Markdown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTag {
    pub raw_tag: String,
    pub normalized_value: String,
    pub line_number: usize,
    pub in_frontmatter: bool,
    pub had_hash_prefix: bool,
    /// Span covering the full replaceable tag value (for pruning a frontmatter entry cleanly
    /// this covers whitespace/comma/list line when `prune_span` is populated).
    pub value_span: (usize, usize),
    pub prune_span: (usize, usize),
    pub snippet: String,
}

/// Extracted tag weeds detected across scanned workspace notes.
#[derive(Debug, Clone)]
pub struct DetectedTagWeed {
    pub kind: TagWeedKind,
    pub canonical_tag: String,
    pub confidence: f64,
    pub reason: String,
    pub occurrence: TagOccMeta,
}

#[derive(Debug, Clone)]
pub struct TagOccMeta {
    pub file_id: String,
    pub file_path: String,
    pub file_name: String,
    pub line_number: usize,
    pub in_frontmatter: bool,
    pub raw_tag: String,
    pub canonical_tag: String,
    pub patchable: bool,
    pub sensitive: bool,
    pub snippet: String,
    pub span: (usize, usize),
    pub replacement: String,
}

pub fn extract_tags_from_markdown(text: &str) -> Vec<ParsedTag> {
    let mut tags = Vec::new();
    let starts = line_starts(text);
    let fm_end = frontmatter_end(text);

    if fm_end > 0 {
        extract_frontmatter_tags(text, fm_end, &starts, &mut tags);
    }

    let excluded = excluded_code_ranges(text, true);
    extract_inline_tags(text, fm_end, &excluded, &starts, &mut tags);
    tags
}

fn line_snippet_at(text: &str, byte_offset: usize) -> String {
    let start = text[..byte_offset]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = text[byte_offset..]
        .find('\n')
        .map(|i| byte_offset + i)
        .unwrap_or(text.len());
    text[start..end].trim_end_matches('\r').trim().chars().take(160).collect()
}

fn extract_frontmatter_tags(
    text: &str,
    fm_end: usize,
    starts: &[usize],
    out: &mut Vec<ParsedTag>,
) {
    let mut pos = 0usize;
    let mut in_tags_list = false;
    while pos < fm_end {
        let nl = text[pos..fm_end]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(fm_end);
        let next_pos = if nl < fm_end { nl + 1 } else { fm_end };
        let raw_line = &text[pos..nl];
        let line_no_cr = raw_line.trim_end_matches('\r');
        let trimmed = line_no_cr.trim();

        if trimmed == "---" || trimmed == "..." {
            in_tags_list = false;
            pos = next_pos;
            continue;
        }

        if let Some(after_key) = strip_yaml_tags_key(line_no_cr) {
            let trimmed_val = after_key.trim();
            if trimmed_val.is_empty() {
                in_tags_list = true;
            } else {
                in_tags_list = false;
                let base = pos + (line_no_cr.len() - after_key.len());
                parse_inline_yaml_list(text, after_key, base, starts, out);
            }
            pos = next_pos;
            continue;
        }

        if in_tags_list {
            let leading = line_no_cr.len() - line_no_cr.trim_start_matches([' ', '\t']).len();
            if line_no_cr[leading..].starts_with('-') {
                let after_dash = &line_no_cr[leading + 1..];
                let ws_after_dash = after_dash.len() - after_dash.trim_start_matches([' ', '\t']).len();
                let item_raw = after_dash[ws_after_dash..].trim_end();
                let item_start = pos + leading + 1 + ws_after_dash;
                let (val_start, val_end, had_hash) = unwrap_yaml_scalar(text, item_start, item_start + item_raw.len());
                if val_start < val_end {
                    let norm = text[val_start..val_end].to_string();
                    out.push(ParsedTag {
                        raw_tag: if had_hash {
                            format!("#{norm}")
                        } else {
                            norm.clone()
                        },
                        normalized_value: norm,
                        line_number: line_for_offset(starts, pos),
                        in_frontmatter: true,
                        had_hash_prefix: had_hash,
                        value_span: (val_start, val_end),
                        prune_span: (pos, next_pos),
                        snippet: line_snippet_at(text, pos),
                    });
                }
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                in_tags_list = false;
            }
        }
        pos = next_pos;
    }
}

fn strip_yaml_tags_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    for key in ["tags:", "tag:"] {
        if let Some(rest) = trimmed.strip_prefix(key) {
            return Some(rest);
        }
        let prefix = trimmed.get(..key.len())?;
        if prefix.eq_ignore_ascii_case(key) {
            return Some(&trimmed[key.len()..]);
        }
    }
    None
}

fn unwrap_yaml_scalar(text: &str, start: usize, end: usize) -> (usize, usize, bool) {
    let mut s = start;
    let mut e = end;
    let slice = &text[s..e];
    if (slice.starts_with('"') && slice.ends_with('"') && slice.len() >= 2)
        || (slice.starts_with('\'') && slice.ends_with('\'') && slice.len() >= 2)
    {
        s += 1;
        e -= 1;
    }
    if s < e && text.as_bytes()[s] == b'#' {
        (s + 1, e, true)
    } else {
        (s, e, false)
    }
}

fn parse_inline_yaml_list(
    text: &str,
    after_key: &str,
    base: usize,
    starts: &[usize],
    out: &mut Vec<ParsedTag>,
) {
    let leading_ws = after_key.len() - after_key.trim_start().len();
    let trimmed = after_key[leading_ws..].trim_end();
    if trimmed.is_empty() {
        return;
    }
    let (inner_start, inner_end) = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        (base + leading_ws + 1, base + leading_ws + trimmed.len() - 1)
    } else {
        (base + leading_ws, base + leading_ws + trimmed.len())
    };

    let mut cur = inner_start;
    let inner = &text[inner_start..inner_end];
    let raw_parts: Vec<(usize, usize)> = {
        let mut parts = Vec::new();
        let mut item_s = 0usize;
        for (idx, b) in inner.bytes().enumerate() {
            if b == b',' {
                parts.push((inner_start + item_s, inner_start + idx));
                item_s = idx + 1;
            }
        }
        parts.push((inner_start + item_s, inner_end));
        parts
    };

    for (idx, (ps, pe)) in raw_parts.iter().enumerate() {
        let chunk = &text[*ps..*pe];
        let ltrim = chunk.len() - chunk.trim_start().len();
        let rtrim = chunk.len() - chunk.trim_end().len();
        let ts = ps + ltrim;
        let te = pe.saturating_sub(rtrim);
        if ts >= te {
            cur = *pe;
            continue;
        }
        let (val_start, val_end, had_hash) = unwrap_yaml_scalar(text, ts, te);
        if val_start < val_end {
            let prune_span = if raw_parts.len() == 1 {
                (ts, te)
            } else if idx + 1 < raw_parts.len() {
                // Remove through comma and leading spaces of next element (`raw_parts[idx + 1].0` is right after `,`).
                let next_s = raw_parts[idx + 1].0;
                let after_comma = &text[next_s..];
                let ws_next = after_comma.len() - after_comma.trim_start_matches([' ', '\t']).len();
                (ts, next_s + ws_next)
            } else {
                // Last element: remove comma before this element.
                (*ps - 1, te)
            };
            let norm = text[val_start..val_end].to_string();
            out.push(ParsedTag {
                raw_tag: if had_hash {
                    format!("#{norm}")
                } else {
                    norm.clone()
                },
                normalized_value: norm,
                line_number: line_for_offset(starts, val_start),
                in_frontmatter: true,
                had_hash_prefix: had_hash,
                value_span: (val_start, val_end),
                prune_span,
                snippet: line_snippet_at(text, val_start),
            });
        }
        cur = *pe;
    }
    let _ = cur;
}

fn extract_inline_tags(
    text: &str,
    start_from: usize,
    excluded: &[(usize, usize)],
    starts: &[usize],
    out: &mut Vec<ParsedTag>,
) {
    let bytes = text.as_bytes();
    let mut i = start_from;
    while i < bytes.len() {
        if offset_in_ranges(excluded, i) {
            i += 1;
            continue;
        }
        if bytes[i] == b'#' {
            let prev_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'\r' | b'(' | b'[' | b',' | b';'
                );
            if prev_ok && i + 1 < bytes.len() && is_tag_body_byte(bytes[i + 1]) {
                let mut end = i + 1;
                while end < bytes.len() && is_tag_body_byte(bytes[end]) {
                    end += 1;
                }
                let body = &text[i + 1..end];
                // Skip trailing slash/dash/underscore punctuation if any.
                let body_trimmed = body.trim_end_matches(['-', '_', '/']);
                let span_end = i + 1 + body_trimmed.len();
                if !body_trimmed.is_empty() && body_trimmed.bytes().any(|b| b.is_ascii_alphabetic())
                {
                    out.push(ParsedTag {
                        raw_tag: format!("#{body_trimmed}"),
                        normalized_value: body_trimmed.to_string(),
                        line_number: line_for_offset(starts, i),
                        in_frontmatter: false,
                        had_hash_prefix: true,
                        value_span: (i + 1, span_end),
                        prune_span: (i, span_end),
                        snippet: line_snippet_at(text, i),
                    });
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

fn is_tag_body_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/')
}

/// True when `tag` looks like an accidental CSS/design hex color (`#RGB`, `#RRGGBB`, `#RRGGBBAA`)
/// containing hex alpha digits `A-F` (never flagging numeric tags or ordinary words).
pub fn is_hex_color_false_positive(tag: &str, had_hash_prefix: bool) -> bool {
    if !had_hash_prefix {
        return false;
    }
    let hex = tag.strip_prefix('#').unwrap_or(tag);
    if !matches!(hex.len(), 3 | 6 | 8) {
        return false;
    }
    let has_digit = hex.bytes().any(|b| b.is_ascii_digit());
    let all_hex = hex.bytes().all(|b| b.is_ascii_hexdigit());
    let is_known_css_3hex_word = matches!(
        hex.to_ascii_lowercase().as_str(),
        "bad" | "bed" | "cab" | "cad" | "dab" | "dad" | "fad" | "fed" | "fee" | "ebb" | "ace" | "add" | "age" | "bee"
    );
    all_hex && has_digit && !is_known_css_3hex_word
}

/// Optimal string alignment (Damerau-Levenshtein) distance between two ASCII/UTF-8 strings.
pub fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut val = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                val = val.min(dp[i - 2][j - 2] + 1);
            }
            dp[i][j] = val;
        }
    }
    dp[n][m]
}

fn differs_only_by_digits(a: &str, b: &str) -> bool {
    let da: String = a.chars().filter(|c| c.is_ascii_digit()).collect();
    let db: String = b.chars().filter(|c| c.is_ascii_digit()).collect();
    let na: String = a.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    let nb: String = b.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    na == nb && da != db
}

pub struct ScannedNoteTags {
    pub file_id: String,
    pub file_path: String,
    pub file_name: String,
    pub sensitive: bool,
    pub tags: Vec<ParsedTag>,
}

#[derive(Clone)]
pub struct TagPatchTarget {
    pub file_id: String,
    pub span: SpanReplacement,
}

/// Clusters hex colors, casing discrepancies, and near-duplicate typo tags across notes.
pub fn analyze_tag_weeds(
    notes: &[ScannedNoteTags],
) -> (Vec<DetectedTagWeed>, HashMap<String, TagPatchTarget>) {
    let mut detections = Vec::new();
    let mut patch_map: HashMap<String, TagPatchTarget> = HashMap::new();

    // Group non-hex tags by exact lowercase value.
    #[allow(clippy::type_complexity)]
    let mut by_lower: BTreeMap<String, Vec<(&ScannedNoteTags, &ParsedTag)>> = BTreeMap::new();

    for note in notes {
        for tag in &note.tags {
            if is_hex_color_false_positive(&tag.normalized_value, tag.had_hash_prefix) {
                let issue_key = format!("tag:{}:{}:{}", note.file_id, tag.prune_span.0, tag.prune_span.1);
                let patchable = tag.in_frontmatter;
                let occ = TagOccMeta {
                    file_id: note.file_id.clone(),
                    file_path: note.file_path.clone(),
                    file_name: note.file_name.clone(),
                    line_number: tag.line_number,
                    in_frontmatter: tag.in_frontmatter,
                    raw_tag: tag.raw_tag.clone(),
                    canonical_tag: String::new(),
                    patchable,
                    sensitive: note.sensitive,
                    snippet: tag.snippet.clone(),
                    span: tag.prune_span,
                    replacement: String::new(),
                };
                if patchable {
                    patch_map.insert(
                        issue_key,
                        TagPatchTarget {
                            file_id: note.file_id.clone(),
                            span: SpanReplacement {
                                start: tag.prune_span.0,
                                end: tag.prune_span.1,
                                replacement: String::new(),
                            },
                        },
                    );
                }
                detections.push(DetectedTagWeed {
                    kind: TagWeedKind::HexColorFalsePositive,
                    canonical_tag: tag.raw_tag.clone(),
                    confidence: if patchable { 0.99 } else { 0.94 },
                    reason: if patchable {
                        format!(
                            "Hex color `{}` inside YAML frontmatter `tags:` can be safely pruned.",
                            tag.raw_tag
                        )
                    } else {
                        format!(
                            "Hex color `{}` in Markdown body triggers tag-weed parsers (wrap in backticks to preserve).",
                            tag.raw_tag
                        )
                    },
                    occurrence: occ,
                });
                continue;
            }
            by_lower
                .entry(tag.normalized_value.to_ascii_lowercase())
                .or_default()
                .push((note, tag));
        }
    }

    // 1. Detect CaseMismatch clusters where distinct casings share a lowercase key.
    for (lower_key, entries) in &by_lower {
        let mut variant_counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_, tag) in entries {
            *variant_counts.entry(tag.normalized_value.clone()).or_insert(0) += 1;
        }
        if variant_counts.len() < 2 {
            continue;
        }
        let canonical = variant_counts
            .iter()
            .max_by(|(k1, c1), (k2, c2)| {
                c1.cmp(c2)
                    .then_with(|| {
                        usize::from(k1.as_str() == lower_key.as_str())
                            .cmp(&usize::from(k2.as_str() == lower_key.as_str()))
                    })
                    .then_with(|| k2.cmp(k1))
            })
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| lower_key.clone());

        for (note, tag) in entries {
            if tag.normalized_value == canonical {
                continue;
            }
            let issue_key = format!("tag:{}:{}:{}", note.file_id, tag.value_span.0, tag.value_span.1);
            let occ = TagOccMeta {
                file_id: note.file_id.clone(),
                file_path: note.file_path.clone(),
                file_name: note.file_name.clone(),
                line_number: tag.line_number,
                in_frontmatter: tag.in_frontmatter,
                raw_tag: tag.raw_tag.clone(),
                canonical_tag: if tag.in_frontmatter {
                    canonical.clone()
                } else {
                    format!("#{canonical}")
                },
                patchable: true,
                sensitive: note.sensitive,
                snippet: tag.snippet.clone(),
                span: tag.value_span,
                replacement: canonical.clone(),
            };
            patch_map.insert(
                issue_key,
                TagPatchTarget {
                    file_id: note.file_id.clone(),
                    span: SpanReplacement {
                        start: tag.value_span.0,
                        end: tag.value_span.1,
                        replacement: canonical.clone(),
                    },
                },
            );
            detections.push(DetectedTagWeed {
                kind: TagWeedKind::CaseMismatch,
                canonical_tag: canonical.clone(),
                confidence: 0.96,
                reason: format!(
                    "Casing discrepancy `{}` -> canonical `{}`.",
                    tag.normalized_value, canonical
                ),
                occurrence: occ,
            });
        }
    }

    // 2. Detect separator variants (`_` vs `-`) & Damerau-Levenshtein distance-1 typos (`len >= 5`).
    let lower_keys: Vec<(String, usize)> = by_lower
        .iter()
        .map(|(k, v)| (k.clone(), v.len()))
        .collect();
    let mut handled_pairs: BTreeSet<(String, String)> = BTreeSet::new();

    for i in 0..lower_keys.len() {
        for j in (i + 1)..lower_keys.len() {
            let (ka, ca) = (&lower_keys[i].0, lower_keys[i].1);
            let (kb, cb) = (&lower_keys[j].0, lower_keys[j].1);
            if differs_only_by_digits(ka, kb) {
                continue;
            }
            let sep_variant = ka.replace('_', "-") == kb.replace('_', "-");
            let typo_variant = !sep_variant
                && ka.chars().count() >= 5
                && kb.chars().count() >= 5
                && ca != cb
                && ca.max(cb) >= 2
                && damerau_levenshtein(ka, kb) == 1;
            if !(sep_variant || typo_variant) {
                continue;
            }

            let (canon_lower, typo_lower) = if ca > cb
                || (ca == cb && ka.contains('-') && kb.contains('_'))
                || (ca == cb && ka < kb)
            {
                (ka.clone(), kb.clone())
            } else {
                (kb.clone(), ka.clone())
            };
            if !handled_pairs.insert((canon_lower.clone(), typo_lower.clone())) {
                continue;
            }

            if let Some(entries) = by_lower.get(&typo_lower) {
                let kind_label = if sep_variant {
                    "separator variant"
                } else {
                    "typo/spelling variant"
                };
                for (note, tag) in entries {
                    let issue_key = format!(
                        "tag:{}:{}:{}",
                        note.file_id, tag.value_span.0, tag.value_span.1
                    );
                    if patch_map.contains_key(&issue_key) {
                        continue;
                    }
                    let occ = TagOccMeta {
                        file_id: note.file_id.clone(),
                        file_path: note.file_path.clone(),
                        file_name: note.file_name.clone(),
                        line_number: tag.line_number,
                        in_frontmatter: tag.in_frontmatter,
                        raw_tag: tag.raw_tag.clone(),
                        canonical_tag: if tag.in_frontmatter {
                            canon_lower.clone()
                        } else {
                            format!("#{canon_lower}")
                        },
                        patchable: true,
                        sensitive: note.sensitive,
                        snippet: tag.snippet.clone(),
                        span: tag.value_span,
                        replacement: canon_lower.clone(),
                    };
                    patch_map.insert(
                        issue_key,
                        TagPatchTarget {
                            file_id: note.file_id.clone(),
                            span: SpanReplacement {
                                start: tag.value_span.0,
                                end: tag.value_span.1,
                                replacement: canon_lower.clone(),
                            },
                        },
                    );
                    detections.push(DetectedTagWeed {
                        kind: TagWeedKind::TypoOrVariant,
                        canonical_tag: canon_lower.clone(),
                        confidence: if sep_variant { 0.92 } else { 0.88 },
                        reason: format!(
                            "Near-duplicate {kind_label} `{}` consolidated into `{canon_lower}`.",
                            tag.normalized_value
                        ),
                        occurrence: occ,
                    });
                }
            }
        }
    }

    (detections, patch_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_frontmatter_and_inline_tags_safely() {
        let md = "\
---
title: Palette & Architecture
tags: [Architecture, \"#A3E2F1\", govrenance]
---
We follow #architecture and #year-2026 specs (#2026 stays ignored).
Accent #A3E2F1 appears in body text; `#f00bar` in inline code stays ignored.
```css
.btn { color: #A3E2F1; }
```
";
        let tags = extract_tags_from_markdown(md);
        let vals: Vec<&str> = tags.iter().map(|t| t.raw_tag.as_str()).collect();
        assert!(vals.contains(&"Architecture"));
        assert!(vals.contains(&"#A3E2F1"));
        assert!(vals.contains(&"govrenance"));
        assert!(vals.contains(&"#architecture"));
        assert!(vals.contains(&"#year-2026"));
        assert!(!vals.contains(&"#2026"));
        assert!(!vals.contains(&"#f00bar"));
        // Only two #A3E2F1 occurrences: frontmatter and non-code body (NOT CSS block).
        let hex_count = tags.iter().filter(|t| t.raw_tag == "#A3E2F1").count();
        assert_eq!(hex_count, 2);
    }

    #[test]
    fn clusters_case_typo_and_hex_colors_without_confusing_numbered_tags() {
        let n1 = "\
---
tags: [Architecture, \"#A3E2F1\", governance]
---
Tracking #phase1 and #governance across teams.
";
        let n2 = "\
See #architecture, #phase2, and #govrenance notes.
Color accent #A3E2F1.
";
        let scanned = vec![
            ScannedNoteTags {
                file_id: "f1".into(),
                file_path: r"C:\Notes\n1.md".into(),
                file_name: "n1.md".into(),
                sensitive: false,
                tags: extract_tags_from_markdown(n1),
            },
            ScannedNoteTags {
                file_id: "f2".into(),
                file_path: r"C:\Notes\n2.md".into(),
                file_name: "n2.md".into(),
                sensitive: false,
                tags: extract_tags_from_markdown(n2),
            },
        ];

        let (weeds, patches) = analyze_tag_weeds(&scanned);
        assert!(weeds
            .iter()
            .any(|w| w.kind == TagWeedKind::CaseMismatch && w.canonical_tag == "architecture"));
        assert!(weeds
            .iter()
            .any(|w| w.kind == TagWeedKind::TypoOrVariant && w.canonical_tag == "governance"));
        assert!(weeds
            .iter()
            .any(|w| w.kind == TagWeedKind::HexColorFalsePositive && w.occurrence.patchable));
        assert!(weeds
            .iter()
            .any(|w| w.kind == TagWeedKind::HexColorFalsePositive && !w.occurrence.patchable));
        // Must NEVER treat #phase1 vs #phase2 as typos.
        assert!(!weeds
            .iter()
            .any(|w| w.canonical_tag.contains("phase")));
        assert!(patches.len() >= 3);
    }
}
