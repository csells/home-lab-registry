use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Run the home-lab lease allocator.")]
struct Options {
    configuration: PathBuf,
    #[arg(long)]
    enroll_ledger: bool,
    #[arg(long, conflicts_with = "enroll_ledger")]
    handoff_ledger: Option<PathBuf>,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let options = Options::parse();
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
    lab_allocator::http::serve(options.configuration).await
}
