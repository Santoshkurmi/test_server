use actix_web::{web, HttpRequest, HttpResponse, Error, get};
use actix_ws::handle;
use serde_json::json;

use crate::models::{AppState, WebSocketQuery};
use crate::auth::is_authorized;

#[get("/ws")]
pub async fn websocket_handler(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
    query: web::Query<WebSocketQuery>,
) -> Result<HttpResponse, Error> {
    // Basic authorization check (you might want to implement token-specific auth)
    if !is_authorized(&req, &state, None).await {
        return Ok(HttpResponse::Unauthorized().json(json!({
            "error": "Unauthorized"
        })));
    }
    
    let token = &query.token;
    
    // Validate token exists in any current build
    let projects = state.projects.read().await;
    let mut token_valid = false;
    
    for (_, project_state) in projects.iter() {
        let current_builds = project_state.current_builds.lock().await;
        for build in current_builds.values() {
            if build.socket_token == *token {
                token_valid = true;
                break;
            }
        }
        if token_valid {
            break;
        }
    }
    drop(projects);
    
    if !token_valid {
        return Ok(HttpResponse::BadRequest().json(json!({
            "error": "Invalid or expired token"
        })));
    }
    
    let (res, mut session, _msg_stream) = handle(&req, stream)?;
    
    // Add connection to manager and get receiver
    let mut receiver = state.websocket_manager.add_connection(token).await;
    
    // Send connection established message
    let welcome_msg = json!({
        "type": "connected",
        "token": token,
        "timestamp": chrono::Utc::now()
    });
    let _ = session.text(welcome_msg.to_string()).await;
    
    // Send any existing logs for this token
    let projects = state.projects.read().await;
    for (_, project_state) in projects.iter() {
        let current_builds = project_state.current_builds.lock().await;
        for build in current_builds.values() {
            if build.socket_token == *token {
                // Send existing logs
                for log in &build.logs {
                    let log_msg = json!({
                        "type": "log",
                        "build_id": build.id,
                        "step": log.step,
                        "level": log.level,
                        "message": log.message,
                        "timestamp": log.timestamp,
                        "command": log.command
                    });
                    let _ = session.text(log_msg.to_string()).await;
                }
                
                // Send current status
                let status_msg = json!({
                    "type": "status",
                    "build_id": build.id,
                    "status": build.status,
                    "current_step": build.current_step,
                    "total_steps": build.total_steps
                });
                let _ = session.text(status_msg.to_string()).await;
                break;
            }
        }
    }
    drop(projects);
    
    // Handle incoming messages
    let token_clone = token.clone();
    let websocket_manager = state.websocket_manager.clone();
    
    actix_web::rt::spawn(async move {
        while let Ok(message) = receiver.recv().await {
            if session.text(message).await.is_err() {
                break; // Client disconnected
            }
        }
        
        // Clean up connection
        websocket_manager.remove_connection(&token_clone).await;
        log::info!("WebSocket connection closed for token: {}", token_clone);
    });
    
    Ok(res)
}