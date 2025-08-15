mod approvals;
mod calldata;
mod config;
mod dex;
mod error;
mod exec;
mod metrics;
mod mev;
mod network;
mod route;
mod router;
mod utils;

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::time::Duration;
use tracing::{error, info};

use crate::config::Config;
use crate::metrics::serve_metrics;
use crate::network::MultiChain;
use crate::route::{RoutePlanner, StrategyEngine};

#[tokio::main]
async fn main() -> Result<()> {
    // Логгер: уровень берём из RUST_LOG (пример: RUST_LOG=info,DeFiArbitraje=debug)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 1) Выбор пути к конфигу: ENV → argv → набор дефолтов (кроссплатформенно)
    let cfg_path = std::env::var("DEFI_CONFIG")
        .ok()
        .or_else(|| std::env::args().nth(1))
        .or_else(|| {
            let candidates = [
                ".\\config\\defi_config.json", // Windows
                "./config/defi_config.json",
                "config/defi_config.json",
                "/mnt/data/defi_config.json",
            ];
            candidates
                .iter()
                .find(|p| Path::new(p).exists())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| ".\\config\\defi_config.json".to_string());

    if !Path::new(&cfg_path).exists() {
        eprintln!(
            "⚠️ Конфиг не найден: {}\nЗапусти так: cargo run -p evm-arb-service -- .\\config\\defi_config.json\nили задай ENV DEFI_CONFIG",
            cfg_path
        );
        std::process::exit(1);
    }

    let cfg =
        Config::load(&cfg_path).with_context(|| format!("loading config from {}", cfg_path))?;
    info!(
        "Загружен конфиг: version={}, networks={}",
        cfg.version,
        cfg.networks.len()
    );

    // 2) Метрики (Prometheus)
    let prom_port = cfg.telemetry.prometheus.port;
    let metrics_handle = tokio::spawn(async move {
        if let Err(e) = serve_metrics(prom_port).await {
            eprintln!("metrics server error: {e:#}");
        }
    });

    // 3) Клиенты сетей
    let chains = Arc::new(MultiChain::from_config(&cfg).await?);
    info!("Инициализировано сетей: {}", chains.clients.len());

    // 4) Планировщик/движок
    let planner = Arc::new(RoutePlanner::from_config(&cfg));
    let mut engine = StrategyEngine::new(cfg.clone(), chains.clone(), planner.clone()).await?;

    let poll_ms = cfg.global.execution.poll_interval_ms as u64;

    // 5) Главный цикл + корректное завершение по сигналу
    loop {
        tokio::select! {
            // Работаем: сканируем и пытаемся исполнять
            _ = async {
                if let Err(e) = engine.scan_and_execute().await {
                    error!("Ошибка в scan_and_execute: {e:#}");
                }
                tokio::time::sleep(Duration::from_millis(poll_ms)).await;
            } => {},

            // Ждём сигнала остановки
            _ = shutdown_signal() => {
                info!("Получен сигнал завершения — выходим корректно");
                break;
            }
        }
    }

    // 6) Останавливем фоновую задачу метрик (если ещё живёт)
    metrics_handle.abort();

    Ok(())
}

/// Ожидание Ctrl+C (везде) + SIGTERM (на Unix).
async fn shutdown_signal() {
    // Всегда ждём Ctrl+C
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    // На Unix параллельно слушаем SIGTERM (service stop/restart и т.п.)
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let term = std::future::pending::<()>(); // на Windows ждём только Ctrl+C

    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
