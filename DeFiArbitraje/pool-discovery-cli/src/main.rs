mod config;
mod discover;

use clap::Parser;
use anyhow::Result;
use tracing::{info, error};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "pool-discovery-cli")]
#[command(about = "Поиск пулов v2/v3/solidly по JSON-конфигу и выгрузка reserves/liquidity", long_about = None)]
struct Args {
    /// Путь к конфигу
    #[arg(long, default_value = "/mnt/data/defi_config.json")]
    config: String,

    /// Путь к выходному JSON
    #[arg(long, default_value = "/mnt/data/pools.generated.json")]
    out: String,

    /// Максимум одновременных RPC задач
    #[arg(long, default_value_t = 32)]
    concurrency: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    info!("Загрузка конфига из {}", args.config);
    let cfg = config::Config::load(&args.config)?;

    let out = discover::run_discovery(cfg, args.concurrency).await?;

    std::fs::write(&args.out, serde_json::to_string_pretty(&out)?)?;
    info!("Готово: {}", &args.out);
    Ok(())
}
