use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub service: ServiceConfig,
    pub grpc: GrpcConfig,
    pub http: HttpConfig,
    pub broker: BrokerConfig,
    pub client: ClientConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceConfig {
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub enabled: bool,
    pub legacy_header_mode: bool,
    pub tenant_claim: String,
    pub legacy_tenant: String,
    pub api_keys: Vec<String>,
    pub jwt: JwtConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JwtConfig {
    pub issuer: String,
    pub audience: String,
    pub clock_skew_secs: u64,
    pub hmac_secret: Option<String>,
    pub jwks_url: Option<String>,
}

fn default_tenant_claim() -> String {
    "tenant_id".to_string()
}

fn default_legacy_tenant() -> String {
    "legacy".to_string()
}

impl AppConfig {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;

        let cfg: AppConfig = settings.try_deserialize()?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if !self.auth.enabled {
            return Ok(());
        }

        if self.auth.jwt.issuer.is_empty() {
            anyhow::bail!("auth.jwt.issuer is required when auth is enabled");
        }
        if self.auth.jwt.audience.is_empty() {
            anyhow::bail!("auth.jwt.audience is required when auth is enabled");
        }
        if self.auth.tenant_claim.is_empty() {
            anyhow::bail!("auth.tenant_claim must not be empty");
        }
        if self.auth.jwt.hmac_secret.as_deref().unwrap_or("").is_empty() {
            anyhow::bail!("auth.jwt.hmac_secret is required in this build");
        }
        if self.auth.jwt.jwks_url.as_deref().is_some() {
            tracing::warn!("auth.jwt.jwks_url is configured but ignored in this build");
        }
        Ok(())
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            legacy_header_mode: true,
            tenant_claim: default_tenant_claim(),
            legacy_tenant: default_legacy_tenant(),
            api_keys: Vec::new(),
            jwt: JwtConfig::default(),
        }
    }
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            issuer: String::new(),
            audience: String::new(),
            clock_skew_secs: 30,
            hmac_secret: None,
            jwks_url: None,
        }
    }
}

impl<'de> Deserialize<'de> for AuthConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct AuthConfigRaw {
            enabled: Option<bool>,
            legacy_header_mode: Option<bool>,
            #[serde(default = "default_tenant_claim")]
            tenant_claim: String,
            #[serde(default = "default_legacy_tenant")]
            legacy_tenant: String,
            #[serde(default)]
            api_keys: Vec<String>,
            jwt: Option<JwtConfig>,
        }

        let raw = AuthConfigRaw::deserialize(deserializer)?;
        Ok(AuthConfig {
            enabled: raw.enabled.unwrap_or(true),
            legacy_header_mode: raw.legacy_header_mode.unwrap_or(true),
            tenant_claim: raw.tenant_claim,
            legacy_tenant: raw.legacy_tenant,
            api_keys: raw.api_keys,
            jwt: raw.jwt.unwrap_or_default(),
        })
    }
}
