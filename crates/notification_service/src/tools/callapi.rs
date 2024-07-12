/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::tools::prometheus::CALL_EXTERNAL_API;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Method, Response, Url,
};
use serde::{de::DeserializeOwned, Serialize};
use shared::call_external_api;
use std::{fmt::Debug, str::FromStr};
use tracing::{error, info};

#[macros::add_error]
pub enum CallApiError {
    HeaderSerializationFailed(String),
    ResponseDeserializationFailed(String),
    ConnectionError(String),
    BodySerializationFailed(String),
    ExternalAPICallError(Response),
}

pub async fn call_api<T, U>(
    method: Method,
    url: &Url,
    headers: Vec<(&str, &str)>,
    body: Option<U>,
) -> Result<T, CallApiError>
where
    T: DeserializeOwned,
    U: Serialize + Debug,
{
    let start_time = std::time::Instant::now();

    let client = Client::new();

    let mut header_map = HeaderMap::new();

    for (header_key, header_value) in headers {
        let header_name = HeaderName::from_str(header_key)
            .map_err(|err| CallApiError::HeaderSerializationFailed(err.to_string()))?;
        let header_value = HeaderValue::from_str(header_value)
            .map_err(|err| CallApiError::HeaderSerializationFailed(err.to_string()))?;

        header_map.insert(header_name, header_value);
    }

    let mut request = client
        .request(method.to_owned(), url.to_owned())
        .headers(header_map.to_owned());

    if let Some(body) = &body {
        let body = serde_json::to_string(body)
            .map_err(|err| CallApiError::BodySerializationFailed(err.to_string()))?;
        request = request.body(body);
    }

    let resp = request.send().await;

    let url_str = format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.port().unwrap_or(80)
    );

    let status = match resp.as_ref() {
        Ok(resp) => resp.status().as_str().to_string(),
        Err(err) => err
            .status()
            .map(|status| status.to_string())
            .unwrap_or("UNKNOWN".to_string()),
    };

    call_external_api!(
        method.as_str(),
        url_str.as_str(),
        url.path(),
        status.as_str(),
        start_time
    );

    match resp {
        Ok(resp) => {
            if resp.status().is_success() {
                info!(tag = "[OUTGOING API]", request_method = %method, request_body = format!("{:?}", body), request_url = %url_str, request_headers = format!("{:?}", header_map), response = format!("{:?}", resp), latency = format!("{:?}ms", start_time.elapsed().as_millis()));
                Ok(resp
                    .json::<T>()
                    .await
                    .map_err(|err| CallApiError::ResponseDeserializationFailed(err.to_string()))?)
            } else {
                error!(tag = "[OUTGOING API - ERROR]", request_method = %method, request_body = format!("{:?}", body), request_url = %url_str, request_headers = format!("{:?}", header_map), error = format!("{:?}", resp), latency = format!("{:?}ms", start_time.elapsed().as_millis()));
                Err(CallApiError::ExternalAPICallError(resp))
            }
        }
        Err(err) => {
            error!(tag = "[OUTGOING API - ERROR]", request_method = %method, request_body = format!("{:?}", body), request_url = %url_str, request_headers = format!("{:?}", header_map), error = format!("{:?}", err), latency = format!("{:?}ms", start_time.elapsed().as_millis()));
            Err(CallApiError::ConnectionError(err.to_string()))
        }
    }
}
