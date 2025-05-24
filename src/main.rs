use actix_web::{web, App, HttpServer, middleware::Logger};
use std::sync::Arc;
use tokio::sync::Mutex;
use openssl::ssl::{SslAcceptor, SslMethod, SslFiletype};

mod config;
mod models;
mod auth;
mod handlers;
mod build;
mod websocket;
mod utils;

use config::Config;
use models::AppState;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    
    // Load configuration
    let config = Config::load("config.toml").expect("Failed to load config");
    let port = config.port;
    let ssl_enabled = config.ssl.enable_ssl;
    
    let certificate_key_path = config.ssl.certificate_key_path.clone();
    let cetificate_path = config.ssl.certificate_path.clone();

    
    
    // Create shared application state
    let app_state = AppState::new(config).await;
    let app_data = web::Data::new(app_state);
   
    log::info!("Starting server on port {}", port);
    
    let server = HttpServer::new(move || {
        let mut app = App::new()
            .app_data(app_data.clone()) //need to see here
            // .wrap(Logger::default())
            .service(handlers::health_check)
            .service(websocket::websocket_handler);
            
        // Dynamically register project routes
        app = handlers::register_project_routes(app, &app_data.config);
        app
    });
    
    if ssl_enabled {
        let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        builder.set_private_key_file(&certificate_key_path, SslFiletype::PEM).unwrap();
        builder.set_certificate_chain_file(&cetificate_path).unwrap();
        
        server.bind_openssl(format!("0.0.0.0:{}", port), builder)?.run().await
    } else {
        server.bind(("0.0.0.0", port))?.run().await
    }
}