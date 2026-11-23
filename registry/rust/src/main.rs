use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Serve the home lab registry")]
struct Args {
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    lab_registry::serve(Args::parse().config).await
}
