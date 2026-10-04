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
            Self::FileRead => "File Read / I/O Problem",
            Self::Metadata => "Metadata / Header Problem",
            Self::EmbeddedThumbnail => "Embedded Thumbnail Problem",
            Self::RawSensorData => "RAW Sensor Data Corruption",
            Self::JpegEncode => "JPEG Encoding Problem",
        }
    }

    pub const fn description(&self) -> &'static str {
        match self {
            Self::FileRead => {
                "Failed reading file from disk, empty file, or invalid image magic bytes"
            }
            Self::Metadata => "Corrupted EXIF/TIFF headers, broken IFD tags, or invalid dimensions",
            Self::EmbeddedThumbnail => "Corrupted embedded preview or thumbnail stream",
            Self::RawSensorData => {
                "Corrupted RAW Bayer data, unexpected EOF in sensor stream, or unpack/demosaic failure"
            }
            Self::JpegEncode => "Failed to compress decoded bitmap into JPEG",
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Diagnostic return codes and execution path from LibRaw C operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibRawDiagnostics {
    pub open_route: String,
    pub open_status: i32,
    pub unpack_status: Option<i32>,
    pub process_status: Option<i32>,
    pub mem_image_status: Option<i32>,
    pub data_callback: Option<String>,
}

impl LibRawDiagnostics {
    pub fn new(open_status: i32) -> Self {
        Self {
            open_route: "libraw_open_file (direct file path)".to_string(),
            open_status,
            unpack_status: None,
            process_status: None,
            mem_image_status: None,
            data_callback: None,
        }
    }

    pub fn status_summary(&self) -> String {
        format!(
            "open={} ({}), unpack={}, dcraw_process={}",
            self.open_status,
            libraw_status_name(self.open_status),
            self.unpack_status.map_or("N/A".to_string(), |c| format!(
                "{c} ({})",
                libraw_status_name(c)
            )),
            self.process_status.map_or("N/A".to_string(), |c| format!(
                "{c} ({})",
                libraw_status_name(c)
            )),
        )
    }
}

pub fn libraw_status_name(code: i32) -> &'static str {
    match code {
        0 => "LIBRAW_SUCCESS",
        -1 => "LIBRAW_UNSPECIFIED_ERROR",
        -2 => "LIBRAW_FILE_UNSUPPORTED",
        -3 => "LIBRAW_REQUEST_FOR_NONEXISTENT_IMAGE",
        -4 => "LIBRAW_OUT_OF_ORDER_CALL",
        -5 => "LIBRAW_NO_THUMBNAIL",
        -6 => "LIBRAW_UNSUPPORTED_THUMBNAIL",
        -7 => "LIBRAW_INPUT_CLOSED",
        -8 => "LIBRAW_NOT_IMPLEMENTED",
        -9 => "LIBRAW_REQUEST_FOR_NONEXISTENT_THUMBNAIL",
        -100007 => "LIBRAW_CANCELLED_BY_CALLBACK",
        -100008 => "LIBRAW_DATA_ERROR",
        -100009 => "LIBRAW_IO_ERROR",
        -100010 => "LIBRAW_PACKED_DATA_ERROR",
        -100011 => "LIBRAW_UNSUFFICIENT_MEMORY",
        -100012 => "LIBRAW_TOO_BIG",
        -100013 => "LIBRAW_MEMPOOL_OVERFLOW",
        _ => "LIBRAW_UNKNOWN_ERROR",
    }
}

/// Detailed failure information including category and human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureDetail {
    pub category: FailureCategory,
    pub message: String,
    pub diagnostics: Option<LibRawDiagnostics>,
}

impl FailureDetail {
    pub fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            diagnostics: None,
        }
    }

    pub fn with_diagnostics(mut self, diag: LibRawDiagnostics) -> Self {
        self.diagnostics = Some(diag);
        self
    }

    pub fn file_read(msg: impl Into<String>) -> Self {
        Self::new(FailureCategory::FileRead, msg)
    }
    pub fn metadata(msg: impl Into<String>) -> Self {
        Self::new(FailureCategory::Metadata, msg)
    }
    pub fn embedded_thumbnail(msg: impl Into<String>) -> Self {
        Self::new(FailureCategory::EmbeddedThumbnail, msg)
    }
    pub fn raw_sensor_data(msg: impl Into<String>) -> Self {
        Self::new(FailureCategory::RawSensorData, msg)
    }
    pub fn jpeg_encode(msg: impl Into<String>) -> Self {
        Self::new(FailureCategory::JpegEncode, msg)
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
    pub diagnostics: Option<LibRawDiagnostics>,
}
