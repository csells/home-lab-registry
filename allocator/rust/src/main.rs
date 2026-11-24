use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Run the home-lab lease allocator.")]
struct Options {
    configuration: PathBuf,
    #[arg(long)]
    enroll_ledger: bool,
    #[arg(long, conflicts_with_all = ["enroll_ledger", "enroll_handoff_authority"])]
    handoff_ledger: Option<PathBuf>,
    #[arg(long, conflicts_with = "enroll_ledger")]
    enroll_handoff_authority: bool,
    /// Accept a verified operator start on a stopped Proxmox lease and queue a fresh export.
    #[arg(long, conflicts_with_all = ["enroll_ledger", "handoff_ledger", "enroll_handoff_authority"])]
    reconcile_final_export: Option<PathBuf>,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let options = Options::parse();
    if let Some(proof) = options.reconcile_final_export {
        let receipt = tokio::task::spawn_blocking(move || {
            lab_allocator::reconcile_final_export(&options.configuration, &proof)
        })
        .await??;
        println!("{receipt}");
        return Ok(());
    }
    if let Some(authorization) = options.handoff_ledger {
        tokio::task::spawn_blocking(move || {
            lab_allocator::handoff(&options.configuration, &authorization)
        })
        .await??;
        println!("The authorized replacement generation owns the current ledger.");
        return Ok(());
    }
    if options.enroll_ledger {
        tokio::task::spawn_blocking(move || lab_allocator::enroll(&options.configuration))
            .await??;
        println!("Idle slot and independent ledger witness enrolled.");
        return Ok(());
    }
    if options.enroll_handoff_authority {
        tokio::task::spawn_blocking(move || {
            lab_allocator::enroll_handoff_authority(&options.configuration)
        })
        .await??;
        println!("The current ledger has an enrolled handoff authority.");
        return Ok(());
    }
    lab_allocator::http::serve(options.configuration).await
}
