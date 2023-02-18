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
use grpc_rust::chat::{
    chat_server::{Chat, ChatServer},
    Empty, Message, User,
};
use hyper::{http, service::make_service_fn, Server};
use prometheus::{IntCounter, IntGauge, Registry};
use redis::{Client, Commands};
use tokio::sync::{mpsc, RwLock};
use tonic::{transport::Server as TonicServer, Request, Response, Status};
use tower::Service;
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
        Shared {
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
struct ChatService {
    shared: Arc<RwLock<Shared>>,
}

impl ChatService {
    fn new(shared: Arc<RwLock<Shared>>) -> Self {
        ChatService { shared }
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
        println!("[Disconnected]");
    }
}

#[tonic::async_trait]
impl Chat for ChatService {
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
        println!("[Connected] {}", &name);

        let (stream_tx, stream_rx) = mpsc::channel::<Result<Message, Status>>(128);

        let redis_client = Client::open("redis://127.0.0.1/").unwrap();
        println!("[REDIS CONNECTION]: redis://127.0.0.1/");
        let mut redis_conn = redis_client.get_connection().unwrap();

        let _result: redis::Value = redis_conn.set(&id, "https://127.0.0.1:5051").unwrap();

        self.shared
            .write()
            .await
            .senders
            .insert(id.clone(), stream_tx);

        Ok(Response::new(Box::pin(CustomStream(stream_rx))))
    }

    async fn send_message(&self, request: Request<Message>) -> Result<Response<Empty>, Status> {
        let req_data = request.into_inner();
        let id = req_data.id;
        let to_id = req_data.to;
        let content = req_data.content;
        let ttl = req_data.ttl;
        let msg = Message {
            id,
            to: to_id,
            content,
            ttl,
        };

        match self.shared.read().await.broadcast(&msg).await {
            Ok(_) => {
                println!("[BROADCAST - SUCCESS] client-id : {}", msg.to);
            }
            Err(e) => {
                println!(
                    "[BROADCAST - ERROR] client-id : {}, error-message: {:?}",
                    msg.to, e
                );

                let redis_client = Client::open("redis://127.0.0.1/").unwrap();
                println!("[REDIS CONNECTION]: redis://127.0.0.1/");
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
    REGISTRY
        .register(Box::new(INCOMING_REQUESTS.clone()))
        .expect("collector can be registered");

    REGISTRY
        .register(Box::new(CONNECTED_CLIENTS.clone()))
        .expect("collector can be registered");
}

async fn metrics_handler() -> Result<impl Reply, Rejection> {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&REGISTRY.gather(), &mut buffer) {
        eprintln!("could not encode custom metrics: {}", e);
    };
    let mut res = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("custom metrics could not be from_utf8'd: {}", e);
            String::default()
        }
    };
    buffer.clear();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&prometheus::gather(), &mut buffer) {
        eprintln!("could not encode prometheus metrics: {}", e);
    };
    let res_custom = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("prometheus metrics could not be from_utf8'd: {}", e);
            String::default()
        }
    };
    buffer.clear();

    res.push_str(&res_custom);
    Ok(res)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    register_custom_metrics();

    let addr = "0.0.0.0:5051".parse().unwrap();
    println!("Server listening on: {}", addr);

    let mut warp = warp::service(warp::path("metrics").and_then(metrics_handler));

    let shared = Arc::new(RwLock::new(Shared::new()));

    Server::bind(&addr)
        .serve(make_service_fn(move |_| {
            let chat_service = ChatService::new(shared.clone());

            let mut tonic = TonicServer::builder()
                .tcp_keepalive(Some(Duration::from_secs(1)))
                .http2_keepalive_interval(Some(Duration::from_secs(30)))
                .http2_keepalive_timeout(Some(Duration::from_secs(5)))
                .add_service(ChatServer::new(chat_service))
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
            EitherBody::Left(b) => b.is_end_stream(),
            EitherBody::Right(b) => b.is_end_stream(),
        }
    }

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        match self.get_mut() {
            EitherBody::Left(b) => Pin::new(b).poll_data(cx).map(map_option_err),
            EitherBody::Right(b) => Pin::new(b).poll_data(cx).map(map_option_err),
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        match self.get_mut() {
            EitherBody::Left(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
            EitherBody::Right(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
        }
    }
}

fn map_option_err<T, U: Into<Error>>(err: Option<Result<T, U>>) -> Option<Result<T, Error>> {
    err.map(|e| e.map_err(Into::into))
}
