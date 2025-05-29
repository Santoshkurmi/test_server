use actix_web::web::{self, Data};
use chrono::Utc;
use serde_json::json;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};

use crate::models::{
    AppState, BuildLog, BuildNextMessage, BuildProcess, BuildResult, BuildStatus, LogLevel,
    ServerMessage,
};
use crate::utils::{self, read_output_lines};

pub struct BuildManager;

impl BuildManager {
    pub async fn process_queue(state: web::Data<AppState>, project_name: String) {
        let projects = state.projects.read().await;
        let project_state = projects.get(&project_name).unwrap().clone();
        drop(projects);

        let mut is_first_run = true;

        let project_config = state.config.projects.get(&project_name).unwrap();

        println!("Processing build queue for project: {}", project_name);

        loop {
            println!("Processing build queue for project234: {}", project_name);

            // Check if we can start a new build
            let mut current_builds = project_state.current_build.lock().await;
            if current_builds.is_some() {
                break; //if some build is running, dont start a new one,
                //this is never goinng to happen, because the build manager is singleton
                // Put back in queue and wait
                // let mut queue = project_state.build_queue.lock().await;
                // queue.insert(0, build_request);
                // drop(queue);
                // drop(current_builds);
            }

            let mut queue = project_state.build_queue.lock().await;
            if queue.is_empty() {
                break;
            } //if queue is emtpy, stop the queue processing completely
            // no build is running,
            //then start the build process imedaitely without adding it in queue,

            //if its running already,adding it in queue and leave it there

            let build_request = queue.remove(0);
            drop(queue);

            // Create build process
            let build_process = BuildProcess {
                id: build_request.id.clone(),
                unique_id: build_request.unique_id.clone(),
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
            let build_request_for_spawn = build_request.clone();

            println!("Starting the build: {}", project_name);

            // let state_clone = Arc::clone(&state); // Assuming state is Arc<MyState>
            tokio::spawn(async move {
                // Self::test(state_clone, project_name_clone,build_id, build_request_for_spawn).await;
                Self::execute_build(
                    state_clone,
                    project_name_clone,
                    build_id,
                    build_request_for_spawn,
                    is_first_run,
                )
                .await;
            })
            .await
            .unwrap();

            is_first_run = false;

            let mut current_builds = project_state.current_build.lock().await;
            *current_builds = None;

            state.build_sender.send(ServerMessage::Shutdown);
        } //loop

        {
            let mut is_queue_running = state.is_queue_running.write().await;
            println!("Stopping build queue");
            *is_queue_running = false;
        }
    }

    async fn execute_build(
        state: actix_web::web::Data<AppState>,
        project_name: String,
        build_id: String,
        build_request: crate::models::BuildRequest,
        is_first_build: bool,
    ) {
        let project_config = state.config.projects.get(&project_name).unwrap().clone();
        let projects = state.projects.read().await;
        let project_state = projects.get(&project_name).unwrap().clone();
        drop(projects);
        let mut success = true;
        let mut step = 1;

        // Execute commands
        for command_config in &project_config.build.commands {
            let resolved_command =
                utils::resolve_command(&command_config.command, &build_request.payload);

            //Self::send_log(
            //    &state,
            //    &build_request.socket_token,
            //    &project_state,
            //    &build_id,
            //    step,
            //    LogLevel::Info,
            //    format!("Executing: {}", command_config.title),
            //    Some(resolved_command.clone()),
            //)
            //.await;

            {
                let mut is_terminated = state.is_terminated.lock().await;
                if *is_terminated {
                    println!("Build is terminated");
                    //send logs in here then again start the next project
                    *is_terminated = false;
                    success = false;
                    break;
                }
            }

            let result = Self::execute_command(&state, &resolved_command).await;

            match result {
                Ok(output) => {
                    println!("One command is done in here");
                }
                Err(error) => {
                    success = false;
                    break;
                }
            }

            step += 1;
        }

        // Execute success/failure commands
        let post_commands = if success {
            &project_config.build.run_on_success
            //handle error or failure in here for the build
        } else {
            &project_config.build.run_on_failure
        };
    }

    async fn execute_command(
        state: &actix_web::web::Data<AppState>,
        command: &str,
        step: usize,
    ) -> Result<String, String> {
        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn command: {}", e))?;
        {
            let is_terminated = state.is_terminated.lock().await;
            if *is_terminated {
                println!("Build is terminated");
                child.kill().await.unwrap();
                return Err("Child is killed".to_string());
            }
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        tokio::join!(
            read_output_lines(stdout, step, "success", state), //read_output_lines(stdout, step, "running", &state),
            read_output_lines(stderr, step, "error", state) //read_output_lines(stdout, step, "running", &state),
        );

        {
            //need to check here
            let mut running_command_child: tokio::sync::MutexGuard<
                '_,
                Option<tokio::process::Child>,
            > = state.running_command_child.lock().await;
            *running_command_child = Some(child);
        }

        let child_arc = state.running_command_child.clone();
        let mut child_arc = child_arc.lock().await;
        let mut child = child_arc.take();
        drop(child_arc);

        // *running_command_child = Some(child);
        if let Some(child_opt) = child.as_mut() {
            let status = child_opt
                .wait()
                .await
                .map_err(|e| format!("Failed to wait for command: {}", e))?;
            if status.success() {
                Ok("".to_string())
            } else {
                Err(format!("Command exited with status: {}", status))
            }
        } else {
            Err(format!("No running command"))
        }
    }

    pub async fn send_log(
        state: &Arc<AppState>,
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

        state.build_sender.send(ServerMessage::Data(
            serde_json::to_string(&ws_message).unwrap(),
        ));

        println!("Sending log to websocket: {}", ws_message.to_string());

        // state.websocket_manager.send_message(socket_token, &ws_message.to_string()).await;
    }

    async fn finalize_build(
        state: actix_web::web::Data<AppState>,
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
                project_name: build.project_name.clone(),
                status: status.clone(),
                started_at: build.started_at,
                completed_at,
                logs: build.logs,
                duration_seconds: duration,
            };

            let mut history = project_state.build_history.lock().await;
            history.push(result.clone());
            drop(history);
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

            // state.queue_sender.send(BuildNextMessage::Project(build.project_name.clone()));
        }

        drop(current_build);

        // Continue processing queue
        // Self::process_queue(state.clone(), build_request.project_name.clone()).await;
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
