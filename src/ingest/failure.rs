use serde::{Deserialize, Serialize};
use std::fmt;

/// Categories of camera RAW corruption or failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    /// File cannot be opened, read from storage, is 0 bytes, or magic bytes are invalid.
    FileRead,
    /// EXIF/TIFF headers, tags, IFD structure corrupted or truncated.
    Metadata,
    /// Embedded preview or thumbnail stream is corrupted or truncated.
    EmbeddedThumbnail,
    /// Bayer sensor data stream is corrupted, unexpected EOF, or LibRaw unpack/process failed.
    RawSensorData,
    /// Mozjpeg encoder failed to compress RGB bitmap into JPEG.
    JpegEncode,
}

impl FailureCategory {
    pub const ALL: [FailureCategory; 5] = [
        FailureCategory::FileRead,
        FailureCategory::Metadata,
        FailureCategory::EmbeddedThumbnail,
        FailureCategory::RawSensorData,
        FailureCategory::JpegEncode,
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            FailureCategory::FileRead => "File Read / I/O Problem",
            FailureCategory::Metadata => "Metadata / Header Problem",
            FailureCategory::EmbeddedThumbnail => "Embedded Thumbnail Problem",
            FailureCategory::RawSensorData => "RAW Sensor Data Corruption",
            FailureCategory::JpegEncode => "JPEG Encoding Problem",
        }
    }

    pub const fn description(&self) -> &'static str {
        match self {
            FailureCategory::FileRead => {
                "Failed reading file from disk, empty file, or invalid image magic bytes"
            }
            FailureCategory::Metadata => {
                "Corrupted EXIF/TIFF headers, broken IFD tags, or invalid dimensions"
            }
            FailureCategory::EmbeddedThumbnail => "Corrupted embedded preview or thumbnail stream",
            FailureCategory::RawSensorData => {
                "Corrupted RAW Bayer data, unexpected EOF in sensor stream, or unpack/demosaic failure"
            }
            FailureCategory::JpegEncode => "Failed to compress decoded bitmap into JPEG",
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Detailed failure information including category and human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureDetail {
    pub category: FailureCategory,
    pub message: String,
}

impl FailureDetail {
    pub fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    pub fn file_read(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::FileRead, message)
    }

    pub fn metadata(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Metadata, message)
    }

    pub fn embedded_thumbnail(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::EmbeddedThumbnail, message)
    }

    pub fn raw_sensor_data(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::RawSensorData, message)
    }

    pub fn jpeg_encode(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::JpegEncode, message)
    }
}

impl fmt::Display for FailureDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category.name(), self.message)
    }
}

impl std::error::Error for FailureDetail {}

/// Record of a corrupted file with its absolute path and failure details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorruptedFile {
    pub path: std::path::PathBuf,
    pub category: FailureCategory,
    pub error: String,
}
