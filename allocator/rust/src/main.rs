use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Run the home-lab lease allocator.")]
struct Options {
    configuration: PathBuf,
    #[arg(long, conflicts_with = "provider_adapter")]
    enroll_ledger: bool,
    #[arg(long, hide = true)]
    provider_adapter: Option<PathBuf>,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let options = Options::parse();
    if options.enroll_ledger {
        tokio::task::spawn_blocking(move || lab_allocator::enroll(&options.configuration))
            .await??;
        println!("Idle slot and independent ledger witness enrolled.");
        return Ok(());
    }
    lab_allocator::http::serve(options.configuration, options.provider_adapter).await
}
