use std::path::PathBuf;

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let mut args = std::env::args_os().skip(1);
    let config = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => PathBuf::from(path),
        _ => return Err("Usage: lab-registry --config <registry.json>".into()),
    };
    lab_registry::serve(config).await
}
