use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use reqwest::Client;
use crate::environment::{RequestBody, ResponseJson, ApsResponse, AppConfig};
use std::env::var;
use std::{net::Ipv4Addr, sync::Arc};


async fn send_notification(
    req: HttpRequest,
    req_body: web::Json<RequestBody>,
) -> impl Responder {
    let device_token = req
        .headers()
        .get("device-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bearer_token = req
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let apns_topic = req
        .headers()
        .get("apns_topic")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let apns_push_type = req
        .headers()
        .get("apns-push-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let apns_priority = req
        .headers()
        .get("apns_priority")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let dhall_config_path = var("DHALL_CONFIG")
        .unwrap_or_else(|_| "./dhall-configs/dev/liveactivity_service.dhall".to_string());

    let app_config:AppConfig = serde_dhall::from_file(dhall_config_path)
        .parse::<AppConfig>()
        .unwrap();

    let client = Client::builder().http2_prior_knowledge().build().unwrap();
    let path = format!("/3/device/{}", device_token);
    let json = ResponseJson {
        aps: ApsResponse {
            timestamp: chrono::Utc::now().timestamp() as u64,
            event: "update".to_string(),
            content_state: req_body.aps.content_state.clone(),
            alert: req_body.aps.alert.clone(),
        },
    };
    
    let res = client
        .post(&format!(
            "{}:{}{}",app_config.apns_url, app_config.apns_port, path))
        .header("Authorization", bearer_token)
        .header("apns-push-type", apns_push_type)
        .header("apns-topic", apns_topic)
        .header("apns-priority", apns_priority)
        .json(&json)        
        .send()
        .await;

    match res {
        Ok(response) => HttpResponse::Ok().body(response.text().await.unwrap_or_default()),
        Err(err) => HttpResponse::InternalServerError().body(err.to_string()),
    }
}

pub async fn run_server() -> std::io::Result<()> {
    let dhall_config_path = var("DHALL_CONFIG")
        .unwrap_or_else(|_| "./dhall-configs/dev/liveactivity_service.dhall".to_string());

    let app_config:AppConfig = serde_dhall::from_file(dhall_config_path)
        .parse::<AppConfig>()
        .unwrap();
    

    HttpServer::new(|| {
        App::new()
            .service(web::resource("/send-notification").route(web::post().to(send_notification)))
    })
    .bind((Ipv4Addr::UNSPECIFIED, app_config.port))?
    .run()
    .await
}