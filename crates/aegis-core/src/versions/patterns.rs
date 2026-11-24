//! Pure-Rust filename version parsing, `Versions\` folder detection, and safe path filtering.

use std::path::Path;

const SEP: char = std::path::MAIN_SEPARATOR;

/// Normalized version metadata extracted from one filename and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersionName {
    pub canonical_stem: String,
    pub canonical_stem_lower: String,
    pub ext_group: String,
    pub version_label: String,
    pub has_explicit_version_tag: bool,
    pub version_order_hint: i64,
    pub in_versions_folder: bool,
    pub versions_subfolder_label: Option<String>,
    pub anchor_dir: String,
}

/// Normalizes file extensions into compatible document families (e.g. `doc`/`docx` -> `"doc"`).
pub fn normalize_ext_group(ext: &str) -> String {
    let clean = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match clean.as_str() {
        "doc" | "docx" => "doc".to_string(),
        "xls" | "xlsx" => "xls".to_string(),
        "ppt" | "pptx" => "ppt".to_string(),
        "md" | "markdown" => "md".to_string(),
        "tif" | "tiff" => "tiff".to_string(),
        "jpg" | "jpeg" => "jpg".to_string(),
        _ => clean,
    }
}

/// Returns true for repository boilerplate filenames whose unadorned cross-subfolder matches
/// would otherwise create noisy false version chains inside a project.
pub fn is_generic_boilerplate_stem(stem_lower: &str) -> bool {
    matches!(
        stem_lower,
        "index"
            | "mod"
            | "lib"
            | "main"
            | "init"
            | "__init__"
            | "package"
            | "tsconfig"
            | "cargo"
            | "makefile"
            | "readme"
            | "license"
            | "changelog"
            | "notes"
            | "document"
            | "untitled"
            | "app"
            | "client"
    )
}

/// Excludes internal, build, package-cache, and OS system directories AFTER stripping `mount_point`
/// so Windows `%TEMP%` paths beneath `AppData\Local\Temp` are never rejected in tests.
pub fn is_excluded_relative_path(path: &str, mount_point: &str) -> bool {
    let rel = relative_to_mount(path, mount_point);
    for seg in rel.split(['\\', '/']) {
        let low = seg.trim().to_ascii_lowercase();
        if low.is_empty() {
            continue;
        }
        if matches!(
            low.as_str(),
            ".git"
                | ".hg"
                | ".svn"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | "__pycache__"
                | ".venv"
                | "venv"
                | "site-packages"
                | ".aegis"
                | ".aegis-quarantine"
                | "$recycle.bin"
                | "system volume information"
                | "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "appdata"
                | "windowsapps"
        ) {
            return true;
        }
    }
    false
}

fn relative_to_mount<'a>(path: &'a str, mount_point: &str) -> &'a str {
    let norm_mount = mount_point.trim_end_matches(['\\', '/']);
    if !norm_mount.is_empty() && path.len() >= norm_mount.len() {
        let prefix = &path[..norm_mount.len()];
        if prefix.eq_ignore_ascii_case(norm_mount) {
            let rest = &path[norm_mount.len()..];
            if rest.is_empty() {
                return "";
            }
            if let Some(stripped) = rest.strip_prefix('\\').or_else(|| rest.strip_prefix('/')) {
                return stripped;
            }
        }
    }
    path
}

/// Splits filename into `(stem, ext_without_dot)` preserving original stem casing.
fn split_stem_ext(filename: &str, path: &str) -> (String, String) {
    let p = Path::new(filename);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = p
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .or_else(|| {
            Path::new(path)
                .extension()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or_default();
    (stem, ext)
}

fn detect_versions_anchor(path: &str, mount_point: &str) -> (bool, Option<String>, String) {
    let norm_path = path.trim();
    let mount_trimmed = mount_point.trim_end_matches(['\\', '/']);
    let rel = relative_to_mount(norm_path, mount_point);
    let mount_prefix_len = norm_path.len().saturating_sub(rel.len());

    let dir_part_rel = match rel.rfind(['\\', '/']) {
        Some(idx) => &rel[..idx],
        None => "",
    };

    if !dir_part_rel.is_empty() {
        let mut cursor = 0usize;
        for seg in dir_part_rel.split(['\\', '/']) {
            if seg.eq_ignore_ascii_case("versions") {
                let anchor_end = mount_prefix_len + cursor;
                let anchor = norm_path[..anchor_end]
                    .trim_end_matches(['\\', '/'])
                    .to_string();
                let anchor_dir = if anchor.is_empty() {
                    if mount_trimmed.is_empty() {
                        SEP.to_string()
                    } else if mount_trimmed.ends_with(':') {
                        format!("{mount_trimmed}{SEP}")
                    } else {
                        mount_trimmed.to_string()
                    }
                } else if anchor.ends_with(':') {
                    format!("{anchor}{SEP}")
                } else {
                    anchor
                };
                let after_versions_start = (cursor + seg.len()).min(dir_part_rel.len());
                let sub = dir_part_rel[after_versions_start..]
                    .trim_matches(['\\', '/'])
                    .replace('\\', "/");
                let label = if sub.is_empty() { None } else { Some(sub) };
                return (true, label, anchor_dir);
            }
            cursor += seg.len() + 1;
        }
    }

    let parent = Path::new(norm_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| mount_point.to_string());
    (false, None, parent)
}

fn alnum_count(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphanumeric()).count()
}

/// Parses a filename + path into its canonical version stem, version label, and `Versions\` anchor.
pub fn parse_version_name(filename: &str, path: &str, mount_point: &str) -> ParsedVersionName {
    let (raw_stem, raw_ext) = split_stem_ext(filename, path);
    let ext_group = normalize_ext_group(&raw_ext);
    let (in_versions_folder, versions_subfolder_label, anchor_dir) =
        detect_versions_anchor(path, mount_point);

    let mut stem = raw_stem.trim().to_string();
    let mut prefix_tags: Vec<String> = Vec::new();
    let mut suffix_tags: Vec<(String, i64)> = Vec::new();

    // Strip unmistakable OS copy/backup leading prefixes ("Copy of ...", "Copy (2) of ...", "Backup of ...").
    loop {
        if let Some((tag, rest, order)) = strip_leading_copy_prefix(&stem)
            && alnum_count(rest) >= 2
        {
            prefix_tags.push(tag);
            suffix_tags.push((String::new(), order));
            stem = rest.trim().to_string();
            continue;
        }
        break;
    }

    // Iteratively strip trailing version/stage/date/copy tokens from right to left.
    loop {
        if let Some((new_stem, tag, order)) = strip_trailing_token(&stem)
            && alnum_count(new_stem) >= 2
        {
            suffix_tags.push((tag, order));
            stem = new_stem
                .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '.'))
                .to_string();
            continue;
        }
        break;
    }

    let clean_stem = stem.trim().to_string();
    let canonical_stem = if clean_stem.is_empty() {
        raw_stem
    } else {
        clean_stem
    };
    let canonical_stem_lower = canonical_stem.to_ascii_lowercase();

    let mut label_parts: Vec<String> = Vec::new();
    let mut order_hint = 0i64;
    for tag in prefix_tags {
        if !tag.is_empty() {
            label_parts.push(tag);
        }
    }
    for (tag, order) in suffix_tags.into_iter().rev() {
        if !tag.is_empty() {
            label_parts.push(tag);
        }
        // Later (innermost) numeric versions outweigh trailing copy badges appropriately.
        order_hint = order_hint.saturating_add(order);
    }

    let has_explicit_version_tag = !label_parts.is_empty();
    let version_label = match (&versions_subfolder_label, label_parts.is_empty()) {
        (Some(sub), false) => format!("Versions/{sub} · {}", label_parts.join(" ")),
        (Some(sub), true) => format!("Versions/{sub}"),
        (None, false) => label_parts.join(" "),
        (None, true) => {
            if in_versions_folder {
                "Versions".to_string()
            } else {
                "base".to_string()
            }
        }
    };

    ParsedVersionName {
        canonical_stem,
        canonical_stem_lower,
        ext_group,
        version_label,
        has_explicit_version_tag,
        version_order_hint: order_hint,
        in_versions_folder,
        versions_subfolder_label,
        anchor_dir,
    }
}

fn strip_leading_copy_prefix(stem: &str) -> Option<(String, &str, i64)> {
    let lower = stem.to_ascii_lowercase();
    if lower.starts_with("copy of ") {
        return Some(("Copy".into(), &stem[8..], 5));
    }
    if lower.starts_with("backup of ") {
        return Some(("Backup".into(), &stem[10..], -20));
    }
    if lower.starts_with("copy (")
        && let Some(end_paren) = lower.find(") of ")
    {
        let num_str = &lower[6..end_paren];
        if let Ok(n) = num_str.trim().parse::<i64>() {
            return Some((format!("Copy ({n})"), &stem[end_paren + 5..], 5 + n));
        }
    }
    None
}

fn strip_trailing_token(stem: &str) -> Option<(&str, String, i64)> {
    let trimmed = stem.trim_end();

    // 1. Trailing parenthesized suffix: `(1)`, `(2)`, `(copy)`, `(copy 2)`, `(v2)`, `(final)`, `(draft)`.
    if trimmed.ends_with(')')
        && let Some(open) = trimmed.rfind('(')
    {
        let inner = trimmed[open + 1..trimmed.len() - 1].trim();
        let before = trimmed[..open].trim_end();
        if !before.is_empty() && !inner.is_empty() {
            if let Ok(n) = inner.parse::<i64>()
                && (1..=999).contains(&n)
            {
                return Some((before, format!("({n})"), n));
            }
            if let Some((tag, ord)) = classify_paren_tag(inner) {
                return Some((before, tag, ord));
            }
        }
    }

    // 2. Trailing Windows Explorer ` - Copy` or ` - Copy 2`.
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("- copy") || lower.ends_with(" copy") {
        let cut = if lower.ends_with("- copy") {
            trimmed.len() - 6
        } else {
            trimmed.len() - 5
        };
        let before = trimmed[..cut].trim_end();
        if !before.is_empty() {
            return Some((before, "Copy".to_string(), 5));
        }
    }

    // 3. Explicit separated token at end (`_`, `-`, `.`, space): dates, `v1`, `rev2`, `final`, `draft`, etc.
    if let Some((before, sep, token)) = split_last_separated_token(trimmed) {
        if let Some((date_tag, ord)) = parse_date_token(token, sep) {
            return Some((before, date_tag, ord));
        }
        if let Some((ver_tag, ord)) = parse_version_or_stage_token(token, sep) {
            return Some((before, ver_tag, ord));
        }
    }

    // 4. Dotted multi-part version suffix like `Spec_v1.2` or `Spec.v2.10` or `Budget-2026-03-14`.
    if let Some((before, date_tag, ord)) = strip_trailing_separated_date(trimmed) {
        return Some((before, date_tag, ord));
    }
    if let Some((before, ver_tag, ord)) = strip_trailing_dotted_semver(trimmed) {
        return Some((before, ver_tag, ord));
    }

    None
}

fn split_last_separated_token(s: &str) -> Option<(&str, char, &str)> {
    for (idx, ch) in s.char_indices().rev() {
        if matches!(ch, '_' | '-' | '.' | ' ') {
            let token = &s[idx + ch.len_utf8()..];
            let before = &s[..idx];
            if !before.is_empty() && !token.is_empty() {
                return Some((before, ch, token));
            }
            return None;
        }
    }
    None
}

fn classify_paren_tag(inner: &str) -> Option<(String, i64)> {
    let low = inner.to_ascii_lowercase();
    if low == "copy" {
        return Some(("(copy)".to_string(), 5));
    }
    if let Some(rest) = low.strip_prefix("copy ")
        && let Ok(n) = rest.trim().parse::<i64>()
    {
        return Some((format!("(copy {n})"), 5 + n));
    }
    parse_version_or_stage_token(inner, ' ').map(|(t, o)| (format!("({t})"), o))
}

fn parse_version_or_stage_token(tok: &str, sep: char) -> Option<(String, i64)> {
    let low = tok.to_ascii_lowercase();
    let trimmed = low.as_str();

    // Numeric version tokens: `v1`, `v02`, `ver3`, `rev2`, `r2`, `rc1`, `beta2`, `alpha1`, `ed2`.
    let prefixes: &[(&str, i64)] = &[
        ("ver", 1_000),
        ("rev", 1_000),
        ("alpha", 100),
        ("beta", 300),
        ("rc", 800),
        ("ed", 1_000),
        ("v", 1_000),
        ("r", 1_000),
    ];
    for &(pfx, base) in prefixes {
        if let Some(num_part) = trimmed.strip_prefix(pfx) {
            let num_part = num_part.trim_start_matches(['_', '-']);
            if !num_part.is_empty()
                && num_part.chars().all(|c| c.is_ascii_digit())
                && num_part.len() <= 4
                && let Ok(n) = num_part.parse::<i64>()
            {
                // Avoid stripping ambiguous lowercase `r2` on space separation (`Part R 2`); allow `_`/`-`/`.` or multi-char prefixes.
                if (pfx == "r" || pfx == "v") && sep == ' ' && pfx == "r" {
                    continue;
                }
                return Some((tok.to_string(), base + n * 10));
            }
        }
    }

    // Known lifecycle stage suffixes (exact word or word+digit like `final2`, `draft3`).
    let (word, num_suffix) = split_alpha_digits(trimmed);
    if let Some(n) = num_suffix
        && n > 999
    {
        return None;
    }
    let stage_ord = match word {
        "bak" | "backup" | "old" | "orig" | "original" | "archive" => -300,
        "prev" | "previous" => -200,
        "tmp" | "temp" | "wip" => -150,
        "draft" => -100,
        "copy" => 5,
        "edited" | "revised" | "updated" | "review" | "reviewed" => 2_000,
        "clean" | "signed" | "approved" => 3_000,
        "final" => 4_000,
        "new" | "latest" | "current" => 4_500,
        _ => return None,
    };
    let bump = num_suffix.unwrap_or(0);
    Some((tok.to_string(), stage_ord + bump))
}

fn split_alpha_digits(s: &str) -> (&str, Option<i64>) {
    let split_idx = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    if split_idx == s.len() {
        return (s, None);
    }
    let word = &s[..split_idx];
    let digits = &s[split_idx..];
    if digits.chars().all(|c| c.is_ascii_digit())
        && let Ok(n) = digits.parse::<i64>()
    {
        (word, Some(n))
    } else {
        (s, None)
    }
}

fn parse_date_token(tok: &str, sep: char) -> Option<(String, i64)> {
    if tok.len() == 8 && tok.chars().all(|c| c.is_ascii_digit()) && sep != ' ' {
        let y: i64 = tok[0..4].parse().ok()?;
        let m: i64 = tok[4..6].parse().ok()?;
        let d: i64 = tok[6..8].parse().ok()?;
        if (1990..=2099).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) {
            return Some((tok.to_string(), y * 10_000 + m * 100 + d));
        }
    }
    None
}

fn strip_trailing_separated_date(stem: &str) -> Option<(&str, String, i64)> {
    let bytes = stem.as_bytes();
    // Match trailing `<sep>YYYY-MM-DD`, `<sep>YYYY_MM_DD`, or `<sep>YYYY.MM.DD` (11 chars).
    if bytes.len() >= 12 {
        let split_at = bytes.len() - 10;
        let sep = stem.as_bytes()[split_at - 1];
        if matches!(sep, b'_' | b'-' | b'.' | b' ') {
            let tail = &stem[split_at..];
            let tb = tail.as_bytes();
            if matches!(tb[4], b'-' | b'_' | b'.')
                && tb[4] == tb[7]
                && let (Ok(y), Ok(m), Ok(d)) = (
                    tail[0..4].parse::<i64>(),
                    tail[5..7].parse::<i64>(),
                    tail[8..10].parse::<i64>(),
                )
                && (1990..=2099).contains(&y)
                && (1..=12).contains(&m)
                && (1..=31).contains(&d)
            {
                let before = &stem[..split_at - 1];
                if !before.is_empty() {
                    return Some((before, tail.to_string(), y * 10_000 + m * 100 + d));
                }
            }
        }
    }
    // Match trailing `<sep>YYYY-MM` or `<sep>YYYY_MM` (8 chars).
    if bytes.len() >= 9 {
        let split_at = bytes.len() - 7;
        let sep = bytes[split_at - 1];
        if matches!(sep, b'_' | b'-' | b' ') {
            let tail = &stem[split_at..];
            let tb = tail.as_bytes();
            if matches!(tb[4], b'-' | b'_')
                && let (Ok(y), Ok(m)) = (tail[0..4].parse::<i64>(), tail[5..7].parse::<i64>())
                && (1990..=2099).contains(&y)
                && (1..=12).contains(&m)
            {
                let before = &stem[..split_at - 1];
                if !before.is_empty() {
                    return Some((before, tail.to_string(), y * 10_000 + m * 100));
                }
            }
        }
    }
    None
}

fn strip_trailing_dotted_semver(stem: &str) -> Option<(&str, String, i64)> {
    let lower = stem.to_ascii_lowercase();
    // Look backwards for `_v1.2`, `-v2.10`, ` v1.2.3`, `.v1.2`
    for idx in (0..lower.len()).rev() {
        let ch = lower.as_bytes()[idx];
        if matches!(ch, b'_' | b'-' | b' ' | b'.') {
            let candidate = &stem[idx + 1..];
            let cand_low = &lower[idx + 1..];
            let rest_str = if let Some(r) = cand_low.strip_prefix("ver") {
                r
            } else if let Some(r) = cand_low.strip_prefix('v') {
                r
            } else {
                continue;
            };
            let parts: Vec<&str> = rest_str.split('.').collect();
            if parts.len() >= 2
                && parts.len() <= 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
            {
                let major: i64 = parts[0].parse().ok()?;
                let minor: i64 = parts[1].parse().ok()?;
                let patch: i64 = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
                let before = &stem[..idx];
                if !before.is_empty() {
                    return Some((
                        before,
                        candidate.to_string(),
                        1_000 + major * 1_000 + minor * 10 + patch,
                    ));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_stage_date_and_copy_patterns() {
        let p1 = parse_version_name("Proposal_v1.docx", r"C:\Work\Proposal_v1.docx", r"C:\");
        let p2 = parse_version_name(
            "Proposal_v2_FINAL (2).doc",
            r"C:\Work\Proposal_v2_FINAL (2).doc",
            r"C:\",
        );
        let p3 = parse_version_name(
            "Proposal-2026-03-14.docx",
            r"C:\Work\Proposal-2026-03-14.docx",
            r"C:\",
        );
        assert_eq!(p1.canonical_stem, "Proposal");
        assert_eq!(p1.ext_group, "doc");
        assert_eq!(p1.version_label, "v1");
        assert_eq!(p2.canonical_stem, "Proposal");
        assert_eq!(p2.ext_group, "doc");
        assert_eq!(p2.version_label, "v2 FINAL (2)");
        assert_eq!(p3.canonical_stem, "Proposal");
        assert_eq!(p3.version_label, "2026-03-14");

        let d1 = parse_version_name("Budget_v1.2.xlsx", r"C:\Work\Budget_v1.2.xlsx", r"C:\");
        let d2 = parse_version_name(
            "Copy of Budget_draft.xlsx",
            r"C:\Work\Copy of Budget_draft.xlsx",
            r"C:\",
        );
        assert_eq!(d1.canonical_stem, "Budget");
        assert_eq!(d1.version_label, "v1.2");
        assert_eq!(d2.canonical_stem, "Budget");
        assert!(d2.version_label.contains("Copy"));
        assert!(d2.version_label.contains("draft"));
    }

    #[test]
    fn does_not_strip_episodes_or_series_numbers() {
        let ep1 = parse_version_name("Lesson_01.md", r"C:\Notes\Lesson_01.md", r"C:\");
        let ep2 = parse_version_name("Lesson_02.md", r"C:\Notes\Lesson_02.md", r"C:\");
        assert_eq!(ep1.canonical_stem, "Lesson_01");
        assert_eq!(ep2.canonical_stem, "Lesson_02");
        assert!(!ep1.has_explicit_version_tag);
    }

    #[test]
    fn detects_versions_folder_anchor_and_preserves_temp_appdata_roots() {
        let v = parse_version_name(
            "Spec_v1.md",
            r"C:\Prj\Alpha\Versions\2026-03-12 Desktop\Spec_v1.md",
            r"C:\",
        );
        assert!(v.in_versions_folder);
        assert_eq!(v.anchor_dir, r"C:\Prj\Alpha");
        assert_eq!(
            v.versions_subfolder_label.as_deref(),
            Some("2026-03-12 Desktop")
        );

        let mount = r"C:\Users\Mac\AppData\Local\Temp\test123";
        assert!(!is_excluded_relative_path(
            r"C:\Users\Mac\AppData\Local\Temp\test123\Work\Spec.md",
            mount
        ));
        assert!(is_excluded_relative_path(
            r"C:\Users\Mac\AppData\Local\Temp\test123\Work\node_modules\pkg.js",
            mount
        ));
    }
}
