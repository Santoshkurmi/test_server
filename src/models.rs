use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{
    Mutex, RwLock,
    broadcast::{self, Sender},
};
use uuid::Uuid;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub projects: Arc<RwLock<HashMap<String, ProjectState>>>,
    pub websocket_manager: Arc<WebSocketManager>,
    pub project_sender: broadcast::Sender<ServerMessage>,
    pub build_sender: broadcast::Sender<ServerMessage>,
    pub queue_sender: broadcast::Sender<BuildNextMessage>,
    pub is_queue_running: Arc<RwLock<bool>>,
    pub running_command_child: Arc<Mutex<Option<tokio::process::Child>>>,
    pub is_terminated: Arc<Mutex<bool>>,
}

#[derive(Clone)]
pub struct ProjectState {
    pub build_queue: Arc<Mutex<Vec<BuildRequest>>>,
    pub current_build: Arc<Mutex<Option<BuildProcess>>>,
    pub build_history: Arc<Mutex<Vec<BuildResult>>>,
}

#[derive(Clone)]
pub struct BuildRequest {
    pub id: String,
    pub project_name: String,
    pub unique_id: String,
    pub payload: HashMap<String, serde_json::Value>,
    pub files: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub socket_token: String,
}

// #[derive()]
pub struct BuildProcess {
    pub id: String,
    pub project_name: String,
    pub unique_id: String,
    pub status: BuildStatus,
    pub current_step: usize,
    pub total_steps: usize,
    pub started_at: DateTime<Utc>,
    pub socket_token: String,
    pub logs: Vec<BuildLog>,
    pub handle: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for BuildProcess {
    fn clone(&self) -> Self {
        Self {
            unique_id: self.unique_id.clone(),
            id: self.id.clone(),
            project_name: self.project_name.clone(),
            status: self.status.clone(),
            current_step: self.current_step,
            total_steps: self.total_steps,
            started_at: self.started_at,
            socket_token: self.socket_token.clone(),
            logs: self.logs.clone(),
            handle: None, // Clone skips the task handle
        }
    }
}

// #[derive(Clone)]
// pub struct BuildProcess {
//     pub id: String,
//     pub project_name: String,
//     pub status: BuildStatus,
//     pub current_step: usize,
//     pub total_steps: usize,
//     pub started_at: DateTime<Utc>,
//     pub socket_token: String,
//     pub logs: Vec<BuildLog>,
//     pub handle: Option<tokio::task::JoinHandle<()>>,
// }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BuildStatus {
    Queued,
    Running,
    Success,
    Failed,
    Aborted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildLog {
    pub timestamp: DateTime<Utc>,
    pub step: usize,
    pub level: LogLevel,
    pub message: String,
    pub command: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Clone)]
pub enum ServerMessage {
    Data(String),
    Shutdown,
}

#[derive(Clone)]
pub enum BuildNextMessage {
    Project(String),
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildResult {
    pub id: String,
    pub project_name: String,
    pub status: BuildStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub logs: Vec<BuildLog>,
    pub duration_seconds: u64,
}

#[derive(Clone)]
pub struct WebSocketManager {
    pub connections: Arc<Mutex<HashMap<String, Vec<broadcast::Sender<String>>>>>,
}

// API Request/Response models
#[derive(Deserialize, Serialize)]
pub struct BuildApiRequest {
    #[serde(flatten)]
    pub payload: HashMap<String, serde_json::Value>,
}

#[derive(Serialize)]
pub struct BuildApiResponse {
    pub success: bool,
    pub message: String,
    pub state: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct BuildStatusResponse {
    pub is_building: bool,
    pub queue_length: usize,
    pub current_build: Option<BuildInfo>,
}

#[derive(Serialize)]
pub struct BuildInfo {
    pub id: String,
    pub status: BuildStatus,
    pub current_step: usize,
    pub total_steps: usize,
    pub socket_token: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct WebSocketQuery {
    pub token: String,
}

impl AppState {
    pub async fn new(
        config: Config,
        project_sender: Sender<ServerMessage>,
        build_sender: Sender<ServerMessage>,
        queue_sender: Sender<BuildNextMessage>,
    ) -> Self {
        let mut projects = HashMap::new();

        for (name, _) in &config.projects {
            projects.insert(
                name.clone(),
                ProjectState {
                    build_queue: Arc::new(Mutex::new(Vec::new())),
                    current_build: Arc::new(Mutex::new(None)),
                    build_history: Arc::new(Mutex::new(Vec::new())),
                },
            );
        }

        Self {
            config,
            is_terminated: Arc::new(Mutex::new(false)),
            running_command_child: Arc::new(Mutex::new(None)),
            project_sender,
            build_sender,
            queue_sender,
            is_queue_running: Arc::new(RwLock::new(false)),

            projects: Arc::new(RwLock::new(projects)),
            websocket_manager: Arc::new(WebSocketManager {
                connections: Arc::new(Mutex::new(HashMap::new())),
            }),
        }
    }
}

impl WebSocketManager {
    // pub async fn add_connection(&self, token: &str,sender:Sender<String>) -> broadcast::Receiver<String> {
    //     let mut connections = self.connections.lock().await;
    //     let mut list = connections.get(token).unwrap_or(&Vec::new()).clone();
    //     let new_receiver = sender.subscribe();

    //     list.push(sender);
    //     connections.insert(token.to_string(), list);

    //     new_receiver
    // }

    // pub async fn send_message(&self, token: &str, message: &str) {
    //     let connections = self.connections.lock().await;
    //     if let Some(sender) = connections.get(token) {
    //         let _ = sender.send(message.to_string());
    //     }
    // }

    // pub async fn remove_connection(&self, token: &str) {
    //     let mut connections = self.connections.lock().await;
    //     connections.remove(token);
    // }
}
