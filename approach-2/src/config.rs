use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub service: ServiceConfig,
    pub grpc: GrpcConfig,
    pub http: HttpConfig,
    pub broker: BrokerConfig,
    pub client: ClientConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceConfig {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GrpcConfig {
    pub address: String,
    pub ping_interval_secs: u64,
    pub ping_timeout_secs: u64,
    pub enable_reflection: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BrokerConfig {
    pub kind: String,
    pub address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClientConfig {
    pub header: String,
    pub device_header: String,
    pub enable_device_support: bool,
    pub device_attribute_headers: Vec<String>,
}

impl AppConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;

        let cfg: AppConfig = settings.try_deserialize()?;
        Ok(cfg)
    }
}
