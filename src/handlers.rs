use actix_web::{App, HttpRequest, HttpResponse, Result, delete, get, post, web};
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::is_authorized;
use crate::build::{self, BuildManager};
use crate::config::Config;
use crate::models::{
    AppState, BuildApiRequest, BuildApiResponse, BuildInfo, BuildRequest, BuildStatusResponse,
};
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
        >,
{
    for (_project_name, project_config) in &config.projects {
        let base_path = &project_config.base_endpoint_path;

        if !&project_config.api.build.endpoint.trim().is_empty() {
            let build_path = format!("{}{}", base_path, &project_config.api.build.endpoint);
            app = app.route(&build_path, web::post().to(build_handler));
        } //if build

        if !&project_config.api.is_building.endpoint.trim().is_empty() {
            let is_building_path =
                format!("{}{}", base_path, &project_config.api.is_building.endpoint);
            app = app.route(&is_building_path, web::get().to(is_building_handler));
        }

        if !&project_config.api.socket.endpoint.trim().is_empty() {
            let websocket_path = format!("{}{}", base_path, &project_config.api.socket.endpoint);
            app = app.route(&websocket_path, web::get().to(websocket_handler));
        }

        if !&project_config.api.abort.endpoint.trim().is_empty() {
            let abort_path = format!("{}{}", base_path, &project_config.api.abort.endpoint);
            app = app.route(&abort_path, web::post().to(abort_handler));
        }

        if !&project_config.api.cleanup.endpoint.trim().is_empty() {
            let cleanup_path = format!("{}{}", base_path, &project_config.api.cleanup.endpoint);
            app = app.route(&cleanup_path, web::post().to(cleanup_handler));
        }
    } //loop

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
            state: "unauthorized".to_string(),
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
                state: "missing".to_string(),
            }));
        }
    }
    if !payload
        .payload
        .contains_key(&project_config.build.unique_build_key)
    {
        return Ok(HttpResponse::BadRequest().json(BuildApiResponse {
            success: false,
            message: format!(
                "Unique Build Key is required: {}",
                &project_config.build.unique_build_key
            ),
            data: None,
            state: "missing".to_string(),
        }));
    }

    // Add to queue
    let projects = state.projects.read().await;
    let project_state = projects.get(&project_name).unwrap();

    let unique_id = payload
        .payload
        .get(&project_config.build.unique_build_key)
        .unwrap()
        .as_str()
        .unwrap();
    // Check if multi-build is allowed
    if !project_config.allow_multi_build {
        let current_builds = project_state.current_build.lock().await;
        if let Some(build) = current_builds.as_ref() {
            return Ok(HttpResponse::Conflict().json(BuildApiResponse {
                success: false,
                message: "Build already in progress".to_string(),
                data: Some(json!({
                    "current_builds": 0,
                    "socket_token": build.socket_token.clone()
                })),
                state: "already_running".to_string(),
            }));
        }
    } else {
        let current_builds = project_state.current_build.lock().await;
        if let Some(build) = current_builds.as_ref() {
            if build.project_name != project_name {
                return Ok(HttpResponse::Conflict().json(BuildApiResponse {
                    success: false,
                    message: "Build already in progress for other project".to_string(),
                    data: None,
                    state: "already_running_other_project".to_string(),
                }));
            }
        } //if

        // Check queue limit
        let queue = project_state.build_queue.lock().await;
        if queue.len() >= project_config.max_pending_build as usize {
            return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
                success: false,
                message: "Build queue is full".to_string(),
                state: "full".to_string(),
                data: Some(json!({
                    "queue_length": queue.len(),
                    "max_queue": project_config.max_pending_build
                })),
            }));
        }
        drop(queue);

        let queues = project_state.build_queue.lock().await;

        for i in 0..queues.len() {
            let build_req = queues.get(i);
            if let Some(build) = build_req {
                if &build.unique_id == unique_id {
                    return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
                        success: false,
                        message: "This build is already in pending".to_string(),
                        state: "already".to_string(),
                        data: Some(json!({"socket_token":build.socket_token})),
                    }));
                }
            }
        }
        drop(queues);
        drop(current_builds);
        let current_build = project_state.current_build.lock().await;
        if let Some(cur) = current_build.as_ref() {
            if cur.unique_id == unique_id {
                return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
                    success: false,
                    message: "This build is already in pending".to_string(),
                    state: "already".to_string(),
                    data: Some(json!({"socket_token":cur.socket_token})),
                }));
            }
        }
        drop(current_build);
    } //if multi build available

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
        unique_id: unique_id.to_string(),
        socket_token: socket_token.clone(),
    };

    let mut queue = project_state.build_queue.lock().await;

    queue.push(build_request);
    drop(queue);

    let state_clone = state.clone();

    {
        let mut is_queue_running = state.is_queue_running.write().await;
        if *is_queue_running {
            println!("Queue is already running,added only");
        } else {
            *is_queue_running = true;
            tokio::spawn(async move {
                BuildManager::process_queue(state_clone, project_name.clone()).await;
            });
        }
        drop(is_queue_running);
    }
    // Start build manager if not running

    // handle.abort();

    // Prepare response
    let mut response_data = json!({
        "success":true,
        "message":"Build is in pending state",
        "build_id": build_id,
        "data": json!({
            "socket_token":socket_token,
            "build_id":build_id,
        }),
        "state": "queued"
    });
    // Add custom return fields
    for return_field in &project_config.api.build.return_fields {
        let value = utils::resolve_variable(&return_field.value, &payload.payload, &socket_token);
        let key = return_field.name.as_ref().unwrap_or(&return_field.value);
        response_data[key] = json!(value);
    }

    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: true,
        state: "building".to_string(),
        message: "Build queued successfully".to_string(),
        data: Some(response_data),
    }))
}

async fn is_building_handler(
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
            state: "unauthorized".to_string(),
        }));
    }

    let projects = state.projects.read().await;
    let project_state = projects.get(&project_name).unwrap();

    let config = state.config.projects.get(&project_name);
    if let Some(pro) = config {
        let unique_key = &pro.build.unique_build_key;
        if payload.payload.contains_key(unique_key) {
            return Ok(HttpResponse::Unauthorized().json(BuildApiResponse {
                success: false,
                message: "Unique Key Not Found".to_string(),
                data: None,
                state: "unique_key_not_found".to_string(),
            }));
        }
    }

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

        return Ok(HttpResponse::Ok().json(BuildStatusResponse {
            is_building: true,
            queue_length: queue.len(),
            current_build: Some(build_info),
        }));
    } //if 

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
            state: "unauthorized".to_string(),
        }));
    }

    // TODO: Implement build abortion logic
    // BuildManager::abort_build(state.clone(), project_name, payload.payload.clone()).await;
    //
    // this is to abort only particular
    //

    let project_config = state.config.projects.get(&project_name).unwrap();

    let project_lock = state.projects.write().await;
    let project_state = project_lock.get(&project_name);
    if let Some(queues_main_lock) = project_state {
        let mut queues = queues_main_lock.build_queue.lock().await;

        // Find the index of the matching build
        if let Some(pos) = queues.iter().position(|build| {
            &build.unique_id
                == payload
                    .payload
                    .get(&project_config.build.unique_build_key)
                    .unwrap()
        }) {
            let build = queues.remove(pos); // Now safely remove it

            return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
                success: true,
                message: "Project is terminated".to_string(),
                state: "aborted".to_string(),
                data: None,
            }));
        }

        let current_build = queues_main_lock.current_build.lock().await;
        if let Some(cur) = current_build.as_ref() {
            if cur.unique_id
                == *payload
                    .payload
                    .get(&project_config.build.unique_build_key)
                    .unwrap()
            {
                let mut child = state.running_command_child.lock().await.take();
                if let Some(child) = child.as_mut() {
                    child.kill().await.unwrap();

                    let mut is_terminated = state.is_terminated.lock().await;
                    *is_terminated = true;
                }
                return Ok(HttpResponse::TooManyRequests().json(BuildApiResponse {
                    success: true,
                    message: "This is being running already..Killing".to_string(),
                    state: "aborted".to_string(),
                    data: None,
                }));
            }
        }
        drop(current_build);
    }
    drop(project_lock);

    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: false,
        message: "No any build found".to_string(),
        data: None,
        state: "not_found".to_string(),
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
            state: "unauthorized".to_string(),
        }));
    }

    // TODO: Implement cleanup logic
    BuildManager::cleanup_project(state.clone(), project_name, payload.payload.clone()).await;

    Ok(HttpResponse::Ok().json(BuildApiResponse {
        success: true,
        message: "Cleanup completed".to_string(),
        data: None,
        state: "success".to_string(),
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
