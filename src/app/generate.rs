//! Shared types and helpers for media generation (image / video / audio).

use crate::core::provider::ProviderError;
use gtk4::prelude::*;
use std::path::PathBuf;

/// PixVerse video models exposed to the user (default first).
pub const PIXVERSE_MODELS: &[&str] = &["v6", "c1", "v5.6", "v5.5", "v5", "v4.5", "v4", "v3.5"];

/// The kind of media the user wants to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenKind {
    Image,
    Video,
    Audio,
}

impl GenKind {
    pub fn label(self) -> &'static str {
        match self {
            GenKind::Image => "Image",
            GenKind::Video => "Video",
            GenKind::Audio => "Audio",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "image" => Self::Image,
            "video" => Self::Video,
            _ => Self::Audio,
        }
    }
}

/// Capabilities of a provider relevant to generation.
#[derive(Debug, Clone)]
pub struct ProviderCap {
    pub id: String,
    pub name: String,
    pub supports_image: bool,
    pub supports_video: bool,
}

/// A fully-built generation request ready to be sent to a provider.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub provider_id: String,
    pub kind: GenKind,
    pub prompt: String,
    pub num_images: u32,
    pub model: String,
    pub duration_seconds: u8,
    pub quality: String,
    pub aspect_ratio: String,
    pub image_path: Option<PathBuf>,
}

/// Read the currently selected text of a `gtk::DropDown`.
pub fn dropdown_text(dd: &gtk4::DropDown) -> Option<String> {
    dd.selected_item()
        .and_then(|i| i.downcast::<gtk4::StringObject>().ok())
        .map(|s| s.string().to_string())
}

/// Estimated credit cost for a video without audio.
///
/// Per-second billing models (v6/c1):
///   v6: 360p=5/s, 540p=7/s, 720p=9/s, 1080p=18/s
///   c1: 360p=6/s, 540p=8/s, 720p=10/s, 1080p=19/s
///
/// Fixed-price models (v5.6/v5.5/v5/v4.5/v4/v3.5) are billed per clip
/// (5/8/10s). We estimate from their 5-second price, scaled linearly.
pub fn estimate_video_credits(model: &str, quality: &str, duration_secs: u64) -> u64 {
    let per_second: u64 = match model.to_ascii_lowercase().as_str() {
        "v6" => match quality {
            "360p" => 5,
            "540p" => 7,
            "1080p" => 18,
            _ => 9,
        },
        "c1" => match quality {
            "360p" => 6,
            "540p" => 8,
            "1080p" => 19,
            _ => 10,
        },
        _ => {
            let base_5s: u64 = match model.to_ascii_lowercase().as_str() {
                "v5.6" => match quality {
                    "360p" | "540p" => 35,
                    "720p" => 45,
                    _ => 75,
                },
                "v5.5" => match quality {
                    "360p" | "540p" => 45,
                    "720p" => 60,
                    _ => 120,
                },
                _ => match quality {
                    "360p" | "540p" => 45,
                    "720p" => 60,
                    _ => 120,
                },
            };
            return base_5s.saturating_mul(duration_secs).saturating_div(5);
        }
    };
    per_second.saturating_mul(duration_secs)
}

/// Format an integer with thousands separators (e.g. 1069020 -> "1,069,020").
pub fn format_num(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Convert a provider/generation error into a short, actionable message
/// suitable for the media UI. Specific `ProviderError` variants are turned
/// into friendly guidance; generic errors fall back to heuristics or the
/// raw message.
pub fn friendly_error(e: &anyhow::Error) -> String {
    for cause in e.chain() {
        if let Some(pe) = cause.downcast_ref::<ProviderError>() {
            return match pe {
                ProviderError::NotSupported {
                    provider,
                    capability,
                } => format!("{} does not support {}.", provider, capability),
                ProviderError::AuthError(_) => {
                    "Authentication failed. Check your API key in Settings.".to_string()
                }
                ProviderError::ApiError { status, .. } => friendly_http_error(*status),
                ProviderError::RateLimit { retry_after_secs } => format!(
                    "Rate limit reached. Try again in ~{} seconds.",
                    retry_after_secs
                ),
                ProviderError::Timeout => "The request timed out. Please try again.".to_string(),
                ProviderError::StreamError(_) => {
                    "The connection was interrupted. Please try again.".to_string()
                }
                ProviderError::VideoFailed(msg) => {
                    format!("Video generation failed: {}", msg)
                }
                ProviderError::UploadFailed(_) => {
                    "Could not upload the reference image. Try a different file.".to_string()
                }
                ProviderError::NotConfigured => {
                    "Provider not configured. Add an API key in Settings.".to_string()
                }
                ProviderError::Other(_) => return fallback_error(e),
            };
        }
    }
    fallback_error(e)
}

fn friendly_http_error(status: u16) -> String {
    match status {
        400 => "The provider rejected the request (400). Try a different prompt or settings."
            .to_string(),
        401 | 403 => {
            "Access denied (401/403). Your API key may be invalid or lack permission — check it \
             in Settings."
                .to_string()
        }
        402 => "Insufficient credits or balance (402). Add credits to your provider account."
            .to_string(),
        429 => "Rate limit exceeded. Wait a moment and try again.".to_string(),
        5..=599 => "The provider server had an error. Try again in a moment.".to_string(),
        _ => format!("The provider returned an error (HTTP {}).", status),
    }
}

fn fallback_error(e: &anyhow::Error) -> String {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        "The request timed out. Please try again.".to_string()
    } else if lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("refused")
        || lower.contains("error sending request")
    {
        "Network error — check your internet connection.".to_string()
    } else if text.trim().is_empty() {
        "Something went wrong during generation.".to_string()
    } else {
        text
    }
}
