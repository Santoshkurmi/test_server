use actix_web::{web, App, HttpRequest, HttpResponse, Result, get, post, delete};
use serde_json::json;
use uuid::Uuid;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::is_authorized;
use crate::models::{AppState, BuildApiRequest, BuildApiResponse, BuildStatusResponse, BuildRequest, BuildInfo};
use crate::config::Config;
use crate::build::BuildManager;
use crate::utils;
use crate::websocket::websocket_handler;

#[get("/health")]
pub async fn health_check() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(json!({
        "status": "healthy",
        "timestamp": Utc::now()
    })))
}

pub fn register_project_routes<T>(mut app: App<T>, config: &Config) -> App<T>

where
    T: actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse,
        Error = actix_web::Error,
        InitError = (),
    >,{
    for (project_name, project_config) in &config.projects {
        let base_path = &project_config.base_endpoint_path;
        
        // Build endpoint
        let build_path = format!("{}{}", base_path, &project_config.api.build.endpoint);
        let build_method = &project_config.api.build.method;
        
        // Register build endpoint based on method
        match build_method.to_uppercase().as_str() {
            "POST" => {
                app = app.route(&build_path, web::post().to(build_handler));
            }
            "GET" => {
                app = app.route(&build_path, web::get().to(build_handler));
            }
            _ => {
                log::warn!("Unsupported HTTP method: {} for project: {}", build_method, project_name);
            }
        }
        
        // Is building endpoint
        let is_building_path = format!("{}{}", base_path, &project_config.api.is_building.endpoint);
        app = app.route(&is_building_path, web::get().to(is_building_handler));

        let websocket_path = format!("{}{}", base_path, &project_config.api.socket.endpoint);
        app = app.route(&websocket_path, web::get().to(websocket_handler));
        
        // Abort endpoint
        let abort_path = format!("{}{}", base_path, &project_config.api.abort.endpoint);
        app = app.route(&abort_path, web::post().to(abort_handler));
        
        // Cleanup endpoint
        let cleanup_path = format!("{}{}", base_path, &project_config.api.cleanup.endpoint);
        app = app.route(&cleanup_path, web::post().to(cleanup_handler));
    }
    
    app
}

async fn build_handler(
    req: HttpRequest,
    payload: web::Json<BuildApiRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse> {
    let project_name = extract_project_name(&req, &state.config)?; 
    
    if !is_authorized(&req, &state, Some(&project_name)).await {
        return Ok(HttpResponse::Unauthorized().json(BuildApiResponse {
            success: false,
            message: "Unauthorized".to_string(),
            data: None,
        }));
    }
    
    let project_config = state.config.projects.get(&project_name).unwrap();
    
    // Validate payload
    for required_field in &project_config.api.build.payload {
        let field_name = required_field.trim_start_matches('$');
        if !payload.payload.contains_key(field_name) {
            return Ok(HttpResponse::BadRequest().json(BuildApiResponse {
                success: false,
                message: format!("Missing required field: {}", field_name),
                data: None,
            }));
        }
    }
    
    // Generate build ID and socket token
    let build_id = Uuid::new_v4().to_string();
    let socket_token = utils::generate_token(32);
    
    // Create build request
    let build_request = BuildRequest {
        id: build_id.clone(),
        project_name: project_name.clone(),
        payload: payload.payload.clone(),
        files: HashMap::new(), // TODO: Handle file uploads
        created_at: Utc::now(),
        socket_token: socket_token.clone(),
    };
    
    // Add to queue
    let projects = state.projects.read().await;
    let project_state = projects.get(&project_name).unwrap();
    
    // Check if multi-build is allowed
    if !project_config.allow_multi_build {
        let current_builds = project_state.current_build.lock().await;
        if current_builds.is_some() {
            return Ok(HttpResponse::Conflict().json(BuildApiResponse {
                success: false,
                message: "Build already in progress".to_string(),
                data: Some(json!({
                    "current_builds": 0
                })),
            }));
        }
    }
    
    // Check queue limit
    let mut queue = project_state.build_queue.lock().await;
    if queue.len() >= project_config.max_pending_build as usize {
        return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
            success: false,
            message: "Build queue is full".to_string(),
            data: Some(json!({
                "queue_length": queue.len(),
                "max_queue": project_config.max_pending_build
            })),
        }));
    }
    
    queue.push(build_request);
    drop(queue);
    
    // Start build manager if not running
    BuildManager::process_queue(state.clone(), project_name.clone()).await;
    
    // Prepare response
    let mut response_data = json!({
        "build_id": build_id,
        "socket_token": socket_token,
        "status": "queued"
    });
    
    // Add custom return fields
    for return_field in &project_config.api.build.return_fields {
        let value = utils::resolve_variable(&return_field.value, &payload.payload, &socket_token);
        let key = return_field.name.as_ref().unwrap_or(&return_field.value);
        response_data[key] = json!(value);
    }
    
    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: true,
        message: "Build queued successfully".to_string(),
        data: Some(response_data),
    }))
}

async fn is_building_handler(
    req: HttpRequest,
    state: web::Data<AppState>,
) -> Result<HttpResponse> {
    let project_name = extract_project_name(&req, &state.config)?;
    
    if !is_authorized(&req, &state, Some(&project_name)).await {
        return Ok(HttpResponse::Unauthorized().json(BuildApiResponse {
            success: false,
            message: "Unauthorized".to_string(),
            data: None,
        }));
    }
    
    let projects = state.projects.read().await;
    let project_state = projects.get(&project_name).unwrap();
    
    let queue = project_state.build_queue.lock().await;
    let current_build = project_state.current_build.lock().await;
    
    if let Some(build) = current_build.as_ref() {
       let build_info = BuildInfo {
                id: build.id.clone(),
                status: build.status.clone(),
                current_step: build.current_step,
                total_steps: build.total_steps,
                socket_token: build.socket_token.clone(),
            };

       return  Ok(HttpResponse::Ok().json(BuildStatusResponse {
        is_building: true,
        queue_length: queue.len(),
        current_build: Some(build_info),
        }));

    }//if 

   
    Ok(HttpResponse::Ok().json(BuildStatusResponse {
        is_building: false,
        queue_length: queue.len(),
        current_build: None,
    }))
}

async fn abort_handler(
    req: HttpRequest,
    payload: web::Json<BuildApiRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse> {
    let project_name = extract_project_name(&req, &state.config)?;
    
    if !is_authorized(&req, &state, Some(&project_name)).await {
        return Ok(HttpResponse::Unauthorized().json(BuildApiResponse {
            success: false,
            message: "Unauthorized".to_string(),
            data: None,
        }));
    }
    
    // TODO: Implement build abortion logic
    BuildManager::abort_build(state.clone(), project_name, payload.payload.clone()).await;
    
    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: true,
        message: "Build aborted".to_string(),
        data: None,
    }))
}

async fn cleanup_handler(
    req: HttpRequest,
    payload: web::Json<BuildApiRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse> {
    let project_name = extract_project_name(&req, &state.config)?;
    
    if !is_authorized(&req, &state, Some(&project_name)).await {
        return Ok(HttpResponse::Unauthorized().json(BuildApiResponse {
            success: false,
            message: "Unauthorized".to_string(),
            data: None,
        }));
    }
    
    // TODO: Implement cleanup logic
    BuildManager::cleanup_project(state.clone(), project_name, payload.payload.clone()).await;
    
    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: true,
        message: "Cleanup completed".to_string(),
        data: None,
    }))
}

pub fn extract_project_name(req: &HttpRequest, config: &Config) -> Result<String> {
    let path = req.path();
    
    for (project_name, project_config) in &config.projects {
        if path.starts_with(&project_config.base_endpoint_path) {
            return Ok(project_name.clone());
        }
    }
    
    Err(actix_web::error::ErrorNotFound("Project not found"))
}