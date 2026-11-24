//! Pure-Rust Markdown, wikilink, embed, and reference link parser + safe path rewriting.

use super::HealLinkKind;
use std::path::{Component, Path, PathBuf};

/// Parsed internal link or asset reference inside a Markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLink {
    pub kind: HealLinkKind,
    pub line_number: usize,
    /// Full match syntax (e.g., `[Report](../old.md)` or `[[old_note|Alias]]`).
    pub raw_syntax: String,
    /// Byte span in the file corresponding solely to the replaceable path portion.
    pub target_span: (usize, usize),
    /// Decoded target path without `#anchor` or `|alias`.
    pub target_path: String,
    /// Optional heading/block anchor (`#section`).
    pub anchor: Option<String>,
    /// Optional wikilink display alias (`|alias`).
    pub alias: Option<String>,
    /// Whether the link used `<...>` delimiters in standard Markdown syntax.
    pub use_angle_brackets: bool,
}

/// Byte replacement instruction applied strictly in reverse-offset order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanReplacement {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// Computes byte start offsets for each 1-based line in `text`.
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

/// Returns the 1-based line number for `byte_offset` using precomputed `starts`.
pub fn line_for_offset(starts: &[usize], byte_offset: usize) -> usize {
    starts.partition_point(|&s| s <= byte_offset).max(1)
}

/// Byte end offset of optional YAML frontmatter (`---` at start of file through closing `---`/`...`).
pub fn frontmatter_end(text: &str) -> usize {
    let offset = if text.starts_with("\u{feff}") { 3 } else { 0 };
    let rest = &text[offset..];
    if !(rest.starts_with("---\n") || rest.starts_with("---\r\n")) {
        return 0;
    }
    let first_nl = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
    let mut pos = offset + first_nl;
    let bytes = text.as_bytes();
    let mut lines_checked = 0usize;
    while pos < bytes.len() && lines_checked < 256 {
        let nl = text[pos..].find('\n').map(|i| pos + i).unwrap_or(bytes.len());
        let line = text[pos..nl].trim_end_matches('\r').trim();
        let next_pos = if nl < bytes.len() { nl + 1 } else { bytes.len() };
        if line == "---" || line == "..." {
            return next_pos;
        }
        pos = next_pos;
        lines_checked += 1;
    }
    0
}

/// Excluded `(start, end)` byte intervals covering YAML frontmatter, fenced code blocks, and inline code.
pub fn excluded_code_ranges(text: &str, include_frontmatter: bool) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let fm_end = frontmatter_end(text);
    if include_frontmatter && fm_end > 0 {
        ranges.push((0, fm_end));
    }

    let bytes = text.as_bytes();
    let mut pos = fm_end;
    let mut active_fence: Option<(u8, usize, usize)> = None;

    while pos < bytes.len() {
        let line_end = text[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(bytes.len());
        let trimmed_start = text[pos..line_end].trim_start_matches([' ', '\t']);
        let line_trimmed = trimmed_start.trim_end_matches(['\r', '\n']);

        if let Some((fence_ch, fence_len, block_start)) = active_fence {
            let count = line_trimmed.bytes().take_while(|&b| b == fence_ch).count();
            if count >= fence_len
                && line_trimmed[count..].bytes().all(|b| b == b' ' || b == b'\t')
            {
                ranges.push((block_start, line_end));
                active_fence = None;
            }
            pos = line_end;
            continue;
        }

        let first = line_trimmed.as_bytes().first().copied();
        if let Some(ch @ (b'`' | b'~')) = first {
            let count = line_trimmed.bytes().take_while(|&b| b == ch).count();
            if count >= 3
                && (ch == b'~' || !line_trimmed[count..].contains('`'))
            {
                active_fence = Some((ch, count, pos));
                pos = line_end;
                continue;
            }
        }

        // Inline backtick code spans on non-fenced lines.
        let mut i = pos;
        while i < line_end {
            if bytes[i] == b'\\' && i + 1 < line_end {
                i += 2;
                continue;
            }
            if bytes[i] == b'`' {
                let tick_len = bytes[i..line_end].iter().take_while(|&&b| b == b'`').count();
                let span_start = i;
                let mut j = i + tick_len;
                let mut closed = None;
                while j < line_end {
                    if bytes[j] == b'`' {
                        let close_len = bytes[j..line_end].iter().take_while(|&&b| b == b'`').count();
                        if close_len == tick_len {
                            closed = Some(j + close_len);
                            break;
                        }
                        j += close_len;
                    } else {
                        j += 1;
                    }
                }
                if let Some(end) = closed {
                    ranges.push((span_start, end));
                    i = end;
                    continue;
                }
                i += tick_len;
                continue;
            }
            i += 1;
        }
        pos = line_end;
    }

    if let Some((_, _, block_start)) = active_fence {
        ranges.push((block_start, bytes.len()));
    }
    ranges.sort_by_key(|r| r.0);
    ranges
}

pub fn offset_in_ranges(ranges: &[(usize, usize)], offset: usize) -> bool {
    let idx = ranges.partition_point(|r| r.0 <= offset);
    if idx == 0 {
        return false;
    }
    let (s, e) = ranges[idx - 1];
    offset >= s && offset < e
}

fn is_external_or_ignored_target(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
        return true;
    }
    if let Some(colon) = t.find(':') {
        let scheme = &t[..colon];
        // Windows drive letters (`C:\...`) have len == 1 and must NOT be treated as URL schemes.
        if scheme.len() >= 2
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            return true;
        }
    }
    false
}

/// Decodes `%XX` UTF-8 sequences in Markdown link destinations.
pub fn percent_decode_target(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Encodes whitespace as `%20` for standard Markdown targets unless angle brackets are used.
pub fn encode_markdown_target(rel_posix: &str, use_angle_brackets: bool) -> String {
    if use_angle_brackets {
        rel_posix.to_string()
    } else {
        rel_posix.replace(' ', "%20")
    }
}

/// Computes a clean forward-slash relative path from `source_file` to `target_file`.
pub fn compute_relative_markdown_target(source_file: &Path, target_file: &Path) -> Option<String> {
    let from_dir = source_file.parent()?;
    let norm = |p: &Path| -> PathBuf {
        let raw = p.to_string_lossy();
        let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
        PathBuf::from(stripped)
    };
    let a = norm(from_dir);
    let b = norm(target_file);
    let a_comps: Vec<Component<'_>> = a.components().collect();
    let b_comps: Vec<Component<'_>> = b.components().collect();

    let mut common = 0usize;
    while common < a_comps.len() && common < b_comps.len() {
        let eq = match (&a_comps[common], &b_comps[common]) {
            (Component::Normal(sa), Component::Normal(sb)) => {
                sa.to_string_lossy().eq_ignore_ascii_case(&sb.to_string_lossy())
            }
            (Component::Prefix(pa), Component::Prefix(pb)) => pa
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&pb.as_os_str().to_string_lossy()),
            _ => a_comps[common] == b_comps[common],
        };
        if !eq {
            break;
        }
        common += 1;
    }

    if common == 0 && (matches!(a_comps.first(), Some(Component::Prefix(_))) || matches!(b_comps.first(), Some(Component::Prefix(_)))) {
        return None;
    }

    let mut rel_parts = Vec::new();
    for comp in &a_comps[common..] {
        if matches!(comp, Component::Normal(_)) {
            rel_parts.push("..".to_string());
        }
    }
    for comp in &b_comps[common..] {
        if let Component::Normal(s) = comp {
            rel_parts.push(s.to_string_lossy().to_string());
        }
    }
    if rel_parts.is_empty() {
        target_file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
    } else {
        Some(rel_parts.join("/"))
    }
}

/// Applies non-overlapping byte-offset replacements cleanly in reverse offset order.
pub fn apply_span_replacements(content: &str, mut edits: Vec<SpanReplacement>) -> String {
    edits.sort_by(|a, b| b.start.cmp(&a.start).then_with(|| b.end.cmp(&a.end)));
    let mut out = content.to_string();
    let mut last_start = usize::MAX;
    for edit in edits {
        if edit.start > edit.end || edit.end > out.len() || edit.end > last_start {
            continue;
        }
        out.replace_range(edit.start..edit.end, &edit.replacement);
        last_start = edit.start;
    }
    out
}

/// Builds a replacement full-syntax string for UI diffs & previews without rewriting file bytes.
pub fn format_healed_link_preview(link: &ParsedLink, replacement_target: &str) -> String {
    match link.kind {
        HealLinkKind::Wikilink | HealLinkKind::WikiEmbed => {
            let prefix = if link.kind == HealLinkKind::WikiEmbed {
                "![["
            } else {
                "[["
            };
            let anchor = link.anchor.as_deref().unwrap_or("");
            let alias = link.alias.as_deref().map(|a| format!("|{a}")).unwrap_or_default();
            format!("{prefix}{replacement_target}{anchor}{alias}]]")
        }
        HealLinkKind::MarkdownLink | HealLinkKind::MarkdownImage => {
            let mut s = link.raw_syntax.clone();
            if let Some(paren_open) = s.find("](") {
                let anchor = link.anchor.as_deref().unwrap_or("");
                let dest = if link.use_angle_brackets {
                    format!("<{replacement_target}{anchor}>")
                } else {
                    format!("{replacement_target}{anchor}")
                };
                if let Some(close) = s[paren_open + 2..].rfind(')') {
                    let tail_raw = s[paren_open + 2..paren_open + 2 + close].trim_start();
                    let title_suffix = tail_raw
                        .find(|c: char| c.is_ascii_whitespace())
                        .filter(|_| !link.use_angle_brackets)
                        .map(|idx| &tail_raw[idx..])
                        .unwrap_or("");
                    s.replace_range(paren_open + 2..paren_open + 2 + close, &format!("{dest}{title_suffix}"));
                }
            }
            s
        }
        HealLinkKind::ReferenceDef => {
            let anchor = link.anchor.as_deref().unwrap_or("");
            format!("{replacement_target}{anchor}")
        }
    }
}

/// Parses all internal Markdown links, images, wikilinks, embeds, and reference definitions.
pub fn parse_markdown_links(text: &str) -> Vec<ParsedLink> {
    let ranges = excluded_code_ranges(text, true);
    let starts = line_starts(text);
    let bytes = text.as_bytes();
    let mut out = Vec::new();

    // 1. Parse reference link definitions (`[id]: target`).
    let mut pos = frontmatter_end(text);
    while pos < bytes.len() {
        let line_end = text[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(bytes.len());
        let next_pos = if line_end < bytes.len() {
            line_end + 1
        } else {
            bytes.len()
        };
        if !offset_in_ranges(&ranges, pos)
            && let Some(parsed) = parse_reference_def_line(text, pos, line_end, &starts)
        {
            out.push(parsed);
        }
        pos = next_pos;
    }

    // 2. Scan inline `[[...]]`, `![[...]]`, `[...](...)`, `![...](...)`.
    let mut i = frontmatter_end(text);
    while i < bytes.len() {
        if offset_in_ranges(&ranges, i) {
            i += 1;
            continue;
        }
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(b"![[") || bytes[i..].starts_with(b"[[") {
            let is_embed = bytes[i] == b'!';
            let start = i;
            let inner_start = i + if is_embed { 3 } else { 2 };
            if let Some(rel_close) = find_closing_double_bracket(text, inner_start) {
                let inner_end = inner_start + rel_close;
                if let Some(parsed) =
                    build_wikilink(text, start, inner_start, inner_end, is_embed, &starts)
                {
                    out.push(parsed);
                }
                i = inner_end + 2;
                continue;
            }
        }
        if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some((end, parsed)) = parse_inline_md_link(text, i, true, &starts) {
                out.push(parsed);
                i = end;
                continue;
            }
        } else if bytes[i] == b'['
            && (i == 0 || bytes[i - 1] != b'!')
            && let Some((end, parsed)) = parse_inline_md_link(text, i, false, &starts)
        {
            out.push(parsed);
            i = end;
            continue;
        }
        i += 1;
    }

    out.sort_by_key(|p| p.target_span.0);
    out
}

fn find_closing_double_bracket(text: &str, inner_start: usize) -> Option<usize> {
    let slice = &text[inner_start..];
    for (idx, b) in slice.bytes().enumerate() {
        if b == b'\n' || b == b'\r' {
            return None;
        }
        if slice[idx..].starts_with("]]") {
            return Some(idx);
        }
    }
    None
}

fn build_wikilink(
    text: &str,
    full_start: usize,
    inner_start: usize,
    inner_end: usize,
    is_embed: bool,
    starts: &[usize],
) -> Option<ParsedLink> {
    let raw_inner = &text[inner_start..inner_end];
    let pipe_rel = raw_inner.find('|');
    let before_pipe_end = pipe_rel.map(|p| inner_start + p).unwrap_or(inner_end);
    let alias = pipe_rel.map(|p| raw_inner[p + 1..].to_string());

    let before_pipe = &text[inner_start..before_pipe_end];
    let hash_rel = before_pipe.find('#');
    let target_end_raw = hash_rel.map(|h| inner_start + h).unwrap_or(before_pipe_end);
    let anchor = hash_rel.map(|h| before_pipe[h..].to_string());

    let slice = &text[inner_start..target_end_raw];
    let lead_trim = slice.len() - slice.trim_start().len();
    let trail_trim = slice.len() - slice.trim_end().len();
    let span_start = inner_start + lead_trim;
    let span_end = target_end_raw.saturating_sub(trail_trim);
    if span_start >= span_end {
        return None;
    }
    let target = text[span_start..span_end].trim().to_string();
    if is_external_or_ignored_target(&target) {
        return None;
    }

    Some(ParsedLink {
        kind: if is_embed {
            HealLinkKind::WikiEmbed
        } else {
            HealLinkKind::Wikilink
        },
        line_number: line_for_offset(starts, full_start),
        raw_syntax: text[full_start..inner_end + 2].to_string(),
        target_span: (span_start, span_end),
        target_path: target,
        anchor,
        alias,
        use_angle_brackets: false,
    })
}

fn parse_inline_md_link(
    text: &str,
    start: usize,
    is_image: bool,
    starts: &[usize],
) -> Option<(usize, ParsedLink)> {
    let bytes = text.as_bytes();
    let mut j = start + if is_image { 2 } else { 1 };
    let mut bracket_depth = 1usize;
    while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
            continue;
        }
        if bytes[j] == b'\n' {
            return None;
        }
        if bytes[j] == b'[' {
            bracket_depth += 1;
        } else if bytes[j] == b']' {
            bracket_depth -= 1;
            if bracket_depth == 0 {
                break;
            }
        }
        j += 1;
    }
    if bracket_depth != 0 || j + 1 >= bytes.len() || bytes[j + 1] != b'(' {
        return None;
    }
    let mut k = j + 2;
    while k < bytes.len() && matches!(bytes[k], b' ' | b'\t') {
        k += 1;
    }
    if k >= bytes.len() {
        return None;
    }

    let (span_start, raw_dest_end, close_paren, use_angle) = if bytes[k] == b'<' {
        let mut p = k + 1;
        while p < bytes.len() && bytes[p] != b'>' && bytes[p] != b'\n' {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b'>' {
            return None;
        }
        let mut c = p + 1;
        while c < bytes.len() && bytes[c] != b')' && bytes[c] != b'\n' {
            c += 1;
        }
        if c >= bytes.len() || bytes[c] != b')' {
            return None;
        }
        (k + 1, p, c + 1, true)
    } else {
        let mut p = k;
        let mut paren_depth = 0usize;
        while p < bytes.len() && bytes[p] != b'\n' {
            let b = bytes[p];
            if b == b'\\' && p + 1 < bytes.len() {
                p += 2;
                continue;
            }
            if b == b'(' {
                paren_depth += 1;
            } else if b == b')' {
                if paren_depth == 0 {
                    break;
                }
                paren_depth -= 1;
            } else if matches!(b, b' ' | b'\t') {
                break;
            }
            p += 1;
        }
        let dest_end = p;
        let mut c = p;
        while c < bytes.len() && bytes[c] != b'\n' {
            if bytes[c] == b')' && paren_depth == 0 {
                break;
            }
            c += 1;
        }
        if c >= bytes.len() || bytes[c] != b')' {
            return None;
        }
        (k, dest_end, c + 1, false)
    };

    if span_start >= raw_dest_end {
        return None;
    }
    let full_dest = &text[span_start..raw_dest_end];
    let (target_rel_end, anchor) = match full_dest.find('#') {
        Some(h) => (span_start + h, Some(full_dest[h..].to_string())),
        None => (raw_dest_end, None),
    };
    if span_start >= target_rel_end {
        return None;
    }
    let raw_target = &text[span_start..target_rel_end];
    if is_external_or_ignored_target(raw_target) {
        return None;
    }

    Some((
        close_paren,
        ParsedLink {
            kind: if is_image {
                HealLinkKind::MarkdownImage
            } else {
                HealLinkKind::MarkdownLink
            },
            line_number: line_for_offset(starts, start),
            raw_syntax: text[start..close_paren].to_string(),
            target_span: (span_start, target_rel_end),
            target_path: percent_decode_target(raw_target),
            anchor,
            alias: None,
            use_angle_brackets: use_angle,
        },
    ))
}

fn parse_reference_def_line(
    text: &str,
    pos: usize,
    line_end: usize,
    starts: &[usize],
) -> Option<ParsedLink> {
    let line = text[pos..line_end].trim_end_matches('\r');
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    if !trimmed.starts_with('[') || trimmed.starts_with("[^") {
        return None;
    }
    let close_bracket = trimmed.find("]:")?;
    if close_bracket <= 1 {
        return None;
    }
    let after_colon_rel = close_bracket + 2;
    let rest = &trimmed[after_colon_rel..];
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let target_start = pos + indent + after_colon_rel + ws;
    if target_start >= pos + line.len() {
        return None;
    }

    let bytes = text.as_bytes();
    let (span_start, raw_end, use_angle) = if bytes[target_start] == b'<' {
        let end_rel = text[target_start + 1..pos + line.len()].find('>')?;
        (target_start + 1, target_start + 1 + end_rel, true)
    } else {
        let mut p = target_start;
        while p < pos + line.len() && !matches!(bytes[p], b' ' | b'\t') {
            p += 1;
        }
        (target_start, p, false)
    };
    if span_start >= raw_end {
        return None;
    }
    let full_dest = &text[span_start..raw_end];
    let (target_rel_end, anchor) = match full_dest.find('#') {
        Some(h) => (span_start + h, Some(full_dest[h..].to_string())),
        None => (raw_end, None),
    };
    if span_start >= target_rel_end {
        return None;
    }
    let raw_target = &text[span_start..target_rel_end];
    if is_external_or_ignored_target(raw_target) {
        return None;
    }

    Some(ParsedLink {
        kind: HealLinkKind::ReferenceDef,
        line_number: line_for_offset(starts, pos),
        raw_syntax: line.trim().to_string(),
        target_span: (span_start, target_rel_end),
        target_path: percent_decode_target(raw_target),
        anchor,
        alias: None,
        use_angle_brackets: use_angle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_wikilinks_embeds_refs_and_skips_code_and_urls() {
        let md = "\
---
title: Architecture
tags: [a, b]
---
See [Design Doc](../docs/spec%20v1.md#Overview) and ![Chart](assets/chart.png).
Also check [[Budget Q4|Q4 Budget]] and ![[diagrams/arch.svg#top]].
[spec]: ./docs/ref_guide.md#intro
External [Web](https://example.com/a.md) and anchor [Top](#top) stay ignored.
```markdown
Inside code [Fake](missing.md) and [[fake-wiki]] are ignored.
```
Inline `[InlineFake](nope.md)` is also ignored.
";
        let links = parse_markdown_links(md);
        assert_eq!(links.len(), 5, "got: {links:#?}");
        assert_eq!(links[0].kind, HealLinkKind::MarkdownLink);
        assert_eq!(links[0].target_path, "../docs/spec v1.md");
        assert_eq!(links[0].anchor.as_deref(), Some("#Overview"));

        assert_eq!(links[1].kind, HealLinkKind::MarkdownImage);
        assert_eq!(links[1].target_path, "assets/chart.png");

        assert_eq!(links[2].kind, HealLinkKind::Wikilink);
        assert_eq!(links[2].target_path, "Budget Q4");
        assert_eq!(links[2].alias.as_deref(), Some("Q4 Budget"));

        assert_eq!(links[3].kind, HealLinkKind::WikiEmbed);
        assert_eq!(links[3].target_path, "diagrams/arch.svg");
        assert_eq!(links[3].anchor.as_deref(), Some("#top"));

        assert_eq!(links[4].kind, HealLinkKind::ReferenceDef);
        assert_eq!(links[4].target_path, "./docs/ref_guide.md");
    }

    #[test]
    fn relative_path_and_reverse_span_replacements_stay_exact() {
        let src = Path::new(r"C:\Vault\Projects\Alpha\README.md");
        let dst = Path::new(r"C:\Vault\Archives\2026\Q4 Spec.md");
        let rel = compute_relative_markdown_target(src, dst).unwrap();
        assert_eq!(rel, "../../Archives/2026/Q4 Spec.md");
        assert_eq!(
            encode_markdown_target(&rel, false),
            "../../Archives/2026/Q4%20Spec.md"
        );

        let content = "A [Doc](old1.md) and B [[old2|Alias]].";
        let parsed = parse_markdown_links(content);
        let edits = vec![
            SpanReplacement {
                start: parsed[0].target_span.0,
                end: parsed[0].target_span.1,
                replacement: "new1.md".into(),
            },
            SpanReplacement {
                start: parsed[1].target_span.0,
                end: parsed[1].target_span.1,
                replacement: "new2".into(),
            },
        ];
        assert_eq!(
            apply_span_replacements(content, edits),
            "A [Doc](new1.md) and B [[new2|Alias]]."
        );
    }
}
