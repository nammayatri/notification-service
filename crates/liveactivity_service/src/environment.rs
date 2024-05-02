use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Deserialize, Debug)]
pub struct RequestBody {
    pub aps: Aps,
}

#[derive(Deserialize, Debug)]
pub struct Aps {
    pub content_state: HashMap<String, String>,
    pub alert: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Debug)]
pub struct ResponseJson {
    pub aps: ApsResponse,
}

#[derive(Serialize, Debug)]
pub struct ApsResponse {
    pub timestamp: u64,
    pub event: String,
    pub content_state: HashMap<String, String>,
    pub alert: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct AppConfig {
    pub apns_url: String,
    pub apns_port: String,
    pub port: u16,
}
