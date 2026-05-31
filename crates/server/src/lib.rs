pub mod rooms;
pub mod ws;

use axum::{routing::get, Router};
use tower_http::services::ServeDir;
use ws::{ws_handler, AppState};

/// Build the application router. `dist` is the static client directory.
pub fn app(dist: String) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(dist))
        .with_state(AppState::new())
}
