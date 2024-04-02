/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::incoming_api;
use crate::tools::prometheus::INCOMING_API;
use hyper::Body;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tonic::body::BoxBody;
use tower::{Layer, Service};

#[derive(Debug, Clone, Default)]
pub struct RequestResponseTrackingMiddlewareLayer;

impl<S> Layer<S> for RequestResponseTrackingMiddlewareLayer {
    type Service = RequestResponseTrackingMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        RequestResponseTrackingMiddleware { inner: service }
    }
}

#[derive(Debug, Clone)]
pub struct RequestResponseTrackingMiddleware<S> {
    inner: S,
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl<S> Service<hyper::Request<Body>> for RequestResponseTrackingMiddleware<S>
where
    S: Service<hyper::Request<Body>, Response = hyper::Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: hyper::Request<Body>) -> Self::Future {
        let start_time = Instant::now();

        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let req_path = req.uri().path().to_string();
        let req_method = req.method().to_string();

        Box::pin(async move {
            let response = inner.call(req).await?;

            let resp_headers = response.headers();
            if let (Some(resp_message), Some(resp_status)) = (
                resp_headers
                    .get("grpc-message")
                    .and_then(|val| val.to_str().ok()),
                resp_headers
                    .get("grpc-status")
                    .and_then(|val| val.to_str().ok()),
            ) {
                incoming_api!(
                    req_method.as_str(),
                    req_path.as_str(),
                    resp_status,
                    resp_message,
                    start_time
                );
            } else {
                incoming_api!(
                    req_method.as_str(),
                    req_path.as_str(),
                    "0",
                    "OK",
                    start_time
                );
            }

            Ok(response)
        })
    }
}
