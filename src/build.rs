use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use actix_web::web;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use chrono::Utc;
use serde_json::json;

use crate::models::{AppState, BuildProcess, BuildStatus, BuildLog, LogLevel, BuildResult};
use crate::utils;

pub struct BuildManager;

impl BuildManager {
    pub async fn process_queue(state:  web::Data<AppState>, project_name: String) {
        let projects = state.projects.read().await;
        let project_state = projects.get(&project_name).unwrap().clone();
        drop(projects);
        
        let project_config = state.config.projects.get(&project_name).unwrap();
        
        loop {
            let mut queue = project_state.build_queue.lock().await;
            if queue.is_empty() {
                break;
            }
            
            let build_request = queue.remove(0);
            drop(queue);
            
            // Check if we can start a new build
            let mut current_builds = project_state.current_build.lock().await;
            if !project_config.allow_multi_build && current_builds.is_some() {
                // Put back in queue and wait
                let mut queue = project_state.build_queue.lock().await;
                queue.insert(0, build_request);
                drop(queue);
                drop(current_builds);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }
            
            // Create build process
            let build_process = BuildProcess {
                id: build_request.id.clone(),
                project_name: project_name.clone(),
                status: BuildStatus::Running,
                current_step: 0,
                total_steps: project_config.build.commands.len(),
                started_at: Utc::now(),
                socket_token: build_request.socket_token.clone(),
                logs: Vec::new(),
                handle: None,
            };
            
            *current_builds = Some(build_process);
            // *current_builds.insert(build_request.id.clone(), build_process);
            drop(current_builds);
            
            // Start build execution
            let state_clone = state.clone();
            let project_name_clone = project_name.clone();
            let build_id = build_request.id.clone();
            // let build_id_clone = build_id.clone();
            
            // let state_clone = Arc::clone(&state); // Assuming state is Arc<MyState>
            // let handle = tokio::spawn(async move {
            //     Self::execute_build(state_clone, project_name_clone, build_id, &build_request).await;
            // });
            // Update handle
            // let mut current_build = project_state.current_build.lock().await;
            // if let Some(build) = current_build.as_mut() {
            //     build.handle = Some(handle);
            // }
        }
    }
    
    async fn execute_build(
        state:  web::Data<AppState>,
        project_name: String,
        build_id: String,
        build_request: &crate::models::BuildRequest,
    ) {
        let project_config = state.config.projects.get(&project_name).unwrap().clone();
        let projects = state.projects.read().await;
        let project_state = projects.get(&project_name).unwrap().clone();
        drop(projects);
        
        let mut success = true;
        let mut step = 1;
        
        // Send initial log
        Self::send_log(
            &state,
            &build_request.socket_token,
            &project_state,
            &build_id,
            0,
            LogLevel::Info,
            "Build started".to_string(),
            None,
        ).await;
        
        // Execute commands
        for command_config in &project_config.build.commands {
            let resolved_command = utils::resolve_command(&command_config.command, &build_request.payload);
            
            Self::send_log(
                &state,
                &build_request.socket_token,
                &project_state,
                &build_id,
                step,
                LogLevel::Info,
                format!("Executing: {}", command_config.title),
                Some(resolved_command.clone()),
            ).await;
            
            let result = Self::execute_command(&resolved_command).await;
            
            match result {
                Ok(output) => {
                    if command_config.send_to_sock {
                        for line in output.lines() {
                            Self::send_log(
                                &state,
                                &build_request.socket_token,
                                &project_state,
                                &build_id,
                                step,
                                LogLevel::Info,
                                line.to_string(),
                                None,
                            ).await;
                        }
                    }
                }
                Err(error) => {
                    Self::send_log(
                        &state,
                        &build_request.socket_token,
                        &project_state,
                        &build_id,
                        step,
                        LogLevel::Error,
                        format!("Command failed: {}", error),
                        Some(resolved_command),
                    ).await;
                    
                    if command_config.on_error == "abort" {
                        success = false;
                        break;
                    }
                }
            }
            
            step += 1;
        }
        
        // Execute success/failure commands
        let post_commands = if success {
            &project_config.build.run_on_success
        } else {
            &project_config.build.run_on_failure
        };
        
        for command_config in post_commands {
            let resolved_command = utils::resolve_command(&command_config.command, &build_request.payload);
            let _ = Self::execute_command(&resolved_command).await;
        }
        
        // Update build status
        let final_status = if success { BuildStatus::Success } else { BuildStatus::Failed };
        Self::finalize_build(state, &project_state, &build_id, final_status, &project_config, &build_request).await;
    }
    
    async fn execute_command(command: &str) -> Result<String, String> {
        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;
        
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        
        let mut stdout_reader = BufReader::new(stdout);
        let mut stderr_reader = BufReader::new(stderr);
        
        let mut output = String::new();
        let mut stdout_line = String::new();
        let mut stderr_line = String::new();
        
        loop {
            tokio::select! {
                result = stdout_reader.read_line(&mut stdout_line) => {
                    match result {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            output.push_str(&stdout_line);
                            stdout_line.clear();
                        }
                        Err(e) => return Err(format!("Error reading stdout: {}", e)),
                    }
                }
                result = stderr_reader.read_line(&mut stderr_line) => {
                    match result {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            output.push_str(&stderr_line);
                            stderr_line.clear();
                        }
                        Err(e) => return Err(format!("Error reading stderr: {}", e)),
                    }
                }
            }
        }
        
        let status = child.wait().await.map_err(|e| format!("Failed to wait for command: {}", e))?;
        
        if status.success() {
            Ok(output)
        } else {
            Err(format!("Command exited with status: {}", status))
        }
    }
    
    async fn send_log(
        state: &Arc<AppState>,
        socket_token: &str,
        project_state: &crate::models::ProjectState,
        build_id: &str,
        step: usize,
        level: LogLevel,
        message: String,
        command: Option<String>,
    ) {
        let log = BuildLog {
            timestamp: Utc::now(),
            step,
            level: level.clone(),
            message: message.clone(),
            command,
        };
        
        // Add to build logs
        let mut current_build = project_state.current_build.lock().await;
        if let Some(build) = current_build.as_mut() {
            build.logs.push(log.clone());
            build.current_step = step;
        }
        drop(current_build);
        
        // Send to WebSocket
        let ws_message = json!({
            "type": "log",
            "build_id": build_id,
            "step": step,
            "level": level,
            "message": message,
            "timestamp": log.timestamp
        });
        
        // state.websocket_manager.send_message(socket_token, &ws_message.to_string()).await;
    }
    
    async fn finalize_build(
        state:  web::Data<AppState>,
        project_state: &crate::models::ProjectState,
        build_id: &str,
        status: BuildStatus,
        project_config: &crate::config::ProjectConfig,
        build_request: &crate::models::BuildRequest,
    ) {
        let completed_at = Utc::now();
        
        // Remove from current builds and add to history
        let mut current_build = project_state.current_build.lock().await;
        if let Some(build) = current_build.take() {
            let duration = (completed_at - build.started_at).num_seconds() as u64;
            
            let result = BuildResult {
                id: build.id,
                project_name: build.project_name,
                status: status.clone(),
                started_at: build.started_at,
                completed_at,
                logs: build.logs,
                duration_seconds: duration,
            };
            
            let mut history = project_state.build_history.lock().await;
            history.push(result.clone());
            
            // Send webhook notification
            let webhook_url = match status {
                BuildStatus::Success => &project_config.build.on_success,
                _ => &project_config.build.on_failure,
            };
            
            if !webhook_url.is_empty() {
                utils::send_webhook(webhook_url, &result, &build_request.payload).await;
            }
            
            // Save logs
            utils::save_build_logs(&state.config.log_path, &result).await;
        }
        
        // Continue processing queue
        Self::process_queue(state.clone(), build_request.project_name.clone()).await;
    }
    
    pub async fn abort_build(
        state: actix_web::web::Data<AppState>,
        project_name: String,
        payload: HashMap<String, serde_json::Value>,
    ) {
        // TODO: Implement build abortion logic
        log::info!("Aborting build for project: {}", project_name);
    }
    
    pub async fn cleanup_project(
        state: actix_web::web::Data<AppState>,
        project_name: String,
        payload: HashMap<String, serde_json::Value>,
    ) {
        // TODO: Implement cleanup logic
        log::info!("Cleaning up project: {}", project_name);
    }
}