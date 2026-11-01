//! A fake TV for the tests: a WebSocket server on the loopback interface whose behavior each test
//! scripts frame by frame.

#![allow(dead_code, reason = "each test binary uses a different part")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, accept_async};
use webos::{ClientKey, Endpoint, Manifest, Options, Paired, Timing, Tls};

pub const UUID: &str = "4f1c2a7e-0b3d-4e6a-9c58-2d7f1e0a6b93";
pub const KEY: &str = "5a8e3c1f9d2b4a6e7c0f1b3d5e7a9c2f";

pub type Socket = WebSocketStream<TcpStream>;

/// Serves every connection with `serve`, which also gets the connection's number from 0.
pub async fn tv<F, Fut>(serve: F) -> SocketAddr
where
    F: Fn(Socket, usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serve = Arc::new(serve);
    tokio::spawn(async move {
        let mut count = 0;
        while let Ok((tcp, _)) = listener.accept().await {
            let socket = accept_async(tcp).await.unwrap();
            tokio::spawn(serve(socket, count));
            count += 1;
        }
    });
    address
}

pub fn endpoint(address: SocketAddr) -> Endpoint {
    Endpoint {
        address,
        secure: false,
    }
}

pub fn paired() -> Paired {
    Paired {
        uuid: UUID.into(),
        key: ClientKey::new(KEY),
        manifest: Manifest::Signed,
    }
}

/// Options for a plain connection, with the default timing.
pub fn options() -> Options {
    timed(Timing::default())
}

pub fn timed(timing: Timing) -> Options {
    Options {
        tls: Tls::accepting(Vec::new()).unwrap(),
        timing,
    }
}

/// The next frame the client sent, skipping pings and pongs; `None` once it closed.
pub async fn recv<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> Option<Value> {
    loop {
        match socket.next().await? {
            Ok(Message::Text(text)) => return Some(serde_json::from_str(text.as_str()).unwrap()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

pub async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    frame: Value,
) {
    socket.send(Message::text(frame.to_string())).await.unwrap();
}

/// Answers the client's `hello` as the TV with `uuid`.
pub async fn greet<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>, uuid: &str) {
    let hello = recv(socket).await.unwrap();
    assert_eq!(hello["type"], "hello");
    send(
        socket,
        json!({"type": "hello", "id": hello["id"], "payload": {
            "protocolVersion": 1, "deviceType": "tv", "deviceOS": "webOS",
            "deviceOSVersion": "4.1.0", "deviceOSReleaseVersion": "6.5.3",
            "deviceUUID": uuid, "pairingTypes": ["PIN", "PROMPT", "COMBINED"]}}),
    )
    .await;
}

/// Greets the client and accepts its key; returns the register frame.
pub async fn welcome(socket: &mut Socket) -> Value {
    greet(socket, UUID).await;
    let register = recv(socket).await.unwrap();
    assert_eq!(register["type"], "register");
    send(
        socket,
        json!({"type": "registered", "id": register["id"], "payload": {"client-key": KEY}}),
    )
    .await;
    register
}

/// Answers a request frame with `payload`.
pub async fn answer(socket: &mut Socket, request: &Value, payload: Value) {
    send(
        socket,
        json!({"type": "response", "id": request["id"], "payload": payload}),
    )
    .await;
}

/// Reads until the client closes, which also answers its pings.
pub async fn drain(socket: &mut Socket) {
    while recv(socket).await.is_some() {}
}

/// Short timing for tests about timing.
pub fn quick() -> Timing {
    Timing {
        connect: Duration::from_secs(2),
        request: Duration::from_millis(300),
        ping_every: Duration::from_millis(50),
        silence: Duration::from_millis(250),
        key_grace: Duration::from_millis(200),
    }
}
