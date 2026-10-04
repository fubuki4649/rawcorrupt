use chrono::{DateTime, Utc};
use serde::Serialize;

/// Asset metadata representation matching suisai's `NewDbAsset`.
#[derive(Debug, Clone, Serialize)]
pub struct NewDbAsset {
    pub parent_id: Option<String>,
    pub thumbnail_path: Option<String>,
    pub hash: String,
    pub file_name: String,
    pub size_on_disk: i64,
    pub photo_date: DateTime<Utc>,
    pub photo_timezone: String,
    pub resolution_width: i64,
    pub resolution_height: i64,
    pub mime_type: String,
    pub camera_model: String,
    pub lens_model: String,
    pub shutter_count: i64,
    pub focal_length: i16,
    pub iso: i64,
    pub shutter_speed: String,
    pub aperture: f32,
}
