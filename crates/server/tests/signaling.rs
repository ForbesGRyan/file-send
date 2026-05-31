use futures_util::{SinkExt, StreamExt};
use shared::SignalMsg;
use tokio_tungstenite::tungstenite::Message;

/// Start the server on an ephemeral port; return its base ws URL.
async fn start_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = server::app("nonexistent-dist".to_string());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{addr}/ws")
}

async fn connect(url: &str) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    stream
}

async fn send(ws: &mut (impl SinkExt<Message> + Unpin), msg: &SignalMsg) {
    let json = serde_json::to_string(msg).unwrap();
    let _ = ws.send(Message::Text(json.into())).await;
}

async fn recv(ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> SignalMsg {
    loop {
        let msg = ws.next().await.unwrap().unwrap();
        if let Message::Text(text) = msg {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

#[tokio::test]
async fn create_join_relay_flow() {
    let url = start_server().await;

    // Peer A creates a room.
    let mut a = connect(&url).await;
    send(&mut a, &SignalMsg::Create).await;
    let room = match recv(&mut a).await {
        SignalMsg::Created { room } => room,
        other => panic!("expected Created, got {other:?}"),
    };

    // Peer B joins it.
    let mut b = connect(&url).await;
    send(&mut b, &SignalMsg::Join { room: room.clone() }).await;

    // A is told the peer joined.
    assert_eq!(recv(&mut a).await, SignalMsg::PeerJoined);

    // A sends an SDP offer; B receives it verbatim.
    send(&mut a, &SignalMsg::Sdp { sdp: "OFFER".into(), kind: "offer".into() }).await;
    assert_eq!(recv(&mut b).await, SignalMsg::Sdp { sdp: "OFFER".into(), kind: "offer".into() });
}

#[tokio::test]
async fn third_peer_gets_room_full() {
    let url = start_server().await;
    let mut a = connect(&url).await;
    send(&mut a, &SignalMsg::Create).await;
    let room = match recv(&mut a).await {
        SignalMsg::Created { room } => room,
        other => panic!("got {other:?}"),
    };
    let mut b = connect(&url).await;
    send(&mut b, &SignalMsg::Join { room: room.clone() }).await;
    let _ = recv(&mut a).await; // PeerJoined

    let mut c = connect(&url).await;
    send(&mut c, &SignalMsg::Join { room }).await;
    assert_eq!(recv(&mut c).await, SignalMsg::RoomFull);
}

#[tokio::test]
async fn join_unknown_room() {
    let url = start_server().await;
    let mut a = connect(&url).await;
    send(&mut a, &SignalMsg::Join { room: "ghost".into() }).await;
    assert_eq!(recv(&mut a).await, SignalMsg::RoomNotFound);
}

#[tokio::test]
async fn partner_notified_on_disconnect() {
    let url = start_server().await;
    let mut a = connect(&url).await;
    send(&mut a, &SignalMsg::Create).await;
    let room = match recv(&mut a).await {
        SignalMsg::Created { room } => room,
        other => panic!("got {other:?}"),
    };
    let mut b = connect(&url).await;
    send(&mut b, &SignalMsg::Join { room }).await;
    assert_eq!(recv(&mut a).await, SignalMsg::PeerJoined);

    // B disconnects; A should be told PeerLeft.
    drop(b);
    assert_eq!(recv(&mut a).await, SignalMsg::PeerLeft);
}
