//! Attachment processing — MIME detection, validation, and thumbnails.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

use crate::core::models::{Attachment, AttachmentKind};

/// Allowed MIME prefixes / types.
const ALLOWED_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/webm",
    "audio/mpeg",
    "audio/wav",
    "audio/ogg",
    "application/pdf",
    "text/plain",
    "text/markdown",
    "text/x-markdown",
];

/// Maximum file sizes per category.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB
const MAX_VIDEO_BYTES: u64 = 100 * 1024 * 1024; // 100 MB
const MAX_DOCUMENT_BYTES: u64 = 50 * 1024 * 1024; // 50 MB

/// Validates and prepares an attachment from a file path.
pub fn prepare_attachment(path: &Path) -> Result<Attachment> {
    let attachment = Attachment::from_path(path.to_path_buf())?;
    validate_attachment(&attachment)?;
    Ok(attachment)
}

/// Check that an attachment is allowed and within size limits.
pub fn validate_attachment(attachment: &Attachment) -> Result<()> {
    let mime = attachment.mime_type.as_str();

    let allowed = ALLOWED_MIMES.iter().any(|&m| mime == m);
    if !allowed {
        bail!(
            "File type '{}' is not supported. \
            Allowed: images (PNG, JPG, WebP, GIF), video (MP4), \
            audio, PDF, plain text.",
            mime
        );
    }

    let max = match attachment.kind {
        AttachmentKind::Image => MAX_IMAGE_BYTES,
        AttachmentKind::Video => MAX_VIDEO_BYTES,
        _ => MAX_DOCUMENT_BYTES,
    };

    if attachment.size_bytes > max {
        bail!(
            "File '{}' is too large ({} MB). Maximum is {} MB for this file type.",
            attachment.file_name,
            attachment.size_bytes / 1024 / 1024,
            max / 1024 / 1024,
        );
    }

    Ok(())
}

/// Read a file as base64-encoded bytes (for inline embedding in API calls).
pub fn file_to_base64(path: &Path) -> Result<String> {
    use base64::{Engine as _, engine::general_purpose};
    let bytes = std::fs::read(path)?;
    Ok(general_purpose::STANDARD.encode(bytes))
}

/// Save generated media (image or video) to the downloads folder.
/// Returns the path where the file was saved.
pub async fn save_generated_media(
    data: &[u8],
    filename: &str,
    download_dir: &Path,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(download_dir).await?;
    let dest = download_dir.join(filename);
    tokio::fs::write(&dest, data).await?;
    Ok(dest)
}

/// Download a URL to the downloads folder and return the local path.
pub async fn download_url(url: &str, download_dir: &Path) -> Result<PathBuf> {
    let response = reqwest::get(url).await?;
    let filename = url
        .rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .unwrap_or("download")
        .to_string();

    let bytes = response.bytes().await?;
    save_generated_media(&bytes, &filename, download_dir).await
}
