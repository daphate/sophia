mod claude;
mod config;
mod convert;
mod handlers;
mod types;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    config::load_dotenv();
    let config = config::ProxyConfig::from_env();
    let addr = config.bind_addr;

    let app = Router::new()
        // Anthropic Messages API
        .route("/v1/messages", post(handlers::create_message))
        // Models (convenience)
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/models/{model_id}", get(handlers::get_model))
        .route("/health", get(|| async { "ok" }))
        .layer(CorsLayer::permissive())
        .with_state(config);

    info!("Sophia proxy listening on {addr} (Anthropic Messages API)");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
