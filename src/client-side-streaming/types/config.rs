use serde::{Deserialize};

#[derive(Debug, Deserialize, Clone, Default)]

#[serde(default)]
pub struct ServerConfig {
    pub host_ip: String,
    pub port: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ApiConfig {
    pub base_url: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub api: ApiConfig,
}