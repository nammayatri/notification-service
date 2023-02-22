use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct APIRequest {
    pub pt: Location,
    pub ts: String,
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct APIResponse {
    pub _result: String,
}
