use std::{
    collections::HashMap,
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use futures::{
    future::{self, Either},
    Stream, TryFutureExt,
};
use grpc_rust::server_side_streaming::{
    server_stream_server::{ServerStream, ServerStreamServer},
    Empty, Message, User,
};
use hyper::{http, service::make_service_fn, Server};
use prometheus::{IntCounter, IntGauge, Registry};
use redis::{Client, Commands};
use tokio::sync::{mpsc, RwLock};
use tonic::{transport::Server as TonicServer, Request, Response, Status};
use tower::Service;
use tracing::{error, info};
use warp::{Filter, Rejection, Reply};

#[allow(clippy::expect_used)]
pub static INCOMING_REQUESTS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        IntCounter::new("incoming_requests", "Incoming Requests")
            .expect("incoming requests metric couldn't be created")
    });
#[allow(clippy::expect_used)]
pub static CONNECTED_CLIENTS: once_cell::sync::Lazy<IntGauge> = once_cell::sync::Lazy::new(|| {
    IntGauge::new("connected_clients", "Connected Clients")
        .expect("connected clients metric couldn't be created")
});
pub static REGISTRY: once_cell::sync::Lazy<Registry> = once_cell::sync::Lazy::new(Registry::new);

#[derive(Debug)]
struct Shared {
    senders: HashMap<String, mpsc::Sender<Result<Message, Status>>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            senders: HashMap::new(),
        }
    }

    async fn broadcast(&self, msg: &Message) -> Result<(), ()> {
        match self.senders.get(&msg.to) {
            Some(stream_tx) => stream_tx.send(Ok(msg.clone())).await.unwrap(),
            None => return Err(()),
        }

        Ok(())
    }
}

#[derive(Debug)]
struct ServerStreamService {
    shared: Arc<RwLock<Shared>>,
}

impl ServerStreamService {
    fn new(shared: Arc<RwLock<Shared>>) -> Self {
        Self { shared }
    }
}

struct CustomStream(mpsc::Receiver<Result<Message, Status>>);

impl Stream for CustomStream {
    type Item = Result<Message, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Pin::new(&mut self).poll_next(cx)
        self.get_mut().0.poll_recv(cx)
    }
}

impl Drop for CustomStream {
    fn drop(&mut self) {
        CONNECTED_CLIENTS.dec();
        info!("[Disconnected]");
    }
}

#[tonic::async_trait]
impl ServerStream for ServerStreamService {
    type ReceiveMessageStream =
        Pin<Box<dyn Stream<Item = Result<Message, Status>> + Send + Sync + 'static>>;

    async fn receive_message(
        &self,
        request: Request<User>,
    ) -> Result<Response<Self::ReceiveMessageStream>, Status> {
        let req_data = request.into_inner();
        let id = req_data.id;
        let name = req_data.name;

        INCOMING_REQUESTS.inc();
        CONNECTED_CLIENTS.inc();
        info!("[Connected] {name}");

        let (stream_tx, stream_rx) = mpsc::channel::<Result<Message, Status>>(128);

        let redis_client = Client::open("redis://127.0.0.1/").unwrap();
        info!(tag = "[REDIS CONNECTION]", "redis://127.0.0.1/");
        let mut redis_conn = redis_client.get_connection().unwrap();

        let _result: redis::Value = redis_conn.set(&id, "https://127.0.0.1:50051").unwrap();

        self.shared
            .write()
            .await
            .senders
            .insert(id.clone(), stream_tx);

        Ok(Response::new(Box::pin(CustomStream(stream_rx))))
    }

    async fn send_message(&self, request: Request<Message>) -> Result<Response<Empty>, Status> {
        let msg = request.into_inner();

        match self.shared.read().await.broadcast(&msg).await {
            Ok(_) => {
                info!(tag = "[BROADCAST - SUCCESS]", client_id = %msg.to);
            }
            Err(_) => {
                error!(tag = "[BROADCAST - ERROR]", client_id = %msg.to);

                let redis_client = Client::open("redis://127.0.0.1/").unwrap();
                info!(tag = "[REDIS CONNECTION]", "redis://127.0.0.1/");
                let mut redis_conn = redis_client.get_connection().unwrap();

                // self.shared.write().await.senders.remove(&msg.to);
                let _result: redis::Value = redis_conn.del(&msg.to).unwrap();

                return Err(Status::unavailable(""));
            }
        }

        Ok(Response::new(Empty {}))
    }
}

fn register_custom_metrics() {
    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(INCOMING_REQUESTS.clone()))
        .expect("`INCOMING_REQUESTS` collector couldn't be registered");

    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(CONNECTED_CLIENTS.clone()))
        .expect("`CONNECTED_CLIENTS` collector couldn't be registered");
}

async fn metrics_handler() -> Result<impl Reply, Rejection> {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();

    let mut buffer = Vec::new();
    if let Err(error) = encoder.encode(&REGISTRY.gather(), &mut buffer) {
        error!(%error, "could not encode custom metrics");
    };
    let mut res = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(error) => {
            error!(%error, "custom metrics could not be converted from bytes");
            String::default()
        }
    };
    buffer.clear();

    let mut buffer = Vec::new();
    if let Err(error) = encoder.encode(&prometheus::gather(), &mut buffer) {
        error!(%error, "could not encode prometheus metrics");
    };
    let res_custom = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(error) => {
            error!(%error, "prometheus metrics could not be converted from bytes");
            String::default()
        }
    };
    buffer.clear();

    res.push_str(&res_custom);
    Ok(res)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = grpc_rust::setup_tracing(std::env!("CARGO_BIN_NAME"));

    register_custom_metrics();

    let addr = "0.0.0.0:50051".parse().unwrap();
    info!("Server listening on: {addr}");

    let mut warp = warp::service(warp::path("metrics").and_then(metrics_handler));

    let shared = Arc::new(RwLock::new(Shared::new()));

    Server::bind(&addr)
        .serve(make_service_fn(move |_| {
            let server_stream_service = ServerStreamService::new(shared.clone());

            let mut tonic = TonicServer::builder()
                .tcp_keepalive(Some(Duration::from_secs(1)))
                .http2_keepalive_interval(Some(Duration::from_secs(30)))
                .http2_keepalive_timeout(Some(Duration::from_secs(5)))
                .add_service(ServerStreamServer::new(server_stream_service))
                .into_service();

            future::ok::<_, Infallible>(tower::service_fn(
                move |req: hyper::Request<hyper::Body>| match req.uri().path() {
                    "/metrics" => Either::Left(
                        warp.call(req)
                            .map_ok(|res| res.map(EitherBody::Left))
                            .map_err(Error::from),
                    ),
                    _ => Either::Right(
                        tonic
                            .call(req)
                            .map_ok(|res| res.map(EitherBody::Right))
                            .map_err(Error::from),
                    ),
                },
            ))
        }))
        .await?;

    Ok(())
}

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

enum EitherBody<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> http_body::Body for EitherBody<A, B>
where
    A: http_body::Body + Send + Unpin,
    B: http_body::Body<Data = A::Data> + Send + Unpin,
    A::Error: Into<Error>,
    B::Error: Into<Error>,
{
    type Data = A::Data;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Left(b) => b.is_end_stream(),
            Self::Right(b) => b.is_end_stream(),
        }
    }

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        match self.get_mut() {
            Self::Left(b) => Pin::new(b).poll_data(cx).map(map_option_err),
            Self::Right(b) => Pin::new(b).poll_data(cx).map(map_option_err),
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        match self.get_mut() {
            Self::Left(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
            Self::Right(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
        }
    }
}

fn map_option_err<T, U: Into<Error>>(err: Option<Result<T, U>>) -> Option<Result<T, Error>> {
    err.map(|e| e.map_err(Into::into))
}
