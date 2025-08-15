use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use lazy_static::lazy_static;
use prometheus::{
    Counter, CounterVec, GaugeVec, IntCounter, IntGauge, TextEncoder, register_counter,
    register_counter_vec, register_gauge_vec, register_int_counter, register_int_gauge,
};
use std::convert::Infallible;

lazy_static! {
    pub static ref METRIC_ROUTES_SCANNED: IntCounter = register_int_counter!(
        "routes_scanned_total",
        "Total number of routes scanned"
    ).expect("register routes_scanned_total");

    pub static ref METRIC_PROFITABLE_FOUND: IntCounter = register_int_counter!(
        "profitable_routes_found_total",
        "Total profitable routes found"
    ).expect("register profitable_routes_found_total");

    pub static ref METRIC_TX_SENT: IntCounter = register_int_counter!(
        "tx_sent_total",
        "Total transactions submitted"
    ).expect("register tx_sent_total");

    /// Целое значение; храните PnL*100 (или *10000) — как решите в коде.
    pub static ref METRIC_PNL_USD: IntGauge = register_int_gauge!(
        "pnl_usd_total",
        "Cumulative PnL in USD (scaled integer)"
    ).expect("register pnl_usd_total");

    /// Аптайм/простой health-гейдж: 1 = OK, 0 = starting/issue
    pub static ref METRIC_HEALTH: IntGauge = register_int_gauge!(
        "service_health",
        "Service health indicator (1=OK)"
    ).expect("register service_health");

    /// Для наглядности — «последний скрейп» в unix-милисекундах
    pub static ref METRIC_LAST_SCRAPE_MS: IntGauge = register_int_gauge!(
        "metrics_last_scrape_ms",
        "Last /metrics scrape time (unix ms)"
    ).expect("register metrics_last_scrape_ms");

    pub static ref METRIC_OPPS_FOUND: Counter = register_counter!(
        "opportunities_found_total",
        "Total quote opportunities found",
    ).expect("register opportunities_found_total");

    pub static ref METRIC_BEST_PNL_USD: GaugeVec = register_gauge_vec!(
        "best_pnl_usd",
        "Best PnL in USD by chain",
        & ["chain"]
    ).expect("register best_pnl_usd");

    pub static ref METRIC_LAST_SIM_GAS: GaugeVec = register_gauge_vec!(
        "last_sim_gas",
        "Last gas estimate from simulation by chain",
        & ["chain"]
    ).expect("register last_sim_gas");

    pub static ref METRIC_EXEC_OK: CounterVec = register_counter_vec!(
        "exec_success_total",
        "Total successful executions by chain",
        & ["chain"]
    ).expect("register exec_success_total");

    pub static ref METRIC_EXEC_FAIL: CounterVec = register_counter_vec!(
        "exec_fail_total",
        "Total failed executions by chain",
        & ["chain"]
    ).expect("register exec_fail_total");
}

/// HTTP-хендлер: роутим /metrics и /healthz
async fn http_handler(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    match req.uri().path() {
        "/metrics" => metrics_response().await,
        "/healthz" => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("ok"))
            .unwrap()),
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("not found"))
            .unwrap()),
    }
}

async fn metrics_response() -> Result<Response<Body>, Infallible> {
    // Проставим «здоровье» и отметим момент скрейпа:
    METRIC_HEALTH.set(1);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    METRIC_LAST_SCRAPE_MS.set(now_ms);

    // Сериализация реестра в текст OpenMetrics/Prometheus:
    let encoder = TextEncoder::new();
    let body = match encoder.encode_to_string(&prometheus::gather()) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("encode metrics error: {e}");
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from(msg))
                .unwrap());
        }
    };

    Ok(Response::builder()
        // Рекомендуемый заголовок для text exposition format:
        // https://prometheus.io/docs/guides/utf8/
        .header(
            hyper::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .status(StatusCode::OK)
        .body(Body::from(body))
        .unwrap())
}

/// Поднимаем отдельный HTTP-сервер метрик.
/// Вызывается из main: `tokio::spawn(serve_metrics(port));`
pub async fn serve_metrics(port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = ([0, 0, 0, 0], port).into();
    let make_svc = make_service_fn(|_| async { Ok::<_, Infallible>(service_fn(http_handler)) });
    let server = Server::bind(&addr).serve(make_svc);

    tracing::info!("Prometheus /metrics on http://0.0.0.0:{port}/metrics  (/healthz too)");
    server.await?;
    Ok(())
}
