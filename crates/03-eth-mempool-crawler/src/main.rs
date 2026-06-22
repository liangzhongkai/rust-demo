use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "eth-mempool-crawler",
    about = "Case study: observe Ethereum mempool via P2P (mock pipeline)",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the mock pipeline (default) — no network or database required
    Mock {
        /// How long to simulate mempool activity (seconds)
        #[arg(long, default_value_t = 10)]
        duration: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("eth_mempool_crawler=info")
        }))
        .init();

    let cli = Cli::parse();
    let duration = match cli.command {
        Some(Commands::Mock { duration }) => duration,
        None => 10,
    };

    println!("Running mock mempool pipeline ({duration}s)...");
    println!("For live mainnet P2P, see reference/ and upstream repo.\n");
    eth_mempool_crawler::run_mock_crawler(duration).await?;

    Ok(())
}
