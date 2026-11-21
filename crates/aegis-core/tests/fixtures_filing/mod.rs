//! Builds tiny but real documents, photos and songs for the filing tests.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

fn zip_file(path: &Path, entries: &[(&str, String)]) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(body.as_bytes()).unwrap();
    }
    z.finish().unwrap();
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A minimal Word document whose body is `text` (one paragraph per line).
pub fn docx(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    let paras: String = text
        .lines()
        .map(|l| format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(l)))
        .collect();
    zip_file(
        &p,
        &[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string(),
            ),
            (
                "word/document.xml",
                format!(
                    r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{paras}</w:body></w:document>"#
                ),
            ),
        ],
    );
    p
}

/// A minimal Excel workbook with one sheet holding `cells` in column A (inline strings).
pub fn xlsx(dir: &Path, name: &str, cells: &[&str]) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    let rows: String = cells
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                r#"<row r="{r}"><c r="A{r}" t="inlineStr"><is><t>{}</t></is></c></row>"#,
                xml_escape(c),
                r = i + 1
            )
        })
        .collect();
    zip_file(
        &p,
        &[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string(),
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
                ),
            ),
        ],
    );
    p
}

/// A one-page PDF that shows `text` in Helvetica.
pub fn pdf(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    let content = format!("BT /F1 18 Tf 72 700 Td ({}) Tj ET", text.replace('(', "\\(").replace(')', "\\)"));
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, o).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes(),
    );
    std::fs::write(&p, out).unwrap();
    p
}

/// A tiny JPEG carrying EXIF DateTimeOriginal (`"YYYY:MM:DD HH:MM:SS"`) and camera Model.
pub fn jpeg_with_exif(dir: &Path, name: &str, taken: &str, camera: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    // TIFF (little endian): header, IFD0 {Model, ExifIFDPointer}, model string, Exif IFD {DateTimeOriginal}, date.
    let mut model = camera.as_bytes().to_vec();
    model.push(0);
    let ifd0_off = 8u32;
    let ifd0_len = 2 + 2 * 12 + 4;
    let model_off = ifd0_off + ifd0_len;
    let mut model_len = model.len() as u32;
    if model_len % 2 == 1 {
        model_len += 1;
    }
    let exif_off = model_off + model_len;
    let exif_len = 2 + 12 + 4;
    let date_off = exif_off + exif_len;
    let mut date = taken.as_bytes().to_vec();
    date.push(0);
    let mut t: Vec<u8> = Vec::new();
    t.extend_from_slice(b"II");
    t.extend_from_slice(&42u16.to_le_bytes());
    t.extend_from_slice(&ifd0_off.to_le_bytes());
    // IFD0
    t.extend_from_slice(&2u16.to_le_bytes());
    t.extend_from_slice(&0x0110u16.to_le_bytes());
    t.extend_from_slice(&2u16.to_le_bytes());
    t.extend_from_slice(&(model.len() as u32).to_le_bytes());
    t.extend_from_slice(&model_off.to_le_bytes());
    t.extend_from_slice(&0x8769u16.to_le_bytes());
    t.extend_from_slice(&4u16.to_le_bytes());
    t.extend_from_slice(&1u32.to_le_bytes());
    t.extend_from_slice(&exif_off.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    // model
    t.extend_from_slice(&model);
    if model.len() % 2 == 1 {
        t.push(0);
    }
    // Exif IFD
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&0x9003u16.to_le_bytes());
    t.extend_from_slice(&2u16.to_le_bytes());
    t.extend_from_slice(&(date.len() as u32).to_le_bytes());
    t.extend_from_slice(&date_off.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&date);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&t);
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&[0xFF, 0xD9]);
    std::fs::write(&p, out).unwrap();
    p
}

/// An MP3 with ID3v2.3 artist, album and title, followed by a few silent MPEG frames.
pub fn mp3_with_tags(dir: &Path, name: &str, artist: &str, album: &str, title: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    let frame = |id: &str, text: &str| {
        let mut body = vec![0u8]; // ISO-8859-1
        body.extend_from_slice(text.as_bytes());
        let mut f = id.as_bytes().to_vec();
        f.extend_from_slice(&(body.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&body);
        f
    };
    let mut frames = Vec::new();
    frames.extend(frame("TPE1", artist));
    frames.extend(frame("TALB", album));
    frames.extend(frame("TIT2", title));
    let size = frames.len() as u32;
    let syncsafe = [((size >> 21) & 0x7F) as u8, ((size >> 14) & 0x7F) as u8, ((size >> 7) & 0x7F) as u8, (size & 0x7F) as u8];
    let mut out = b"ID3".to_vec();
    out.extend_from_slice(&[3, 0, 0]);
    out.extend_from_slice(&syncsafe);
    out.extend_from_slice(&frames);
    for _ in 0..4 {
        // MPEG-1 Layer III, 128 kbps, 44.1 kHz: 417-byte frames.
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        out.extend(std::iter::repeat_n(0u8, 413));
    }
    std::fs::write(&p, out).unwrap();
    p
}
