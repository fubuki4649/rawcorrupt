use crate::ingest::main::ingest;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "corrupt",
    version,
    about = "Camera RAW corruption tester using suisai ingestion and thumbnailing pipeline",
    long_about = "Input a directory (or single camera RAW file) to automatically verify each file through suisai's exact pipeline, reporting which stage failed on any corruption."
)]
pub struct Cli {
    /// Path to a directory containing camera raws (or a single camera raw file)
    #[arg(help = "Path to a directory containing camera raws")]
    pub source: String,

    /// Number of worker threads for parallel verification (default: CPU cores)
    #[arg(short = 't', long = "threads")]
    pub threads: Option<usize>,

    /// Fast half-size demosaic decoding instead of full-size
    #[arg(long = "half-size", default_value_t = false)]
    pub half_size: bool,

    /// Optional directory to save successfully generated JPEG thumbnails
    #[arg(long = "save-thumbnails", value_name = "DIR")]
    pub save_thumbnails: Option<PathBuf>,
}

pub async fn run_cli() {
    let cli = Cli::parse();
    let summary = ingest(cli.source, cli.threads, cli.half_size, cli.save_thumbnails).await;

    if summary.failed > 0 {
        std::process::exit(1);
    }
}
