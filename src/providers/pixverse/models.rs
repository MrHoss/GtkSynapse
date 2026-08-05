//! PixVerse-specific API models and types.
//!
//! Aligned with the official OpenAPI spec (openapi/v2):
//! <https://docs.platform.pixverse.ai>

use serde::{Deserialize, Serialize};

/// Default model used for video generation (drives cost estimation).
pub const PIXVERSE_DEFAULT_MODEL: &str = "v6";

/// Snap a requested duration to a value the given model supports.
///
/// Per the official spec: v6/c1 allow 1..=15; v5.5/v5.6 allow 5/8/10;
/// all older models allow 5/8.
pub fn clamp_duration(model: &str, duration: u8) -> u8 {
    match model.to_ascii_lowercase().as_str() {
        "v6" | "c1" => duration.clamp(1, 15),
        "v5.5" | "v5.6" => nearest_duration(&[5, 8, 10], duration),
        _ => nearest_duration(&[5, 8], duration),
    }
}

/// Valid durations for a model (for the UI spin button).
pub fn model_duration_range(model: &str) -> (u8, u8) {
    match model.to_ascii_lowercase().as_str() {
        "v6" | "c1" => (1, 15),
        "v5.5" | "v5.6" => (5, 10),
        _ => (5, 8),
    }
}

fn nearest_duration(allowed: &[u8], duration: u8) -> u8 {
    allowed
        .iter()
        .copied()
        .min_by_key(|&a| (a as i32 - duration as i32).abs())
        .unwrap_or(allowed[0])
}

// ─── Text-to-Video Request ────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PixVerseTextToVideoRequest {
    pub prompt: String,
    pub model: String,
    pub duration: u8,         // v6/c1: 1..=15, older models: 5/8
    pub quality: String,      // "360p", "540p", "720p", "1080p"
    pub aspect_ratio: String, // "16:9", "9:16", "1:1", "4:3", "3:4"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_mode: Option<String>, // "normal" or "fast"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
}

// ─── Image-to-Video Request ───────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PixVerseImageToVideoRequest {
    pub prompt: String,
    pub model: String,
    pub img_id: i64, // Image ID returned by the upload API
    pub duration: u8,
    pub quality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_mode: Option<String>,
}

// ─── Response Types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PixVerseApiResponse<T> {
    #[serde(rename = "ErrCode")]
    pub err_code: i32,
    #[serde(rename = "ErrMsg")]
    pub err_msg: Option<String>,
    #[serde(rename = "Resp")]
    pub resp: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct PixVerseCreateTaskResp {
    #[serde(rename = "video_id")]
    pub video_id: i64,
}

/// Video status response. Status codes (official):
/// 1 = success, 5 = generating, 6 = deleted,
/// 7 = content moderation failed, 8 = generation failed.
#[derive(Debug, Deserialize)]
pub struct PixVerseVideoStatus {
    #[serde(rename = "id")]
    pub id: i64,
    #[serde(rename = "status")]
    pub status: i32,
    #[serde(rename = "url")]
    pub url: Option<String>,
}

/// Response of `POST /image/upload`. `img_id` is used in later requests.
#[derive(Debug, Deserialize)]
pub struct PixVerseUploadResponse {
    #[serde(rename = "img_id")]
    pub img_id: i64,
    #[serde(rename = "img_url", alias = "url")]
    pub img_url: Option<String>,
}

/// Response of `GET /account/balance`.
#[derive(Debug, Deserialize)]
pub struct PixVerseBalance {
    #[serde(rename = "account_id")]
    pub account_id: i64,
    #[serde(rename = "credit_monthly")]
    pub credit_monthly: i64,
    #[serde(rename = "credit_package")]
    pub credit_package: i64,
}

// ─── Status Helpers ───────────────────────────────────────────

impl PixVerseVideoStatus {
    pub fn is_done(&self) -> bool {
        self.status == 1
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.status, 6 | 7 | 8)
    }

    pub fn is_pending(&self) -> bool {
        !self.is_done() && !self.is_failed()
    }

    /// The API does not report percentages; return representative values.
    pub fn progress_percent(&self) -> u8 {
        match self.status {
            1 => 100,
            6 | 7 | 8 => 0,
            _ => 50,
        }
    }

    pub fn status_label(&self) -> &'static str {
        match self.status {
            1 => "Completed",
            5 => "Generating",
            6 => "Deleted",
            7 => "Content moderation failed",
            8 => "Generation failed",
            _ => "Processing",
        }
    }
}
