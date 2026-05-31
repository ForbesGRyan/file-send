mod rooms;
mod ws;

use axum::{routing::get, Router};
use tower_http::services::ServeDir;
use ws::{ws_handler, AppState};

#[tokio::main]
async fn main() {
    let state = AppState::new();

    // Directory of the built client (Trunk output). Override with CLIENT_DIST.
    let dist = std::env::var("CLIENT_DIST").unwrap_or_else(|_| "crates/client/dist".to_string());

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(dist))
        .with_state(state);

    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("file-send server listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
