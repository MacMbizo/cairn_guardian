//! Format coverage (80+ creative, visual, 3D, font, video, audio, and document formats) and
//! pure-Rust header-only dimension/color extraction (`K3`, `R9` — no full image decode or external tools).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// High-level creative / digital asset family shown in Forgotten Assets filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssetFamily {
    DesignSource,
    RawPhoto,
    Image,
    ThreeD,
    Video,
    Audio,
    Typography,
    Document,
}

impl AssetFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DesignSource => "design_source",
            Self::RawPhoto => "raw_photo",
            Self::Image => "image",
            Self::ThreeD => "three_d",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Typography => "typography",
            Self::Document => "document",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "design_source" => Some(Self::DesignSource),
            "raw_photo" => Some(Self::RawPhoto),
            "image" => Some(Self::Image),
            "three_d" => Some(Self::ThreeD),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "typography" => Some(Self::Typography),
            "document" => Some(Self::Document),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DesignSource => "Design & Vector",
            Self::RawPhoto => "RAW Photography",
            Self::Image => "Images & Renders",
            Self::ThreeD => "3D & CAD",
            Self::Video => "Video & Motion",
            Self::Audio => "Audio & Stems",
            Self::Typography => "Fonts & Type",
            Self::Document => "Documents & Decks",
        }
    }

    pub fn priority(self) -> u8 {
        match self {
            Self::DesignSource => 8,
            Self::RawPhoto => 7,
            Self::ThreeD => 6,
            Self::Video => 5,
            Self::Typography => 4,
            Self::Image => 3,
            Self::Audio => 2,
            Self::Document => 1,
        }
    }

    /// High-value source assets are retained regardless of small byte size (e.g., SVG/fonts).
    pub fn is_always_kept_small(self) -> bool {
        matches!(
            self,
            Self::DesignSource | Self::RawPhoto | Self::ThreeD | Self::Typography
        )
    }
}

/// Maps 80+ file extensions onto creative and knowledge-work asset families.
pub fn classify_asset_family(ext: &str) -> Option<AssetFamily> {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match e.as_str() {
        // Design source & vector (17)
        "psd" | "psb" | "ai" | "eps" | "svg" | "svgz" | "fig" | "sketch" | "xd" | "indd"
        | "indt" | "idml" | "cdr" | "afdesign" | "afphoto" | "afpub" | "xcf" | "kra" | "dwg"
        | "dxf" => Some(AssetFamily::DesignSource),

        // Camera RAW formats (13)
        "cr2" | "cr3" | "nef" | "nrw" | "arw" | "dng" | "orf" | "rw2" | "raf" | "raw" | "pef"
        | "srw" | "3fr" => Some(AssetFamily::RawPhoto),

        // Raster images & HDR renders (14)
        "jpg" | "jpeg" | "png" | "heic" | "heif" | "tif" | "tiff" | "webp" | "avif" | "bmp"
        | "tga" | "exr" | "hdr" | "jxl" | "gif" => Some(AssetFamily::Image),

        // 3D models & CAD assemblies (18)
        "blend" | "obj" | "fbx" | "stl" | "glb" | "gltf" | "c4d" | "3ds" | "max" | "ma" | "mb"
        | "dae" | "usd" | "usdz" | "skp" | "step" | "stp" | "igs" => Some(AssetFamily::ThreeD),

        // Video & NLE project files (12)
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "m4v" | "webm" | "mts" | "prproj" | "aep"
        | "drp" | "fcpxml" => Some(AssetFamily::Video),

        // Audio & DAW projects (12)
        "wav" | "flac" | "aiff" | "aif" | "mp3" | "m4a" | "ogg" | "opus" | "aac" | "als" | "flp"
        | "logicx" | "rpp" | "mid" => Some(AssetFamily::Audio),

        // Typography (5)
        "ttf" | "otf" | "woff" | "woff2" | "ttc" => Some(AssetFamily::Typography),

        // Knowledge documents & decks (19)
        "pdf" | "docx" | "doc" | "pptx" | "ppt" | "xlsx" | "xlsm" | "xls" | "odt" | "odp"
        | "ods" | "rtf" | "md" | "epub" | "pages" | "key" | "numbers" | "tex" | "ipynb" => {
            Some(AssetFamily::Document)
        }

        _ => None,
    }
}

/// Visual dimensions and color accent read strictly from file headers (at most 64 KiB read).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisualHeader {
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub aspect_label: Option<String>,
    pub dominant_color: Option<String>,
}

/// Formats a human-friendly aspect ratio label (`16:9`, `4:3`, `Square`, `Portrait`, etc.).
pub fn aspect_label_for(width: i64, height: i64) -> Option<String> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let ratio = width as f64 / height as f64;
    if (ratio - 1.0).abs() < 0.06 {
        return Some("Square (1:1)".to_string());
    }
    let known = [
        (16.0 / 9.0, "16:9"),
        (16.0 / 10.0, "16:10"),
        (3.0 / 2.0, "3:2"),
        (4.0 / 3.0, "4:3"),
        (21.0 / 9.0, "21:9"),
        (9.0 / 16.0, "9:16"),
        (2.0 / 3.0, "2:3"),
        (3.0 / 4.0, "3:4"),
        (4.0 / 5.0, "4:5"),
    ];
    for (target, label) in known {
        if (ratio - target).abs() < 0.04 {
            return Some(label.to_string());
        }
    }
    Some(if ratio > 1.0 {
        format!("Landscape ({ratio:.2}:1)")
    } else {
        format!("Portrait ({:.2}:1)", 1.0 / ratio)
    })
}

/// Reads lightweight visual dimensions and optional SVG dominant hex accent without full decoding.
pub fn inspect_visual_header(path: &Path) -> Option<VisualHeader> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    let mut file = File::open(path).ok()?;
    let cap = if ext == "svg" { 65_536usize } else { 32_768usize };
    let mut buf = vec![0u8; cap];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.is_empty() {
        return None;
    }

    let mut out = match ext.as_str() {
        "png" => parse_png(&buf),
        "jpg" | "jpeg" => parse_jpeg(&buf),
        "gif" => parse_gif(&buf),
        "bmp" => parse_bmp(&buf),
        "webp" => parse_webp(&buf),
        "psd" | "psb" => parse_psd(&buf),
        "svg" => parse_svg(&buf),
        _ => None,
    }?;

    if let (Some(w), Some(h)) = (out.width, out.height)
        && out.aspect_label.is_none()
    {
        out.aspect_label = aspect_label_for(w, h);
    }
    Some(out)
}

fn parse_png(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 24 || &buf[0..8] != b"\x89PNG\r\n\x1a\n" || &buf[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]) as i64;
    let h = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]) as i64;
    (w > 0 && h > 0).then_some(VisualHeader {
        width: Some(w),
        height: Some(h),
        ..Default::default()
    })
}

fn parse_gif(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 10 || (&buf[0..6] != b"GIF87a" && &buf[0..6] != b"GIF89a") {
        return None;
    }
    let w = u16::from_le_bytes([buf[6], buf[7]]) as i64;
    let h = u16::from_le_bytes([buf[8], buf[9]]) as i64;
    (w > 0 && h > 0).then_some(VisualHeader {
        width: Some(w),
        height: Some(h),
        ..Default::default()
    })
}

fn parse_bmp(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 26 || &buf[0..2] != b"BM" {
        return None;
    }
    let w = i32::from_le_bytes([buf[18], buf[19], buf[20], buf[21]]).abs() as i64;
    let h = i32::from_le_bytes([buf[22], buf[23], buf[24], buf[25]]).abs() as i64;
    (w > 0 && h > 0).then_some(VisualHeader {
        width: Some(w),
        height: Some(h),
        ..Default::default()
    })
}

fn parse_psd(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 22 || &buf[0..4] != b"8BPS" {
        return None;
    }
    let h = u32::from_be_bytes([buf[14], buf[15], buf[16], buf[17]]) as i64;
    let w = u32::from_be_bytes([buf[18], buf[19], buf[20], buf[21]]) as i64;
    (w > 0 && h > 0).then_some(VisualHeader {
        width: Some(w),
        height: Some(h),
        ..Default::default()
    })
}

fn parse_webp(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 30 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WEBP" {
        return None;
    }
    let (w, h) = match &buf[12..16] {
        b"VP8 " => {
            if buf.len() < 30 {
                return None;
            }
            let w = (u16::from_le_bytes([buf[26], buf[27]]) & 0x3FFF) as i64;
            let h = (u16::from_le_bytes([buf[28], buf[29]]) & 0x3FFF) as i64;
            (w, h)
        }
        b"VP8L" => {
            if buf.len() < 25 {
                return None;
            }
            let bits = u32::from_le_bytes([buf[21], buf[22], buf[23], buf[24]]);
            let w = ((bits & 0x3FFF) + 1) as i64;
            let h = (((bits >> 14) & 0x3FFF) + 1) as i64;
            (w, h)
        }
        b"VP8X" => {
            if buf.len() < 30 {
                return None;
            }
            let w = ((u32::from_le_bytes([buf[24], buf[25], buf[26], 0])) + 1) as i64;
            let h = ((u32::from_le_bytes([buf[27], buf[28], buf[29], 0])) + 1) as i64;
            (w, h)
        }
        _ => return None,
    };
    (w > 0 && h > 0).then_some(VisualHeader {
        width: Some(w),
        height: Some(h),
        ..Default::default()
    })
}

fn parse_jpeg(buf: &[u8]) -> Option<VisualHeader> {
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < buf.len() {
        while i < buf.len() && buf[i] != 0xFF {
            i += 1;
        }
        while i < buf.len() && buf[i] == 0xFF {
            i += 1;
        }
        if i >= buf.len() {
            break;
        }
        let marker = buf[i];
        i += 1;
        // Standalone markers (TEM, RST, SOI, EOI, SOS)
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            continue;
        }
        if marker == 0xDA {
            break;
        }
        if i + 1 >= buf.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([buf[i], buf[i + 1]]) as usize;
        if seg_len < 2 || i + seg_len > buf.len() {
            break;
        }
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof && seg_len >= 7 {
            let h = u16::from_be_bytes([buf[i + 3], buf[i + 4]]) as i64;
            let w = u16::from_be_bytes([buf[i + 5], buf[i + 6]]) as i64;
            if w > 0 && h > 0 {
                return Some(VisualHeader {
                    width: Some(w),
                    height: Some(h),
                    ..Default::default()
                });
            }
        }
        i += seg_len;
    }
    None
}

fn parse_svg(buf: &[u8]) -> Option<VisualHeader> {
    let text = String::from_utf8_lossy(buf);
    if !text.contains("<svg") {
        return None;
    }
    let mut width = extract_svg_dim(&text, "width");
    let mut height = extract_svg_dim(&text, "height");
    if (width.is_none() || height.is_none())
        && let Some((vw, vh)) = extract_svg_viewbox(&text)
    {
        width = width.or(Some(vw));
        height = height.or(Some(vh));
    }
    let dominant_color = extract_first_svg_hex_color(&text);
    if width.is_none() && height.is_none() && dominant_color.is_none() {
        return None;
    }
    Some(VisualHeader {
        width,
        height,
        aspect_label: width.zip(height).and_then(|(w, h)| aspect_label_for(w, h)),
        dominant_color,
    })
}

fn extract_svg_viewbox(text: &str) -> Option<(i64, i64)> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find("viewbox")?;
    let after = &text[idx..];
    let q1 = after.find(['"', '\''])?;
    let rest = &after[q1 + 1..];
    let q2 = rest.find(['"', '\''])?;
    let nums: Vec<f64> = rest[..q2]
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();
    if nums.len() >= 4 && nums[2] > 0.0 && nums[3] > 0.0 {
        Some((nums[2].round() as i64, nums[3].round() as i64))
    } else {
        None
    }
}

fn extract_svg_dim(text: &str, attr: &str) -> Option<i64> {
    let tag_end = text.find('>')?;
    let header = &text[..tag_end];
    let needle = format!("{attr}=");
    let lower = header.to_ascii_lowercase();
    let pos = lower.find(&needle)?;
    let rest = &header[pos + needle.len()..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let inner = rest[1..].split(quote).next()?.trim();
    let num_str: String = inner
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let val = num_str.parse::<f64>().ok()?;
    (val > 0.0).then_some(val.round() as i64)
}

fn extract_first_svg_hex_color(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut fallback_hex = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 7 <= bytes.len() {
            let slice = &text[i + 1..i + 7];
            if slice.bytes().all(|b| b.is_ascii_hexdigit()) {
                let hex = format!("#{}", slice.to_ascii_uppercase());
                if !matches!(hex.as_str(), "#000000" | "#FFFFFF") {
                    return Some(hex);
                }
                fallback_hex.get_or_insert(hex);
                i += 7;
                continue;
            }
        }
        i += 1;
    }
    fallback_hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn classifies_80_plus_formats_cleanly() {
        assert_eq!(classify_asset_family("psd"), Some(AssetFamily::DesignSource));
        assert_eq!(classify_asset_family(".NEF"), Some(AssetFamily::RawPhoto));
        assert_eq!(classify_asset_family("blend"), Some(AssetFamily::ThreeD));
        assert_eq!(classify_asset_family("woff2"), Some(AssetFamily::Typography));
        assert_eq!(classify_asset_family("prproj"), Some(AssetFamily::Video));
        assert_eq!(classify_asset_family("flac"), Some(AssetFamily::Audio));
        assert_eq!(classify_asset_family("pptx"), Some(AssetFamily::Document));
        assert_eq!(classify_asset_family("webp"), Some(AssetFamily::Image));
        assert_eq!(classify_asset_family("dll"), None);
    }

    #[test]
    fn parses_png_jpeg_gif_bmp_psd_and_svg_headers() {
        let tmp = TempDir::new().unwrap();

        let png = tmp.path().join("hero.png");
        let mut png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png_bytes.extend_from_slice(&1920u32.to_be_bytes());
        png_bytes.extend_from_slice(&1080u32.to_be_bytes());
        png_bytes.resize(32, 0);
        File::create(&png).unwrap().write_all(&png_bytes).unwrap();
        let h_png = inspect_visual_header(&png).unwrap();
        assert_eq!((h_png.width, h_png.height), (Some(1920), Some(1080)));
        assert_eq!(h_png.aspect_label.as_deref(), Some("16:9"));

        let jpg = tmp.path().join("shot.jpg");
        let jpg_bytes = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00,
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x04, 0xB0, 0x06, 0x40, 0x03, 0x01, 0x11, 0x00, 0xFF, 0xD9,
        ];
        File::create(&jpg).unwrap().write_all(&jpg_bytes).unwrap();
        let h_jpg = inspect_visual_header(&jpg).unwrap();
        assert_eq!((h_jpg.width, h_jpg.height), (Some(1600), Some(1200)));
        assert_eq!(h_jpg.aspect_label.as_deref(), Some("4:3"));

        let gif = tmp.path().join("anim.gif");
        let mut gif_bytes = b"GIF89a".to_vec();
        gif_bytes.extend_from_slice(&512u16.to_le_bytes());
        gif_bytes.extend_from_slice(&512u16.to_le_bytes());
        File::create(&gif).unwrap().write_all(&gif_bytes).unwrap();
        let h_gif = inspect_visual_header(&gif).unwrap();
        assert_eq!(h_gif.aspect_label.as_deref(), Some("Square (1:1)"));

        let psd = tmp.path().join("label.psd");
        let mut psd_bytes = b"8BPS\x00\x01\x00\x00\x00\x00\x00\x00\x00\x03".to_vec();
        psd_bytes.extend_from_slice(&2000u32.to_be_bytes());
        psd_bytes.extend_from_slice(&3000u32.to_be_bytes());
        File::create(&psd).unwrap().write_all(&psd_bytes).unwrap();
        let h_psd = inspect_visual_header(&psd).unwrap();
        assert_eq!((h_psd.width, h_psd.height), (Some(3000), Some(2000)));
        assert_eq!(h_psd.aspect_label.as_deref(), Some("3:2"));

        let svg = tmp.path().join("logo.svg");
        std::fs::write(
            &svg,
            r##"<svg viewBox="0 0 800 600"><rect fill="#FFFFFF"/><path fill="#10B981"/></svg>"##,
        )
        .unwrap();
        let h_svg = inspect_visual_header(&svg).unwrap();
        assert_eq!((h_svg.width, h_svg.height), (Some(800), Some(600)));
        assert_eq!(h_svg.dominant_color.as_deref(), Some("#10B981"));
    }
}
