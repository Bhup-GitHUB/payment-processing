use metrics_exporter_prometheus::PrometheusBuilder;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}

pub fn init_prometheus() -> anyhow::Result<String> {
    let handle = PrometheusBuilder::new().install_recorder()?;
    Ok(handle.render())
}
