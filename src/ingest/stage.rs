use serde::{Deserialize, Serialize};
use std::fmt;

/// Ingestion pipeline stages corresponding to suisai's ingest steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStage {
    /// File discovery and format type detection via infer (`search_path_for_assets`).
    SearchPath,
    /// Reading file bytes and computing xxh3_128 content hash (`hash_and_transfer`).
    HashAndRead,
    /// Extracting and parsing EXIF/TIFF metadata into DB asset (`SuisaiAsset::to_db_entry`).
    MetadataExtraction,
    /// Decoding RAW sensor data and compressing JPEG thumbnail (`extract_thumbnail`).
    ThumbnailExtraction,
}

impl IngestStage {
    pub const ALL: [IngestStage; 4] = [
        IngestStage::SearchPath,
        IngestStage::HashAndRead,
        IngestStage::MetadataExtraction,
        IngestStage::ThumbnailExtraction,
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            IngestStage::SearchPath => "Search & Type Check",
            IngestStage::HashAndRead => "Hash & Read",
            IngestStage::MetadataExtraction => "Metadata Extraction",
            IngestStage::ThumbnailExtraction => "Thumbnail & RAW Decode",
        }
    }

    pub const fn step_number(&self) -> usize {
        match self {
            IngestStage::SearchPath => 1,
            IngestStage::HashAndRead => 2,
            IngestStage::MetadataExtraction => 3,
            IngestStage::ThumbnailExtraction => 4,
        }
    }

    pub const fn description(&self) -> &'static str {
        match self {
            IngestStage::SearchPath => "Inferring image magic bytes in search_path",
            IngestStage::HashAndRead => "Streaming and hashing file bytes with xxh3_128",
            IngestStage::MetadataExtraction => {
                "Extracting EXIF tags into NewDbAsset via SuisaiAsset"
            }
            IngestStage::ThumbnailExtraction => {
                "Decoding sensor raw data and generating JPEG thumbnail"
            }
        }
    }
}

impl fmt::Display for IngestStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}
