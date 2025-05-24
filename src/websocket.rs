use actix_web::{Error, HttpRequest, HttpResponse, get, web};
use actix_ws::handle;
use serde_json::json;
use tokio::sync::broadcast;

use crate::{handlers::extract_project_name, models::{AppState, ServerMessage, WebSocketQuery}};

pub async fn websocket_handler(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
    query: web::Query<WebSocketQuery>,
) -> Result<HttpResponse, Error> {
    let token = &query.token;
    let project_name = extract_project_name(&req, &state.config)?; 
    

    // Validate token exists in any current build
    let projects = state.projects.read().await;
    let mut token_valid = false;

    let project_state = projects.get(&project_name).unwrap();

    let current_build = project_state.current_build.lock().await;

    if let Some(build) = current_build.as_ref() {
        if build.socket_token == *token {
            token_valid = true;
        }
    }
    else{
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "No any build in progress"
        })));
    }


    if !token_valid {
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "Invalid or expired token"
        })));
    }

    let (res, mut session, _msg_stream) = handle(&req, stream)?;

    // Add connection to manager and get receiver
    let mut receiver = state.build_sender.subscribe();

    // Send any existing logs for this token
   
    if let Some(build) = current_build.as_ref() {
        let json_array = serde_json::to_string(&*build.logs).unwrap();
        // for line in buf.iter() {
        let _ = session.text(json_array).await;
    }

    // Handle incoming messages
    let token_clone = token.clone();

    actix_web::rt::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(ServerMessage::Data(data)) => {
                    let _ = session.text(data).await;
                }
                Ok(ServerMessage::Shutdown) => {
                    if let Err(e) = session.close(None).await {
                        log::error!("Error closing websocket: {}", e);
                    }
                    break; // shutdown signal received
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => continue, // Lagged or other
            }
        } //loop

        log::info!("WebSocket connection closed for token: {}", token_clone);
    });

    Ok(res)
}
