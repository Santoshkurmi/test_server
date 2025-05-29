use chrono::Utc;
use rand::{Rng, distributions::Alphanumeric};
use reqwest::Client;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use std::io::Write;
use std::path::Path;

use crate::build::BuildManager;
use crate::models::AppState;
use crate::models::BuildResult;

pub fn generate_token(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}
pub async fn read_output_lines(
    stream: Option<impl tokio::io::AsyncRead + Unpin>,
    step: usize,
    status: &str,
    state: &actix_web::web::Data<AppState>,
) {
    if let Some(output) = stream {
        let reader = BufReader::new(output);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            //send_output(state, step, status, &line).await;
            BuildManager::send_log(
                state,
                project_state,
                build_id,
                step,
                level,
                message,
                command,
            )
        }
    }
}

pub fn resolve_variable(
    variable: &str,
    payload: &HashMap<String, Value>,
    socket_token: &str,
) -> String {
    match variable {
        "%status%" => "queued".to_string(),
        "%socket_token%" => socket_token.to_string(),
        var if var.starts_with('$') => {
            let env_var = &var[1..];
            env::var(env_var).unwrap_or_else(|_| {
                // Try to get from payload
                payload
                    .get(env_var)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
        }
        var => payload
            .get(var)
            .and_then(|v| v.as_str())
            .unwrap_or(var)
            .to_string(),
    }
}

pub fn resolve_command(command: &str, payload: &HashMap<String, Value>) -> String {
    command
        .replace("${payload}", &json!(payload).to_string())
        .replace("${timestamp}", &Utc::now().to_rfc3339())
}

pub async fn send_webhook(
    webhook_url: &str,
    result: &BuildResult,
    payload: &HashMap<String, Value>,
) {
    let webhook_url = webhook_url.replace("${payload}", &json!(payload).to_string());
    let webhook_url = webhook_url.replace("${result}", &json!(result).to_string());

    let client = Client::new();
    let _ = client.post(webhook_url).send().await;
}

pub async fn save_build_logs(log_path: &str, result: &BuildResult) {
    let log_path = Path::new(log_path);
    let log_file_path = log_path.join(format!("{}.log", result.id));

    if log_file_path.exists() {
        fs::remove_file(&log_file_path).unwrap();
    }

    fs::create_dir_all(log_path).unwrap();

    let mut log_file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(log_file_path)
        .unwrap();

    for log in &result.logs {
        let _ = log_file
            .write_all(format!("{}\n", log.message).as_bytes())
            .unwrap();
    }
}

