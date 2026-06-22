//! `dl-pipeline` CLI (DAM-46).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use dl_pipeline::warehouse::{JsonlWarehouse, Warehouse, WarehouseConfig};
use dl_pipeline::{
    ingest_cycle_v1, reconcile, DatePartition, PipelineError, PipelineRunId, DEFAULT_WAREHOUSE_ROOT,
};

#[derive(Parser, Debug)]
#[command(name = "dl-pipeline", about = "Damascus Laundry data pipeline (DAM-46)")]
struct Cli {
    /// Override the warehouse root. Default: `./data/warehouse`.
    #[arg(long, global = true, default_value = DEFAULT_WAREHOUSE_ROOT)]
    root: PathBuf,

    /// Run against a per-process temp warehouse. Test mode never mutates
    /// the production warehouse. Required for the CI fixture tests.
    #[arg(long, global = true)]
    test_mode: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Ingest JSONL files into a partitioned table.
    Ingest {
        #[command(subcommand)]
        what: IngestWhat,
    },
    /// Run daily reconciliation: join cycle_v1 to recon_report_v1 by
    /// cycle_id and write daily_recon_v1 for the day.
    Reconcile(CommonArgs),
    /// Verify the day's batch: replay the partition, recompute the
    /// blake3 checksum, compare against the stored one. Exits 0 on
    /// match, 1 on mismatch.
    Verify(CommonArgs),
    /// Archive partitions older than N days to data/archive/.
    Compact {
        /// Archive partitions whose date is older than N days.
        #[arg(long, default_value_t = 90)]
        older_than_days: u64,
    },
}

#[derive(Subcommand, Debug)]
enum IngestWhat {
    Cycles(IngestArgs),
    Trades(IngestArgs),
}

#[derive(Args, Debug, Clone)]
struct IngestArgs {
    /// File or directory of `.jsonl` files to ingest.
    path: PathBuf,
}

#[derive(Args, Debug, Clone)]
struct CommonArgs {
    /// Date (UTC) to operate on, in YYYY-MM-DD form.
    #[arg(long)]
    date: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dl-pipeline: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), PipelineError> {
    let run = PipelineRunId::new();
    let config = if cli.test_mode {
        eprintln!("[test-mode] using per-process temp warehouse");
        WarehouseConfig::new(std::env::temp_dir().join(format!(
            "dl-pipeline-cli-test-{}-{}",
            std::process::id(),
            run
        )))
    } else {
        WarehouseConfig::new(cli.root.clone())
    };
    let mut warehouse = JsonlWarehouse::open(config)?;

    match cli.cmd {
        Cmd::Ingest { what } => match what {
            IngestWhat::Cycles(args) => {
                let stats = ingest_cycle_v1(&mut warehouse, &args.path, &run)?;
                println!("ingest cycles: {}", stats);
                Ok(())
            }
            IngestWhat::Trades(args) => {
                let stats = dl_pipeline::ingest_trade_v1(&mut warehouse, &args.path, &run)?;
                println!("ingest trades: {}", stats);
                Ok(())
            }
        },
        Cmd::Reconcile(args) => {
            let date = DatePartition::parse(&args.date)?;
            let row = reconcile(&mut warehouse, &date, &run)?;
            println!("reconcile: {}", serde_json::to_string(&row).unwrap());
            Ok(())
        }
        Cmd::Verify(args) => {
            let date = DatePartition::parse(&args.date)?;
            let (live, stored) = warehouse.verify_partition(date.as_str())?;
            match stored {
                None => {
                    println!(
                        "verify {}: no stored checksum. live: rows={} blake3={}",
                        date, live.row_count, live.row_set_blake3
                    );
                    eprintln!(
                        "verify {}: warning — no prior checksum to compare; the partition has not been sealed",
                        date
                    );
                    Ok(())
                }
                Some(s) if s.row_set_blake3 == live.row_set_blake3 && s.row_count == live.row_count => {
                    println!(
                        "verify {}: OK rows={} blake3={}",
                        date, live.row_count, live.row_set_blake3
                    );
                    Ok(())
                }
                Some(s) => {
                    eprintln!(
                        "verify {}: MISMATCH live(rows={}, blake3={}) != stored(rows={}, blake3={})",
                        date, live.row_count, live.row_set_blake3, s.row_count, s.row_set_blake3
                    );
                    Err(PipelineError::ChecksumMismatch {
                        date: date.to_string(),
                        recorded: s.row_set_blake3,
                        computed: live.row_set_blake3,
                    })
                }
            }
        }
        Cmd::Compact { older_than_days } => {
            let archived = warehouse.compact(older_than_days, run.0.as_str())?;
            println!(
                "compact: archived {} partition(s) older than {} days: {:?}",
                archived.len(),
                older_than_days,
                archived
            );
            Ok(())
        }
    }
}
