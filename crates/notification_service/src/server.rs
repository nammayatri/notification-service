/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::{
    action::{healthcheck::Healthcheck, notification::NotificationService},
    common::types::*,
    environment::{AppConfig, AppState},
    health_server::HealthServer,
    middleware::request_response_tracking::RequestResponseTrackingMiddlewareLayer,
    notification_server::NotificationServer,
    reader::run_notification_reader,
    tools::{logger::setup_tracing, prometheus::prometheus_metrics},
    NotificationPayload,
};
use actix_web::{web, App, HttpResponse, HttpServer};
use anyhow::{anyhow, Result};
use std::{
    env::var,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::mpsc::{self, Receiver, Sender},
    time::sleep,
};
use tonic::{transport::Server, Status};
use tracing::*;

pub async fn run_server() -> Result<()> {
    let dhall_config_path = var("DHALL_CONFIG")
        .unwrap_or_else(|_| "./dhall-configs/dev/notification_service.dhall".to_string());
    let app_config = serde_dhall::from_file(dhall_config_path)
        .parse::<AppConfig>()
        .unwrap();

    let _guard = setup_tracing(app_config.logger_cfg);

    std::panic::set_hook(Box::new(|panic_info| {
        error!("Panic Occured : {:?}", panic_info);
    }));

    let app_state = AppState::new(app_config).await;

    #[allow(clippy::type_complexity)]
    let (read_notification_tx, read_notification_rx): (
        Sender<(
            ClientId,
            Option<Sender<Result<NotificationPayload, Status>>>,
        )>,
        Receiver<(
            ClientId,
            Option<Sender<Result<NotificationPayload, Status>>>,
        )>,
    ) = mpsc::channel(100000);

    let graceful_termination_requested = Arc::new(AtomicBool::new(false));
    let signal = tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        let mut sigint = signal(SignalKind::interrupt()).unwrap();
        tokio::select! { _ = sigterm.recv() => {}, _ = sigint.recv() => {} }
    });

    let read_notification_thread = run_notification_reader(
        read_notification_rx,
        graceful_termination_requested.clone(),
        app_state.redis_pool.clone(),
        app_state.reader_delay_seconds,
        app_state.retry_delay_seconds,
        app_state.last_known_notification_cache_expiry,
        app_state.max_shards,
    );

    let prometheus = prometheus_metrics();
    let http_server = HttpServer::new(move || {
        App::new().wrap(prometheus.clone()).route(
            "/health",
            web::get()
                .to(|| Box::pin(async { HttpResponse::Ok().body("Notification Service Is Up!") })),
        )
    })
    .bind((Ipv4Addr::UNSPECIFIED, app_state.http_server_port))
    .unwrap()
    .shutdown_timeout(60)
    .run();

    let grpc_port = app_state.grpc_port;
    let middleware = tower::ServiceBuilder::new()
        .layer(RequestResponseTrackingMiddlewareLayer)
        .into_inner();
    let notification_service = NotificationService::new(read_notification_tx, app_state);
    let grpc_server = Server::builder()
        .layer(middleware)
        .add_service(NotificationServer::new(notification_service))
        .add_service(HealthServer::new(Healthcheck))
        .serve(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            grpc_port,
        )));

    tokio::select! {
        res = http_server => {
            Err(anyhow!("[HTTP_SERVER] : {:?}", res))
        }
        res = grpc_server => {
            Err(anyhow!("[GRPC_SERVER] : {:?}", res))
        }
        res = read_notification_thread => {
            Err(anyhow!("[READ_NOTIFICATION] : {:?}", res))
        }
        res = signal => {
            error!("[GRACEFULL_TERMINATION] : {:?}", res);
            graceful_termination_requested.store(true, Ordering::Relaxed);
            sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    }
}
