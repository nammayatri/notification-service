/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use super::types::*;
use crate::tools::{
    callapi::{call_api, CallApiError},
    error::AppError,
};
use actix_http::StatusCode;
use anyhow::Result;
use reqwest::{Method, Url};

pub async fn internal_authentication(
    auth_url: &Url,
    token: &str,
    auth_api_key: &str,
) -> Result<AuthResponseData> {
    let resp: Result<AuthResponseData, CallApiError> = call_api::<AuthResponseData, ()>(
        Method::GET,
        auth_url,
        vec![
            ("content-type", "application/json"),
            ("token", token),
            ("api-key", auth_api_key),
        ],
        None,
    )
    .await;

    match resp {
        Ok(resp) => Ok(resp),
        Err(CallApiError::ExternalAPICallError(error)) => {
            if error.status() == StatusCode::BAD_REQUEST {
                Err(AppError::DriverAppAuthFailed)?
            } else if error.status() == StatusCode::UNAUTHORIZED {
                Err(AppError::DriverAppUnauthorized)?
            } else {
                Err(AppError::DriverAppAuthFailed)?
            }
        }
        Err(err) => Err(AppError::InternalError(err.to_string()))?,
    }
}

pub async fn driver_quote_respond(
    base_url: &Url,
    token: &str,
    headers: Vec<(&str, &str)>,
    request_body: super::types::DriverRespondReq,
) -> Result<super::types::ApiSuccess, CallApiError> {
    let mut url = base_url.clone();
    url.path_segments_mut()
        .expect("Invalid base URL")
        .push("driver")
        .push("quote")
        .push("respond");

    let mut all_headers = vec![("content-type", "application/json"), ("token", token)];
    all_headers.extend(headers);

    call_api::<super::types::ApiSuccess, super::types::DriverRespondReq>(
        Method::POST,
        &url,
        all_headers,
        Some(request_body),
    )
    .await
}
