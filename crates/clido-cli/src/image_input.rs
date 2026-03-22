//! Image attachment support: magic-byte detection, base64 encoding.
//!
//! This module is intentionally minimal — it handles local file paths only
//! (the full plan with resize and URL fetching is V2 work).  What is here is
//! enough to attach a PNG/JPEG/GIF/WebP to a TUI user message and send it to
//! a vision-capable model via `ContentBlock::Image`.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

/// A successfully loaded and base64-encoded image file.
pub struct ImageAttachment {
    /// Original file path on disk.
    pub path: PathBuf,
    /// MIME type string: "image/png", "image/jpeg", "image/gif", "image/webp".
    pub media_type: &'static str,
    /// Base64-encoded raw bytes (standard encoding, no line breaks).
    pub base64_data: String,
    /// Size in bytes of the original file.
    pub file_size: usize,
}

impl ImageAttachment {
    /// Read `path`, detect its format via magic bytes, and base64-encode it.
    ///
    /// Returns `None` if the file cannot be read or is not a supported image format.
    pub fn from_path(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let media_type = detect_media_type(&bytes)?;
        let file_size = bytes.len();
        let base64_data = B64.encode(&bytes);
        Some(Self {
            path: path.to_path_buf(),
            media_type,
            base64_data,
            file_size,
        })
    }

    /// Short filename (last component of path) for display.
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Human-readable info line shown in the TUI after `/image <path>`.
    /// Format: "Image attached: screenshot.png (PNG, 45KB)"
    pub fn info_line(&self) -> String {
        let kind = match self.media_type {
            "image/png" => "PNG",
            "image/jpeg" => "JPEG",
            "image/gif" => "GIF",
            "image/webp" => "WebP",
            other => other,
        };
        let kb = (self.file_size + 511) / 1024; // round up
        format!(
            "Image attached: {} ({}, {}KB)",
            self.display_name(),
            kind,
            kb
        )
    }
}

/// Detect image format from the first bytes of the file.
///
/// Inspects magic bytes so detection is reliable regardless of file extension.
/// Returns `None` for unknown or unsupported formats.
pub fn detect_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 4 && &bytes[0..4] == b"\x89PNG" {
        return Some("image/png");
    }
    if bytes.len() >= 3 && &bytes[0..3] == b"\xFF\xD8\xFF" {
        return Some("image/jpeg");
    }
    if bytes.len() >= 4 && (&bytes[0..4] == b"GIF8") {
        return Some("image/gif");
    }
    // WebP: "RIFF" at 0..4 and "WEBP" at 8..12
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── detect_media_type ──────────────────────────────────────────────────────

    #[test]
    fn test_detect_png_magic_bytes() {
        let bytes = b"\x89PNG\r\n\x1a\n some payload";
        assert_eq!(detect_media_type(bytes), Some("image/png"));
    }

    #[test]
    fn test_detect_jpeg_magic_bytes() {
        let bytes = b"\xFF\xD8\xFF\xE0 some jpeg payload";
        assert_eq!(detect_media_type(bytes), Some("image/jpeg"));
    }

    #[test]
    fn test_detect_gif_magic_bytes() {
        let bytes = b"GIF89a some gif payload";
        assert_eq!(detect_media_type(bytes), Some("image/gif"));
    }

    #[test]
    fn test_detect_webp_magic_bytes() {
        // 12 bytes: RIFF + 4-byte size + WEBP
        let mut bytes = vec![0u8; 12];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[4..8].copy_from_slice(b"\x00\x00\x00\x00");
        bytes[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_media_type(&bytes), Some("image/webp"));
    }

    #[test]
    fn test_detect_unknown_returns_none() {
        let bytes = b"hello world, not an image";
        assert_eq!(detect_media_type(bytes), None);
    }

    #[test]
    fn test_detect_too_short_returns_none() {
        assert_eq!(detect_media_type(b"\xFF\xD8"), None);
        assert_eq!(detect_media_type(b""), None);
    }

    // ── ImageAttachment::from_path ─────────────────────────────────────────────

    #[test]
    fn test_from_path_png_roundtrip() {
        // Write a minimal PNG magic header to a temp file and verify loading.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
        f.write_all(png_header).unwrap();
        f.flush().unwrap();

        let att = ImageAttachment::from_path(f.path()).expect("should load PNG");
        assert_eq!(att.media_type, "image/png");
        assert!(!att.base64_data.is_empty());
        assert_eq!(att.file_size, png_header.len());
    }

    #[test]
    fn test_from_path_non_image_returns_none() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"just text, not an image").unwrap();
        f.flush().unwrap();

        assert!(ImageAttachment::from_path(f.path()).is_none());
    }

    #[test]
    fn test_from_path_missing_file_returns_none() {
        let p = std::path::Path::new("/tmp/clido_test_nonexistent_image_12345.png");
        assert!(ImageAttachment::from_path(p).is_none());
    }

    #[test]
    fn test_info_line_format_png() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        // Write 1024 bytes of PNG
        let mut data = vec![0u8; 1024];
        data[0..4].copy_from_slice(b"\x89PNG");
        f.write_all(&data).unwrap();
        f.flush().unwrap();

        let att = ImageAttachment::from_path(f.path()).unwrap();
        let line = att.info_line();
        assert!(line.contains("PNG"), "expected PNG in info line: {}", line);
        assert!(line.contains("1KB"), "expected 1KB in info line: {}", line);
        assert!(line.starts_with("Image attached:"));
    }
}
