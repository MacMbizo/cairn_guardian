//! Works out what a file is (spec §7.2): its kind, and for documents the life area, document
//! type, who it is from and its date, with a confidence and the evidence behind it.

use super::Kind;

const CODE_EXTS: [&str; 18] = [
    "rs", "py", "ts", "tsx", "js", "jsx", "java", "c", "cpp", "h", "cs", "go", "php", "rb", "vb", "ipynb", "kt", "swift",
];
const SYSTEM_EXTS: [&str; 18] = [
    "dll", "cab", "cat", "mst", "pyc", "pyi", "sys", "ini", "dat", "cfg", "resources", "manifest", "msp", "tmp", "log",
    "lnk", "db", "pdb",
];
const DOC_EXTS: [&str; 16] = [
    "pdf", "doc", "docx", "odt", "rtf", "txt", "md", "xls", "xlsx", "xlsm", "ods", "csv", "ppt", "pptx", "odp", "pages",
];
const PHOTO_EXTS: [&str; 9] = ["jpg", "jpeg", "heic", "png", "tif", "tiff", "raw", "cr2", "nef"];
const VIDEO_EXTS: [&str; 9] = ["mp4", "mov", "mkv", "avi", "wmv", "m4v", "webm", "3gp", "mts"];
const MUSIC_EXTS: [&str; 6] = ["mp3", "m4a", "flac", "ogg", "wma", "aac"];
const AUDIO_EXTS: [&str; 3] = ["wav", "m4b", "opus"];
const ARCHIVE_EXTS: [&str; 5] = ["zip", "rar", "7z", "tar", "gz"];
const FONT_EXTS: [&str; 4] = ["ttf", "otf", "woff", "woff2"];

fn looks_like_screenshot(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("screenshot") || n.contains("screen shot") || n.starts_with("capture") || n.starts_with("snip")
}

/// The kind of a file from its extension and name. Anything inside a code project is code.
pub fn kind_of(ext: &str, name: &str, in_atomic_unit: bool) -> Kind {
    if in_atomic_unit {
        return Kind::Code;
    }
    let e = ext.to_ascii_lowercase();
    let e = e.as_str();
    if CODE_EXTS.contains(&e) {
        Kind::Code
    } else if SYSTEM_EXTS.contains(&e) {
        Kind::System
    } else if ["exe", "msi", "msix"].contains(&e) {
        Kind::Installer
    } else if ARCHIVE_EXTS.contains(&e) {
        Kind::Archive
    } else if FONT_EXTS.contains(&e) {
        Kind::Font
    } else if (e == "png" || e == "jpg" || e == "jpeg") && looks_like_screenshot(name) {
        Kind::Screenshot
    } else if PHOTO_EXTS.contains(&e) {
        Kind::Photo
    } else if VIDEO_EXTS.contains(&e) {
        Kind::Video
    } else if MUSIC_EXTS.contains(&e) {
        Kind::Music
    } else if AUDIO_EXTS.contains(&e) {
        Kind::Audio
    } else if DOC_EXTS.contains(&e) {
        Kind::Document
    } else {
        Kind::Other
    }
}
