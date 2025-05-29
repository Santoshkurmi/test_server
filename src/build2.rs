use models::{SharedState};

use crate::handle_error_success::handle_error_success;
use crate::models;
use crate::util::{read_output_lines, send_output};
use std::sync::Arc;
use tokio::process::Command;


pub async fn build(state: Arc<SharedState>) {

    
    let commands = state.config.lock().await.commands.clone();
    let mut step = 1;

    for cmd in commands {
        println!("Running command: {}\n", cmd.command);
        send_output(&state, step, "running", &format!("Running command: {}\n", cmd.command)).await;
        
        let  child = Command::new("bash")
            .arg("-c")
            .arg(cmd.command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                handle_error_success(&state, "error".to_string()).await;

                send_output(&state, step, "error", &format!("Failed to spawn: {}", e)).await;
                break;
            }
        };

      

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Read both stdout and stderr concurrently
        tokio::join!(
            read_output_lines(stdout, step, "running", &state),
            read_output_lines(stderr, step, "error", &state)
        );

        let status = child.wait().await;
        match status {
            Ok(status) if status.success() => {
                step += 1;
            }
            Ok(_status) => {
                handle_error_success(&state, "error".to_string()).await;

                // handle_error_success(&state,"error".to_string()).await;
                break;
        }

            Err(e) => {
            handle_error_success(&state, "error".to_string()).await;
            send_output(&state, step, "error", &format!("Failed to wait for command: {}", e)).await;
                    break;
               
            }
        }
    }//loop

    handle_error_success(&state, "success".to_string()).await;

}




