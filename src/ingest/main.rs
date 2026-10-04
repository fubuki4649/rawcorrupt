use crate::ingest::helpers::extract_thumbnail::test_extract_thumbnail;
use crate::ingest::helpers::hash_and_transfer::hash_and_read;
use crate::ingest::helpers::search_path::{is_image_by_infer, search_path_for_assets};
use crate::ingest::stage::IngestStage;
use crate::ingest::traits::SuisaiAsset;
use crate::models::asset::NewDbAsset;
use colored::*;
use std::collections::HashMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

/// Summary statistics for the corruption verification run.
#[derive(Debug, Default, Clone)]
pub struct IngestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub total_bytes: u64,
    pub stage_failures: HashMap<IngestStage, usize>,
    pub duration_ms: u64,
}

/// Runs the camera RAW corruption tester on a path (directory or file)
/// using suisai's exact ingestion and thumbnailing pipeline.
pub async fn ingest(
    path: String,
    threads: Option<usize>,
    half_size: bool,
    save_thumbnails: Option<PathBuf>,
) -> IngestSummary {
    let target = PathBuf::from(&path);
    let start_time = Instant::now();

    let (tx, rx) = tokio::sync::mpsc::channel::<(PathBuf, PathBuf)>(100);
    let shared_rx = Arc::new(Mutex::new(rx));

    // Handle single file input or directory traversal
    if target.is_file() {
        let parent = target.parent().unwrap_or(Path::new(""));
        let rel = target.strip_prefix(parent).unwrap_or(&target).to_path_buf();
        tx.send((target.clone(), rel)).await.ok();
        drop(tx);
    } else {
        // Launch producer to probe for camera raw assets matching suisai
        let scan_path = path.clone();
        tokio::task::spawn_blocking(move || {
            println!("Testing camera RAW files from: {scan_path}");
            search_path_for_assets(&PathBuf::from(scan_path), &tx).unwrap_or_else(|err| {
                eprintln!("Error searching for assets: {err}");
            });
            drop(tx);
        });
    }

    let available_threads = threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .unwrap_or(NonZero::new(8).unwrap())
            .get()
    });

    let mut workers = JoinSet::new();
    println!("Starting verification with {available_threads} threads");

    let summary = Arc::new(Mutex::new(IngestSummary::default()));

    for _ in 0..available_threads {
        let rx = shared_rx.clone();
        let summary = summary.clone();
        let save_thumbnails = save_thumbnails.clone();

        workers.spawn(async move {
            loop {
                let (path, _rel_path) = {
                    let mut guard = rx.lock().await;
                    match guard.recv().await {
                        Some(item) => item,
                        None => break,
                    }
                };

                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let mut failed_stage: Option<IngestStage> = None;
                let mut error_msg: Option<String> = None;
                let mut computed_hash: Option<String> = None;
                let mut file_bytes: u64 = 0;
                let mut extracted_asset: Option<NewDbAsset> = None;

                // Stage 1: Search & Type Check (mirrors suisai search_path_for_assets infer check)
                if !is_image_by_infer(&path) {
                    failed_stage = Some(IngestStage::SearchPath);
                    error_msg = Some(
                        "File magic bytes not recognized as Image format by infer".to_string(),
                    );
                }

                // Stage 2: Hash & Read (mirrors suisai hash_and_transfer)
                if failed_stage.is_none() {
                    match hash_and_read(&path).await {
                        Ok((h, bytes)) => {
                            computed_hash = Some(h);
                            file_bytes = bytes;
                        }
                        Err(e) => {
                            failed_stage = Some(IngestStage::HashAndRead);
                            error_msg = Some(format!("Error reading/streaming file: {e}"));
                        }
                    }
                }

                // Stage 3: Metadata Extraction (mirrors suisai SuisaiAsset::to_db_entry)
                if failed_stage.is_none() {
                    let path_clone = path.clone();
                    let hash_clone = computed_hash.clone().unwrap_or_default();
                    let size_on_disk = file_bytes.div_ceil(1024) as i64;

                    let meta_res = tokio::task::spawn_blocking(move || {
                        path_clone.try_to_db_entry(hash_clone, size_on_disk)
                    })
                    .await
                    .unwrap();

                    match meta_res {
                        Ok(asset) => {
                            extracted_asset = Some(asset);
                        }
                        Err(e) => {
                            failed_stage = Some(IngestStage::MetadataExtraction);
                            error_msg = Some(format!("Error extracting EXIF metadata: {e}"));
                        }
                    }
                }

                // Stage 4: Thumbnail Extraction & RAW Sensor Decode (mirrors suisai extract_thumbnail)
                if failed_stage.is_none() {
                    let path_clone = path.clone();
                    let dest_thumb = save_thumbnails.as_ref().map(|dir| {
                        let stem = path_clone.file_stem().unwrap_or_default().to_string_lossy();
                        dir.join(format!("{stem}.jpeg"))
                    });

                    let thumb_res = tokio::task::spawn_blocking(move || {
                        test_extract_thumbnail(&path_clone, dest_thumb.as_deref(), half_size)
                    })
                    .await
                    .unwrap();

                    if let Err(e) = thumb_res {
                        failed_stage = Some(IngestStage::ThumbnailExtraction);
                        error_msg =
                            Some(format!("Error decoding RAW and generating thumbnail: {e}"));
                    }
                }

                // Record outcome
                let mut s = summary.lock().await;
                s.total += 1;
                s.total_bytes += file_bytes;

                if let Some(stage) = failed_stage {
                    s.failed += 1;
                    *s.stage_failures.entry(stage).or_insert(0) += 1;

                    eprintln!();
                    eprintln!(
                        "{} {}",
                        "[FAIL]".white().on_red().bold(),
                        path.display().to_string().bold()
                    );
                    eprintln!(
                        "  {} [Stage {}] {} ({})",
                        "Failed Stage:".red().bold(),
                        stage.step_number(),
                        stage.name().yellow().bold(),
                        stage.description()
                    );
                    if let Some(err) = error_msg {
                        eprintln!("  {} {}", "Error:       ".red().bold(), err);
                    }
                    if let Some(h) = computed_hash {
                        eprintln!("  {} {}", "xxh3 Hash:   ".cyan(), h);
                    }
                    if file_bytes > 0 {
                        eprintln!(
                            "  {} {} bytes ({:.2} MB)",
                            "File Size:   ".cyan(),
                            file_bytes,
                            file_bytes as f64 / (1024.0 * 1024.0)
                        );
                    }

                    if let Some(asset) = extracted_asset {
                        eprintln!("  {}", "Extracted Metadata Prior to Failure:".cyan().bold());
                        eprintln!("    Camera Model: {}", asset.camera_model);
                        eprintln!("    Lens Model:   {}", asset.lens_model);
                        eprintln!("    Photo Date:   {}", asset.photo_date);
                        eprintln!(
                            "    Resolution:   {}x{}",
                            asset.resolution_width, asset.resolution_height
                        );
                        eprintln!("    ISO:          {}", asset.iso);
                        eprintln!("    Shutter:      {}", asset.shutter_speed);
                        eprintln!("    Aperture:     f/{:.1}", asset.aperture);
                    }
                    eprintln!();
                } else {
                    s.passed += 1;
                    let meta_info = extracted_asset.as_ref().map_or_else(String::new, |a| {
                        format!(
                            " | {} | {}x{} | ISO {}",
                            a.camera_model, a.resolution_width, a.resolution_height, a.iso
                        )
                    });
                    println!(
                        "{} {} ({:.2} MB){}",
                        "[PASS]".green().bold(),
                        filename,
                        file_bytes as f64 / (1024.0 * 1024.0),
                        meta_info
                    );
                }
            }
        });
    }

    workers.join_all().await;

    let mut final_summary = Arc::try_unwrap(summary).unwrap().into_inner();
    final_summary.duration_ms = start_time.elapsed().as_millis() as u64;

    print_ingest_summary(&final_summary);
    final_summary
}

fn print_ingest_summary(summary: &IngestSummary) {
    println!("{}", "=".repeat(60).cyan());
    println!(
        "{}",
        "            CAMERA RAW VERIFICATION SUMMARY            ".bold()
    );
    println!("{}", "=".repeat(60).cyan());

    println!(
        "Total Files Scanned:   {}",
        summary.total.to_string().bold()
    );
    println!(
        "Total Passed (Valid):  {}",
        summary.passed.to_string().green().bold()
    );

    if summary.failed > 0 {
        println!(
            "Total Failed (Corrupt): {}",
            summary.failed.to_string().red().bold()
        );
    } else {
        println!("Total Failed (Corrupt): 0");
    }

    println!(
        "Total Data Processed:  {:.2} MB",
        summary.total_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "Total Time Elapsed:    {:.2}s",
        summary.duration_ms as f64 / 1000.0
    );

    if summary.duration_ms > 0 {
        let files_per_sec = (summary.total as f64) / (summary.duration_ms as f64 / 1000.0);
        let mb_per_sec = (summary.total_bytes as f64 / (1024.0 * 1024.0))
            / (summary.duration_ms as f64 / 1000.0);
        println!(
            "Throughput:            {:.1} files/s ({:.2} MB/s)",
            files_per_sec, mb_per_sec
        );
    }

    println!("{}", "-".repeat(60).cyan());
    println!("{}", "Stage Failure Breakdown:".bold());

    for stage in &IngestStage::ALL {
        let count = summary.stage_failures.get(stage).copied().unwrap_or(0);
        let line = format!(
            "  [Stage {}] {:<26} : {}",
            stage.step_number(),
            stage.name(),
            count
        );
        if count > 0 {
            println!("{}", line.red().bold());
        } else {
            println!("{}", line.dimmed());
        }
    }

    println!("{}", "=".repeat(60).cyan());

    if summary.failed == 0 {
        println!(
            "{}",
            "✓ All camera RAW files passed verification cleanly!"
                .green()
                .bold()
        );
    } else {
        println!(
            "{}",
            format!("✗ Found {} corrupted camera RAW file(s)!", summary.failed)
                .red()
                .bold()
        );
    }
    println!();
}
