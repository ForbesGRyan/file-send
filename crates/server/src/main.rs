#[tokio::main]
async fn main() {
    let dist = std::env::var("CLIENT_DIST").unwrap_or_else(|_| "crates/client/dist".to_string());
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("file-send server listening on {addr}");
    axum::serve(listener, server::app(dist)).await.unwrap();
}
