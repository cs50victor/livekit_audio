use std::{error::Error, sync::{atomic::{AtomicBool, Ordering}, Arc}};
use actix_web::{http::Method, HttpRequest, HttpResponse as Resp, Responder};

use livekit_api::{access_token, webhooks};
use serde::{Deserialize, Serialize};
use actix_web::web::{self, Data};
use log::info;


#[derive(Debug, Serialize, Deserialize)]
pub struct ServerMsg<T> {
    data: Option<T>,
    error: Option<String>,
}

impl<T: AsRef<str>> ServerMsg<T> {
    pub fn data(data: T) -> Self {
        Self { data: Some(data), error: None }
    }

    pub fn error(error: T) -> Self {
        let err_msg = error.as_ref();
        log::warn!("server error. {err_msg:?}");
        Self { data: None, error: Some(err_msg.to_string()) }
    }
}

pub async fn health_check() -> impl actix_web::Responder {
    actix_web::HttpResponse::Ok().json(ServerMsg::data("OK"))
}

pub async fn livekit_webhook_handler(
    req: HttpRequest,
    is_active: web::Data<AtomicBool>,
    // server_data: web::Data<super::ServerStateMutex>,
    body: web::Bytes,
) -> impl Responder {
    if req.method().ne(&Method::POST) {
        return Resp::MethodNotAllowed()
            .json(ServerMsg::error("Method not allowed"));
    }

    log::info!("SERVER RECEIVED WEBHOOK");

    let token_verifier = match access_token::TokenVerifier::new() {
        Ok(i) => i,
        Err(e) => return Resp::InternalServerError().json(ServerMsg::error(e.to_string())),
    };
    let webhook_receiver = webhooks::WebhookReceiver::new(token_verifier);

    let jwt = req
        .headers()
        .get("Authorization")
        .and_then(|hv| hv.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let jwt = jwt.trim();

    let body = match std::str::from_utf8(&body) {
        Ok(i) => i,
        Err(e) => return Resp::BadRequest().json(ServerMsg::error(e.to_string())),
    };

    let event = match webhook_receiver.receive(body, jwt) {
        Ok(i) => i,
        Err(e) => return Resp::InternalServerError().json(ServerMsg::error(e.to_string())),
    };

    // room_finished
    if event.room.is_some() {
        let livekit_protocol::Room {
            name: participant_room_name,
            max_participants,
            num_participants,
            ..
        } = event.room.unwrap();
        let event = event.event;
        if event == "room_started" {
            if num_participants < max_participants {
                info!("... establishing connection to room");

                // let server_data = server_data.lock();

                // log::info!("app state {:#?}", *server_data.app_state);

                // *server_data.app_state.lock() =
                //     crate::ParticipantRoomName(participant_room_name);

                // log::info!("app state {:?}", *server_data.app_state);

                is_active.store(true, Ordering::Relaxed);
                info!("\nSERVER FINISHED PROCESSING ROOM_STARTED WEBHOOK");
            };
        } else if event == "room_finished" {
            // let server_data = server_data.lock();

            // log::info!("app state {:#?}", *server_data.app_state);

            // *server_data.app_state.lock() =
            //     crate::ParticipantRoomName(format!("reset:{participant_room_name}"));

            // log::info!("app state {:?}", *server_data.app_state);

            is_active.store(false, Ordering::Relaxed);
            info!("\nSERVER FINISHED PROCESSING ROOM_FINISHED WEBHOOK");
        }
    } else {
        info!("received event {}", event.event);
    }

    Resp::Ok().json(ServerMsg::data("Livekit Webhook Successfully Processed"))
}


#[tokio::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "6669".to_string())
        .parse::<u16>()
        .expect("PORT couldn't be set");


    pretty_env_logger::formatted_builder()
        .filter_module("livekit_audio", log::LevelFilter::Info)
        .filter_module("actix_server", log::LevelFilter::Info)
        .filter_module("actix_web", log::LevelFilter::Info)
        .init();

    info!("starting HTTP server on port {port}");
    
    // let server_resources =
    //     Data::new(parking_lot::Mutex::new(ServerResources { app_state }));

    let is_active = Arc::new(AtomicBool::new(false));

    let server = actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .wrap(actix_web::middleware::Compress::default())
            .wrap(actix_web::middleware::Logger::new("IP - %a | Time - %D ms"))
            .wrap(
                actix_web::middleware::DefaultHeaders::new()
                    .add(("Content-Type", "application/json")),
            )
            .app_data(web::Data::from(is_active.clone()))
            .service(web::resource("/").to(health_check))
            .service(web::resource("/webhooks/livekit").to(livekit_webhook_handler))
    })
    .bind(("0.0.0.0", port))?
    .run();

    server.await
}
