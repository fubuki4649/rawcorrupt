use crate::ingest::failure::{CorruptedFile, FailureCategory, FailureDetail};
use crate::ingest::helpers::extract_thumbnail::{
    detect_dcraw_emu_version, libraw_version_string, test_extract_thumbnail,
};
use crate::ingest::helpers::hash_and_transfer::hash_and_read;
use crate::ingest::helpers::search_path::{is_image_by_infer, search_path_for_assets};
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
    pub failure_breakdown: HashMap<FailureCategory, usize>,
    pub corrupted_files: Vec<CorruptedFile>,
    pub duration_ms: u64,
}

/// Runs the camera RAW corruption tester on a path (directory or file)
/// using suisai's exact ingestion and thumbnailing pipeline.
pub async fn ingest(
    path: String,
    threads: Option<usize>,
    half_size: bool,
    save_thumbnails: Option<PathBuf>,
    log_file: Option<PathBuf>,
    diagnostics: bool,
) -> IngestSummary {
    let target = PathBuf::from(&path);
    let start_time = Instant::now();
    let is_single_file = target.is_file();

    let tool_version = libraw_version_string();
    let dcraw_version = detect_dcraw_emu_version();

    println!("{}", "=".repeat(60).cyan());
    println!(
        "{}",
        "               LIBRAW ENVIRONMENT & DIAGNOSTICS               ".bold()
    );
    println!("{}", "=".repeat(60).cyan());
    println!("  Tool LibRaw Version:     {}", tool_version.green().bold());
    match dcraw_version {
        Some(ref sys_ver) => {
            if tool_version.starts_with(sys_ver) || sys_ver.starts_with(&tool_version) {
                println!(
                    "  System dcraw_emu:        {} {}",
                    sys_ver.green().bold(),
                    format!("(matches tool: {tool_version})").dimmed()
                );
            } else {
                println!(
                    "  System dcraw_emu:        {} {}",
                    sys_ver.yellow().bold(),
                    format!("(VERSION MISMATCH vs tool: {tool_version})")
                        .red()
                        .bold()
                );
            }
        }
        None => {
            println!(
                "  System dcraw_emu:        {}",
                "not found in PATH".yellow()
            );
        }
    }
    println!(
        "  Path Input Mode:         {}",
        "libraw_open_file (direct file path, no buffer/stream)".cyan()
    );
    println!("{}", "=".repeat(60).cyan());
    println!();

    let (tx, rx) = tokio::sync::mpsc::channel::<(PathBuf, PathBuf)>(100);
    let shared_rx = Arc::new(Mutex::new(rx));

    // Handle single file input or directory traversal
    if is_single_file {
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

    // When running parallel workers across multiple files, limit OpenMP per-file
    // internal demosaicing threads to 1 to prevent severe CPU oversubscription.
    if std::env::var_os("OMP_NUM_THREADS").is_none() && available_threads > 1 {
        // SAFETY: Set before any LibRaw processing begins.
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "1");
        }
    }

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

                let mut failure: Option<FailureDetail> = None;
                let mut computed_hash: Option<String> = None;
                let mut file_bytes: u64 = 0;
                let mut extracted_asset: Option<NewDbAsset> = None;
                let mut libraw_diag = None;

                // Step 1: Search & Type Check (run off-thread to avoid blocking Tokio reactor)
                let path_for_infer = path.clone();
                let is_image =
                    tokio::task::spawn_blocking(move || is_image_by_infer(&path_for_infer))
                        .await
                        .unwrap_or(false);

                if !is_image {
                    failure = Some(FailureDetail::file_read(
                        "File header magic bytes not recognized as valid image format by infer",
                    ));
                }

                // Step 2: Hash & Read (mirrors suisai hash_and_transfer streaming)
                if failure.is_none() {
                    match hash_and_read(&path).await {
                        Ok((h, bytes)) => {
                            computed_hash = Some(h);
                            file_bytes = bytes;
                        }
                        Err(e) => {
                            failure = Some(FailureDetail::file_read(format!(
                                "Failed reading file from disk: {e}"
                            )));
                        }
                    }
                }

                // Step 3 & 4: Metadata Extraction & RAW Sensor Decode in a single blocking task (matching suisai)
                // This keeps the file hot in OS page cache and eliminates redundant context switching.
                if failure.is_none() {
                    let path_clone = path.clone();
                    let hash_clone = computed_hash.clone().unwrap_or_default();
                    let size_on_disk = file_bytes.div_ceil(1024) as i64;
                    let dest_thumb = save_thumbnails.as_ref().map(|dir| {
                        let stem = path_clone.file_stem().unwrap_or_default().to_string_lossy();
                        dir.join(format!("{stem}.jpeg"))
                    });

                    let (meta_res, thumb_res) = tokio::task::spawn_blocking(move || {
                        let meta = path_clone.try_to_db_entry(hash_clone, size_on_disk);
                        let thumb = meta.is_ok().then(|| {
                            test_extract_thumbnail(&path_clone, dest_thumb.as_deref(), half_size)
                        });
                        (meta, thumb)
                    })
                    .await
                    .unwrap();

                    match meta_res {
                        Ok(asset) => {
                            extracted_asset = Some(asset);
                            match thumb_res {
                                Some(Ok(diag)) => libraw_diag = Some(diag),
                                Some(Err(fail)) => {
                                    libraw_diag = fail.diagnostics.clone();
                                    failure = Some(fail);
                                }
                                None => {}
                            }
                        }
                        Err(fail) => failure = Some(fail),
                    }
                }

                // Record outcome into summary lock, releasing immediately before terminal I/O
                {
                    let mut s = summary.lock().await;
                    s.total += 1;
                    s.total_bytes += file_bytes;

                    if let Some(ref fail) = failure {
                        s.failed += 1;
                        *s.failure_breakdown.entry(fail.category).or_insert(0) += 1;
                        s.corrupted_files.push(CorruptedFile {
                            path: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
                            category: fail.category,
                            error: fail.message.clone(),
                            diagnostics: fail.diagnostics.clone(),
                        });
                    } else {
                        s.passed += 1;
                    }
                }

                // Print terminal output outside the summary lock so workers never stall each other
                if let Some(fail) = failure {
                    eprintln!(
                        "\n{} {}\n  {} {}\n  {} {}",
                        "[FAIL]".white().on_red().bold(),
                        path.display().to_string().bold(),
                        "Failure Reason:".red().bold(),
                        fail.category.name().yellow().bold(),
                        "Error:         ".red().bold(),
                        fail.message
                    );
                    if let Some(ref diag) = fail.diagnostics {
                        eprintln!("  {} {}", "LibRaw Route:  ".cyan().bold(), diag.open_route);
                        eprintln!(
                            "  {} {}",
                            "Return Codes:  ".cyan().bold(),
                            diag.status_summary()
                        );
                        if let Some(ref cb) = diag.data_callback {
                            eprintln!("  {} {}", "Data Callback: ".yellow().bold(), cb);
                        }
                    }
                    if let Some(h) = computed_hash {
                        eprintln!("  {} {h}", "xxh3 Hash:     ".cyan());
                    }
                    if file_bytes > 0 {
                        eprintln!(
                            "  {} {file_bytes} bytes ({:.2} MB)",
                            "File Size:     ".cyan(),
                            file_bytes as f64 / (1024.0 * 1024.0)
                        );
                    }
                    if let Some(a) = extracted_asset {
                        eprintln!(
                            "  {}\n    Camera Model: {}\n    Lens Model:   {}\n    Photo Date:   {}\n    Resolution:   {}x{}\n    ISO:          {}\n    Shutter:      {}\n    Aperture:     f/{:.1}",
                            "Extracted Metadata Prior to Failure:".cyan().bold(),
                            a.camera_model, a.lens_model, a.photo_date,
                            a.resolution_width, a.resolution_height,
                            a.iso, a.shutter_speed, a.aperture
                        );
                    }
                    eprintln!();
                } else {
                    let meta_info = extracted_asset.as_ref().map_or_else(String::new, |a| {
                        format!(
                            " | {} | {}x{} | ISO {}",
                            a.camera_model, a.resolution_width, a.resolution_height, a.iso
                        )
                    });
                    if is_single_file || diagnostics {
                        println!(
                            "{} {} ({:.2} MB){}",
                            "[PASS]".green().bold(),
                            filename,
                            file_bytes as f64 / (1024.0 * 1024.0),
                            meta_info
                        );
                        if let Some(ref h) = computed_hash {
                            println!("  {} {h}", "xxh3 Hash:     ".cyan());
                        }
                        if let Some(ref diag) = libraw_diag {
                            println!("  {} {}", "LibRaw Route:  ".cyan(), diag.open_route);
                            println!("  {} {}", "Return Codes:  ".cyan(), diag.status_summary());
                        }
                    } else {
                        let hash_info = computed_hash.as_ref().map_or_else(String::new, |h| {
                            format!(" | xxh3: {h}")
                        });
                        println!(
                            "{} {} ({:.2} MB){}{}",
                            "[PASS]".green().bold(),
                            filename,
                            file_bytes as f64 / (1024.0 * 1024.0),
                            hash_info.cyan(),
                            meta_info
                        );
                    }
                }
            }
        });
    }

    workers.join_all().await;

    let mut final_summary = Arc::try_unwrap(summary).unwrap().into_inner();
    final_summary.duration_ms = start_time.elapsed().as_millis() as u64;
    final_summary
        .corrupted_files
        .sort_by(|a, b| a.path.cmp(&b.path));

    print_ingest_summary(&final_summary);

    if let Some(ref log_path) = log_file
        && !final_summary.corrupted_files.is_empty()
    {
        let content: String = final_summary
            .corrupted_files
            .iter()
            .map(|item| format!("{}\n", item.path.display()))
            .collect();
        match tokio::fs::write(log_path, content).await {
            Ok(()) => println!(
                "{} Exported {} corrupted file path(s) to {}\n",
                "✓".green().bold(),
                final_summary.corrupted_files.len().to_string().bold(),
                log_path.display().to_string().yellow().bold()
            ),
            Err(e) => eprintln!(
                "{} Failed to write corrupted log to {}: {e}\n",
                "Warning:".yellow().bold(),
                log_path.display()
            ),
        }
    }

    final_summary
}

fn print_ingest_summary(summary: &IngestSummary) {
    let div = "=".repeat(60).cyan();
    println!(
        "{div}\n{}\n{div}",
        "            CAMERA RAW VERIFICATION SUMMARY            ".bold()
    );
    println!(
        "Total Files Scanned:   {}",
        summary.total.to_string().bold()
    );
    println!(
        "Total Passed (Valid):  {}",
        summary.passed.to_string().green().bold()
    );
    println!(
        "Total Failed (Corrupt): {}",
        if summary.failed > 0 {
            summary.failed.to_string().red().bold()
        } else {
            "0".normal()
        }
    );
    println!(
        "Total Data Processed:  {:.2} MB",
        summary.total_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "Total Time Elapsed:    {:.2}s",
        summary.duration_ms as f64 / 1000.0
    );

    if summary.duration_ms > 0 {
        let sec = summary.duration_ms as f64 / 1000.0;
        println!(
            "Throughput:            {:.1} files/s ({:.2} MB/s)",
            summary.total as f64 / sec,
            (summary.total_bytes as f64 / (1024.0 * 1024.0)) / sec
        );
    }

    println!("{}\n{}", "-".repeat(60).cyan(), "Failure Breakdown:".bold());
    for category in &FailureCategory::ALL {
        let count = summary
            .failure_breakdown
            .get(category)
            .copied()
            .unwrap_or(0);
        let line = format!("  {:<36} : {count}", category.name());
        println!(
            "{}",
            if count > 0 {
                line.red().bold()
            } else {
                line.dimmed()
            }
        );
    }
    println!("{div}");

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
