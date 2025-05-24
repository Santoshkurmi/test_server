use actix_web::{HttpRequest, web};
use std::net::IpAddr;
use std::sync::Arc;

use crate::config::{AuthConfig, Config};
use crate::models::AppState;

pub async fn is_authorized(
    req: &HttpRequest,
    state:  &web::Data<AppState>,
    project_name: Option<&str>,
) -> bool {
    let auth_config = if let Some(project) = project_name {
        if let Some(project_config) = state.config.projects.get(project) {
            project_config.auth.as_ref().unwrap_or(&state.config.auth)
        } else {
            &state.config.auth
        }
    } else {
        &state.config.auth
    };
    
    match auth_config.auth_type.as_str() {
        "token" => check_token_auth(req, auth_config),
        "address" => check_address_auth(req, auth_config),
        "both" => check_token_auth(req, auth_config) && check_address_auth(req, auth_config),
        _ => false,
    }
}

fn check_token_auth(req: &HttpRequest, auth_config: &AuthConfig) -> bool {
    if let Some(auth_header) = req.headers().get("Authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str.starts_with("Bearer ") {
                let token = &auth_str[7..];
                return auth_config.allowed_tokens.contains(&token.to_string());
            }
        }
    }
    
    // Also check query parameter
    if let Some(query_string) = req.uri().query() {
        for pair in query_string.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if key == "token" && auth_config.allowed_tokens.contains(&value.to_string()) {
                    return true;
                }
            }
        }
    }
    
    false
}

fn check_address_auth(req: &HttpRequest, auth_config: &AuthConfig) -> bool {
    let conn_info = req.connection_info();
    let remote_addr = conn_info.realip_remote_addr().unwrap_or("unknown");
    
    match auth_config.address_type.as_str() {
        "ip" => {
            if let Ok(ip) = remote_addr.parse::<IpAddr>() {
                auth_config.allowed_addresses.contains(&ip.to_string())
            } else {
                false
            }
        }
        "hostname" => {
            auth_config.allowed_addresses.contains(&remote_addr.to_string())
        }
        _ => false,
    }
}

pub fn extract_bearer_token(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get("Authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.to_string())
}