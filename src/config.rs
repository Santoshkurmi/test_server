use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::Result;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub name: String,
    pub port: u16,
    pub base_path: String,
    pub log_path: String,
    pub ssl: SslConfig,
    pub auth: AuthConfig,
    pub projects: HashMap<String, ProjectConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SslConfig {
    pub enable_ssl: bool,
    pub certificate_path: String,
    pub certificate_key_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub auth_type: String, // "token", "address", "both"
    pub address_type: String, // "ip", "hostname"
    pub allowed_addresses: Vec<String>,
    pub allowed_tokens: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectConfig {
    pub allow_multi_build: bool,
    pub max_pending_build: u32,
    pub base_endpoint_path: String,
    pub api: ApiConfig,
    pub auth: Option<AuthConfig>,
    pub build: BuildConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiConfig {
    pub build: EndpointConfig,
    pub is_building: EndpointConfig,
    pub abort: EndpointConfig,
    pub cleanup: EndpointConfig,
    pub socket: EndpointConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EndpointConfig {
    pub endpoint: String,
    pub method: String,
    pub payload: Vec<String>,
    #[serde(default)]
    pub return_fields: Vec<ReturnField>,
    #[serde(default)]
    pub file: Vec<FileConfig>,
}
 
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReturnField {
    pub name: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileConfig {
    pub name: String,
    pub path: String,
    pub on_err_suc: String, // "del", "keep"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BuildConfig {
    pub project_path: String,
    pub on_success: String,
    pub on_failure: String,
    pub on_success_payload: Vec<String>,
    pub on_failure_payload: Vec<String>,
    pub commands: Vec<CommandConfig>,
    #[serde(default)]
    pub run_on_success: Vec<CommandConfig>,
    #[serde(default)]
    pub run_on_failure: Vec<CommandConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandConfig {
    pub command: String,
    pub title: String,
    #[serde(default)]
    pub on_error: String, // "abort", "continue"
    #[serde(default)]
    pub send_to_sock: bool,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}