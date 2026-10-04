use rawcorrupt::ingest::helpers::extract_thumbnail::{extract_thumbnail, test_extract_thumbnail};
use rawcorrupt::ingest::helpers::hash_and_transfer::hash_and_read;
use rawcorrupt::ingest::helpers::search_path::search_path_for_assets;
use rawcorrupt::ingest::main::ingest;
use rawcorrupt::ingest::stage::IngestStage;
use rawcorrupt::ingest::traits::SuisaiAsset;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use tempfile::tempdir;

#[tokio::test]
async fn test_empty_file_fails_at_hash_and_read() {
    let dir = tempdir().unwrap();
    let empty_path = dir.path().join("empty.ARW");
    File::create(&empty_path).unwrap();

    let res = hash_and_read(&empty_path).await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[tokio::test]
async fn test_non_image_garbage_fails_at_search_path() {
    let dir = tempdir().unwrap();
    let garbage_path = dir.path().join("corrupt_header.ARW");
    let mut file = File::create(&garbage_path).unwrap();
    file.write_all(b"THIS IS NOT A VALID CAMERA RAW FILE HEADER AT ALL 1234567890")
        .unwrap();

    let summary = ingest(
        garbage_path.to_str().unwrap().to_string(),
        Some(1),
        true,
        None,
    )
    .await;

    assert_eq!(summary.total, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(
        summary.stage_failures.get(&IngestStage::SearchPath),
        Some(&1)
    );
}

#[tokio::test]
async fn test_truncated_file_fails_at_metadata_extraction() {
    let sample_raw = PathBuf::from("/home/kaneki/suisai-test/original_raws/_DSC0326.ARW");
    if !sample_raw.exists() {
        eprintln!("Skipping test: test raw not found");
        return;
    }

    let dir = tempdir().unwrap();
    let truncated_path = dir.path().join("truncated_16k.ARW");

    // Copy first 16KB of the raw file (missing SubIFD table and dimension tags)
    let raw_bytes = fs::read(&sample_raw).unwrap();
    let truncated_bytes = &raw_bytes[..16 * 1024];
    fs::write(&truncated_path, truncated_bytes).unwrap();

    let summary = ingest(
        truncated_path.to_str().unwrap().to_string(),
        Some(1),
        true,
        None,
    )
    .await;

    assert_eq!(summary.total, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(
        summary.stage_failures.get(&IngestStage::MetadataExtraction),
        Some(&1)
    );
}

#[tokio::test]
async fn test_non_raw_image_fails_at_thumbnail_extraction() {
    let sample_raw = PathBuf::from("/home/kaneki/suisai-test/original_raws/_DSC0326.ARW");
    if !sample_raw.exists() {
        eprintln!("Skipping test: test raw not found");
        return;
    }

    let dir = tempdir().unwrap();
    let fake_raw_path = dir.path().join("fake_sensor.ARW");

    let status = std::process::Command::new("exiftool")
        .args(["-b", "-ThumbnailImage", sample_raw.to_str().unwrap()])
        .output()
        .expect("Failed to execute exiftool");

    if status.stdout.is_empty() {
        eprintln!("Skipping: could not extract thumbnail image");
        return;
    }

    fs::write(&fake_raw_path, &status.stdout).unwrap();

    let summary = ingest(
        fake_raw_path.to_str().unwrap().to_string(),
        Some(1),
        true,
        None,
    )
    .await;

    assert_eq!(summary.total, 1);
    assert_eq!(summary.failed, 1);
    assert_eq!(
        summary
            .stage_failures
            .get(&IngestStage::ThumbnailExtraction),
        Some(&1)
    );
}

#[tokio::test]
async fn test_valid_camera_raw_passes_ingest() {
    let sample_raw = PathBuf::from("/home/kaneki/suisai-test/original_raws/_DSC0326.ARW");
    if !sample_raw.exists() {
        eprintln!("Skipping test: test raw not found");
        return;
    }

    let summary = ingest(
        sample_raw.to_str().unwrap().to_string(),
        Some(1),
        true,
        None,
    )
    .await;

    assert_eq!(summary.total, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
}

#[tokio::test]
async fn test_helpers_hash_and_thumbnail() {
    let sample_raw = PathBuf::from("/home/kaneki/suisai-test/original_raws/_DSC0326.ARW");
    if !sample_raw.exists() {
        eprintln!("Skipping test: test raw not found");
        return;
    }

    // Test hash_and_read
    let (hash, bytes) = hash_and_read(&sample_raw).await.unwrap();
    assert!(!hash.is_empty());
    assert_eq!(bytes, 24902400);

    // Test SuisaiAsset trait methods
    let asset = sample_raw.to_db_entry(hash.clone(), (bytes / 1024) as i64);
    assert_eq!(asset.camera_model, "ILCE-9");
    assert_eq!(asset.resolution_width, 6048);
    assert_eq!(asset.resolution_height, 4024);

    let try_asset = sample_raw
        .try_to_db_entry(hash, (bytes / 1024) as i64)
        .unwrap();
    assert_eq!(try_asset.camera_model, "ILCE-9");

    // Test thumbnail generation
    let dir = tempdir().unwrap();
    let thumb_dest = dir.path().join("thumb.jpeg");
    extract_thumbnail(&sample_raw, &thumb_dest).unwrap();
    assert!(thumb_dest.exists());

    // Test in-memory thumbnail decode
    test_extract_thumbnail(&sample_raw, None, true).unwrap();
}

#[tokio::test]
async fn test_search_path_for_assets() {
    let dir = tempdir().unwrap();
    let raw1 = dir.path().join("photo1.ARW");
    let non_raw = dir.path().join("notes.txt");
    File::create(&raw1).unwrap();
    File::create(&non_raw).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    let dir_path = dir.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        search_path_for_assets(&dir_path, &tx).unwrap();
        drop(tx);
    })
    .await
    .unwrap();

    let mut found = Vec::new();
    while let Some((path, _rel)) = rx.recv().await {
        found.push(path);
    }

    assert_eq!(found.len(), 1);
    assert_eq!(found[0], raw1);
}
