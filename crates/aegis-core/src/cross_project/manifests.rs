//! Pure-Rust manifest dependency parsing (`package.json`, `Cargo.toml`, `requirements.txt`,
//! `pyproject.toml`) for Cross-Project Intelligence (`3.3`, `R9`).

use serde_json::Value;

/// One parsed package dependency requirement from a project manifest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedManifestDep {
    pub package: String,
    pub raw_version: String,
    pub major_version: Option<u64>,
}

/// Returns `true` when `filename` is a supported dependency manifest.
pub fn is_manifest_filename(filename: &str) -> bool {
    matches!(
        filename.trim().to_ascii_lowercase().as_str(),
        "package.json" | "cargo.toml" | "requirements.txt" | "pyproject.toml"
    )
}

/// Extracts `(package, raw_version, major_version)` entries from supported manifest text.
pub fn parse_manifest_deps(filename: &str, text: &str) -> Vec<ParsedManifestDep> {
    match filename.trim().to_ascii_lowercase().as_str() {
        "package.json" => parse_package_json(text),
        "cargo.toml" => parse_cargo_toml(text),
        "requirements.txt" => parse_requirements_txt(text),
        "pyproject.toml" => parse_pyproject_toml(text),
        _ => Vec::new(),
    }
}

/// Parses a semantic/PEP440 leading major version component (`^18.2.0` -> `Some(18)`).
pub fn extract_major_version(spec: &str) -> Option<u64> {
    let trimmed = spec.trim();
    if trimmed.is_empty()
        || trimmed == "*"
        || trimmed.eq_ignore_ascii_case("latest")
        || trimmed.starts_with("workspace:")
        || trimmed.starts_with("git+")
        || trimmed.starts_with("http")
        || trimmed.starts_with("file:")
    {
        return None;
    }
    // Strip comparator symbols and find the first digit run.
    let cleaned = trimmed.trim_start_matches(['^', '~', '=', '>', '<', '!', 'v', 'V', ' ']);
    let digits: String = cleaned
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn parse_package_json(text: &str) -> Vec<ParsedManifestDep> {
    let Ok(val) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(obj) = val.get(section).and_then(|v| v.as_object()) else {
            continue;
        };
        for (pkg, ver_val) in obj {
            let Some(ver) = ver_val.as_str() else {
                continue;
            };
            let norm_pkg = pkg.trim().to_ascii_lowercase();
            if norm_pkg.is_empty() {
                continue;
            }
            out.push(ParsedManifestDep {
                package: norm_pkg,
                raw_version: ver.trim().to_string(),
                major_version: extract_major_version(ver),
            });
        }
    }
    out
}

fn parse_cargo_toml(text: &str) -> Vec<ParsedManifestDep> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let hdr = line[1..line.len() - 1].trim().to_ascii_lowercase();
            in_deps = hdr == "dependencies"
                || hdr == "dev-dependencies"
                || hdr == "build-dependencies"
                || hdr == "workspace.dependencies"
                || hdr.ends_with(".dependencies");
            continue;
        }
        if !in_deps {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        let pkg = lhs
            .trim()
            .trim_matches(['"', '\''])
            .to_ascii_lowercase();
        if pkg.is_empty() || pkg.contains(' ') {
            continue;
        }
        let rhs = rhs.trim();
        let ver_opt = if let Some(inner) = rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            Some(inner.to_string())
        } else if rhs.contains("version") {
            extract_inline_toml_version(rhs)
        } else {
            None
        };
        if let Some(raw_ver) = ver_opt {
            out.push(ParsedManifestDep {
                package: pkg,
                major_version: extract_major_version(&raw_ver),
                raw_version: raw_ver,
            });
        }
    }
    out
}

fn extract_inline_toml_version(rhs: &str) -> Option<String> {
    let idx = rhs.find("version")?;
    let after = &rhs[idx + "version".len()..];
    let (_, val_part) = after.split_once('=')?;
    let val_trim = val_part.trim_start();
    let quote = if val_trim.starts_with('"') {
        '"'
    } else if val_trim.starts_with('\'') {
        '\''
    } else {
        return None;
    };
    let inner = &val_trim[1..];
    let end = inner.find(quote)?;
    Some(inner[..end].to_string())
}

fn parse_requirements_txt(text: &str) -> Vec<ParsedManifestDep> {
    let mut out = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('-') {
            continue;
        }
        if let Some(dep) = parse_pip_spec(line) {
            out.push(dep);
        }
    }
    out
}

fn parse_pyproject_toml(text: &str) -> Vec<ParsedManifestDep> {
    let mut out = Vec::new();
    let mut in_deps = false;
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let hdr = line[1..line.len() - 1].trim().to_ascii_lowercase();
            in_deps = hdr == "project" || hdr == "tool.poetry.dependencies";
            continue;
        }
        if !in_deps {
            continue;
        }
        if line.starts_with('"') || line.starts_with('\'') {
            let cleaned = line.trim_end_matches(',').trim_matches(['"', '\'']);
            if let Some(dep) = parse_pip_spec(cleaned) {
                out.push(dep);
            }
        } else if let Some((lhs, rhs)) = line.split_once('=') {
            let pkg = lhs.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
            let ver = rhs.trim().trim_matches(['"', '\'']);
            if !pkg.is_empty() && pkg != "python" && pkg != "name" && pkg != "version" && !ver.is_empty()
                && !ver.starts_with('[')
            {
                out.push(ParsedManifestDep {
                    package: pkg,
                    major_version: extract_major_version(ver),
                    raw_version: ver.to_string(),
                });
            }
        }
    }
    out
}

fn parse_pip_spec(spec: &str) -> Option<ParsedManifestDep> {
    let base = spec.split(';').next().unwrap_or("").trim();
    if base.is_empty() {
        return None;
    }
    for op in ["==", ">=", "~=", "<=", "!="] {
        if let Some((pkg_part, ver_part)) = base.split_once(op) {
            let pkg = pkg_part
                .split('[')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let ver = ver_part
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !pkg.is_empty() && !ver.is_empty() {
                return Some(ParsedManifestDep {
                    package: pkg,
                    major_version: extract_major_version(&ver),
                    raw_version: format!("{op}{ver}"),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_json_cargo_toml_and_requirements() {
        let pkg_json = r#"{"dependencies": {"react": "^18.2.0", "lucide-react": "^0.400.0"}}"#;
        let deps = parse_manifest_deps("package.json", pkg_json);
        assert!(deps.iter().any(|d| d.package == "react" && d.major_version == Some(18)));

        let cargo = r#"
[package]
name = "demo"
[dependencies]
serde = { version = "1.0.210", features = ["derive"] }
rusqlite = "0.32.1"
"#;
        let cdeps = parse_manifest_deps("Cargo.toml", cargo);
        assert!(cdeps.iter().any(|d| d.package == "serde" && d.major_version == Some(1)));
        assert!(cdeps.iter().any(|d| d.package == "rusqlite" && d.major_version == Some(0)));

        let reqs = "fastapi==0.115.0\npydantic>=2.9.0\n";
        let rdeps = parse_manifest_deps("requirements.txt", reqs);
        assert!(rdeps.iter().any(|d| d.package == "pydantic" && d.major_version == Some(2)));
    }
}
