use crate::models::asset::NewDbAsset;
use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDateTime, Utc};
use exiftool_rs::{ImageInfo, image_info};
use std::fs;
use std::path::PathBuf;
use xxhash_rust::xxh3::xxh3_128;

/// A trait providing methods to extract metadata from an image file path
/// and convert it into a database-compatible format, copied directly from suisai.
pub trait SuisaiAsset {
    /// Gets the `xxh3_128` content hash of the asset file
    #[allow(dead_code)]
    fn get_hash(&self) -> String;

    /// On-disk size of the asset in KB
    #[allow(dead_code)]
    fn get_size_on_disk(&self) -> i64;

    /// The date/time the photo was taken, in UTC
    fn get_photo_date(&self, info: &ImageInfo) -> DateTime<Utc>;

    /// The timezone where the photo was taken, as a UTC offset. Defaults to JST (UTC+9).
    fn get_photo_timezone(&self, info: &ImageInfo) -> String;

    /// Returns a `Vec<i64>` of length 2 representing the dimensions of the image (width, height)
    fn get_resolution(&self, info: &ImageInfo) -> Vec<i64>;

    /// The MIME type of the image
    fn get_mime(&self, info: &ImageInfo) -> String;

    /// The model of the camera used to take the image
    fn get_camera_model(&self, info: &ImageInfo) -> String;

    /// The model of the lens used to take the image
    fn get_lens_model(&self, info: &ImageInfo) -> String;

    /// The shutter count of the camera when the image was taken.
    fn get_shutter_count(&self, info: &ImageInfo) -> i64;

    /// The focal length used to take the image, in mm
    fn get_focal_length(&self, info: &ImageInfo) -> i16;

    /// ISO sensitivity of the camera when the image was taken
    fn get_iso(&self, info: &ImageInfo) -> i64;

    /// The shutter speed used to take the photo
    fn get_shutter_speed(&self, info: &ImageInfo) -> String;

    /// The aperture setting (f-stop) used to take the photo
    fn get_aperture(&self, info: &ImageInfo) -> f32;

    /// Returns a `crate::models::asset::NewDbAsset`.
    fn to_db_entry(&self, hash: String, size_on_disk: i64) -> NewDbAsset;

    /// Validating variant for corruption testing that fails if ExifTool errors or dimensions are invalid.
    fn try_to_db_entry(&self, hash: String, size_on_disk: i64) -> Result<NewDbAsset>;
}

impl SuisaiAsset for PathBuf {
    fn get_hash(&self) -> String {
        format!("{:032x}", xxh3_128(&fs::read(self).unwrap_or_default()))
    }

    fn get_size_on_disk(&self) -> i64 {
        fs::metadata(self)
            .map(|m| m.len().div_ceil(1024) as i64)
            .unwrap_or(0)
    }

    fn get_photo_date(&self, info: &ImageInfo) -> DateTime<Utc> {
        info.get("DateTimeOriginal")
            .and_then(|s| NaiveDateTime::parse_from_str(s.trim(), "%Y:%m:%d %H:%M:%S").ok())
            .map(|ndt| ndt.and_utc())
            .unwrap_or_default()
    }

    fn get_photo_timezone(&self, info: &ImageInfo) -> String {
        info.get("OffsetTimeOriginal")
            .filter(|tz| tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')))
            .cloned()
            .unwrap_or_else(|| "+09:00".to_string())
    }

    fn get_resolution(&self, info: &ImageInfo) -> Vec<i64> {
        vec![
            info.get("ImageWidth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            info.get("ImageHeight")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        ]
    }

    fn get_mime(&self, info: &ImageInfo) -> String {
        info.get("MIMEType")
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string())
    }

    fn get_camera_model(&self, info: &ImageInfo) -> String {
        info.get("Model")
            .cloned()
            .unwrap_or_else(|| "Unknown Camera".to_string())
    }

    fn get_lens_model(&self, info: &ImageInfo) -> String {
        ["LensModel", "Lens"]
            .iter()
            .find_map(|&tag| info.get(tag).filter(|s| !s.is_empty()).cloned())
            .unwrap_or_else(|| "Unknown Lens".to_string())
    }

    fn get_shutter_count(&self, info: &ImageInfo) -> i64 {
        const SHUTTER_TAGS: &[&str] = &[
            "ShutterCount",
            "MechanicalShutterCount",
            "ImageCount",
            "ShutterCount2",
            "ShutterCount3",
            "TotalShutterCount",
        ];

        SHUTTER_TAGS
            .iter()
            .filter_map(|&tag| info.get(tag))
            .filter_map(|raw| raw.parse::<i64>().ok())
            .find(|&count| count != 0)
            .unwrap_or(0)
    }

    fn get_focal_length(&self, info: &ImageInfo) -> i16 {
        info.get("FocalLength")
            .and_then(|s| s.split_whitespace().next()?.parse::<f32>().ok())
            .map(|f| f.round() as i16)
            .unwrap_or(0)
    }

    fn get_iso(&self, info: &ImageInfo) -> i64 {
        info.get("ISO")
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    }

    fn get_shutter_speed(&self, info: &ImageInfo) -> String {
        info.get("ShutterSpeed")
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string())
    }

    fn get_aperture(&self, info: &ImageInfo) -> f32 {
        info.get("Aperture")
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .map(|aperture: f32| (aperture * 10.0).round() / 10.0)
            .unwrap_or(0.0)
    }

    fn to_db_entry(&self, hash: String, size_on_disk: i64) -> NewDbAsset {
        let info = image_info(self).unwrap_or_default();
        let res = self.get_resolution(&info);
        NewDbAsset {
            parent_id: None,
            thumbnail_path: None,
            hash,
            file_name: self
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            size_on_disk,
            photo_date: self.get_photo_date(&info),
            photo_timezone: self.get_photo_timezone(&info),
            resolution_width: res[0],
            resolution_height: res[1],
            mime_type: self.get_mime(&info),
            camera_model: self.get_camera_model(&info),
            lens_model: self.get_lens_model(&info),
            shutter_count: self.get_shutter_count(&info),
            focal_length: self.get_focal_length(&info),
            iso: self.get_iso(&info),
            shutter_speed: self.get_shutter_speed(&info),
            aperture: self.get_aperture(&info),
        }
    }

    fn try_to_db_entry(&self, hash: String, size_on_disk: i64) -> Result<NewDbAsset> {
        let info = image_info(self).map_err(|e| anyhow!("ExifTool execution failed: {e}"))?;

        if let Some(err) = info.get("Error") {
            return Err(anyhow!("ExifTool reported corrupted metadata: {err}"));
        }

        if let Some(warning) = info.get("Warning") {
            let warn_lower = warning.to_lowercase();
            if warn_lower.contains("past end of file")
                || warn_lower.contains("corrupt")
                || warn_lower.contains("truncated")
                || warn_lower.contains("bad subifd")
                || warn_lower.contains("bad ifd")
                || warn_lower.contains("premature end of file")
                || warn_lower.contains("error reading")
            {
                return Err(anyhow!(
                    "ExifTool detected corrupted file structure: {warning}"
                ));
            }
        }

        // Validate that sensor data strips or tiles do not extend beyond the actual file length
        let file_size_bytes = fs::metadata(self).map(|m| m.len()).unwrap_or(0);
        if file_size_bytes > 0 {
            if let (Some(offsets_str), Some(counts_str)) =
                (info.get("StripOffsets"), info.get("StripByteCounts"))
            {
                let last_offset = offsets_str
                    .split_whitespace()
                    .last()
                    .and_then(|s| s.parse::<u64>().ok());
                let last_count = counts_str
                    .split_whitespace()
                    .last()
                    .and_then(|s| s.parse::<u64>().ok());
                if let (Some(offset), Some(count)) = (last_offset, last_count)
                    && offset.saturating_add(count) > file_size_bytes
                {
                    return Err(anyhow!(
                        "File truncated: sensor data strip ends at byte {} but file is only {} bytes",
                        offset + count,
                        file_size_bytes
                    ));
                }
            }

            if let (Some(offsets_str), Some(counts_str)) =
                (info.get("TileOffsets"), info.get("TileByteCounts"))
            {
                let last_offset = offsets_str
                    .split_whitespace()
                    .last()
                    .and_then(|s| s.parse::<u64>().ok());
                let last_count = counts_str
                    .split_whitespace()
                    .last()
                    .and_then(|s| s.parse::<u64>().ok());
                if let (Some(offset), Some(count)) = (last_offset, last_count)
                    && offset.saturating_add(count) > file_size_bytes
                {
                    return Err(anyhow!(
                        "File truncated: sensor data tile ends at byte {} but file is only {} bytes",
                        offset + count,
                        file_size_bytes
                    ));
                }
            }
        }

        let res = self.get_resolution(&info);
        if res[0] <= 0 && res[1] <= 0 {
            return Err(anyhow!(
                "Missing or invalid image dimensions in EXIF metadata (width: {}, height: {})",
                res[0],
                res[1]
            ));
        }

        Ok(NewDbAsset {
            parent_id: None,
            thumbnail_path: None,
            hash,
            file_name: self
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            size_on_disk,
            photo_date: self.get_photo_date(&info),
            photo_timezone: self.get_photo_timezone(&info),
            resolution_width: res[0],
            resolution_height: res[1],
            mime_type: self.get_mime(&info),
            camera_model: self.get_camera_model(&info),
            lens_model: self.get_lens_model(&info),
            shutter_count: self.get_shutter_count(&info),
            focal_length: self.get_focal_length(&info),
            iso: self.get_iso(&info),
            shutter_speed: self.get_shutter_speed(&info),
            aperture: self.get_aperture(&info),
        })
    }
}
