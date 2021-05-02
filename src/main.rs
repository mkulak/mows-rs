use std::{result::Result as StdResult};
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use log::*;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_hdr_async, tungstenite::Error};
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::{http, Result};

async fn accept_connection(peer: SocketAddr, stream: TcpStream) {
    if let Err(e) = handle_connection(peer, stream).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

struct PathCapturingCallback {
    path: String,
}

impl Callback for &mut PathCapturingCallback {
    fn on_request(self, request: &Request, response: Response) -> StdResult<Response, ErrorResponse> {
        let path = request.uri().path();
        if !path.starts_with("/rooms/") {
            return Err(http::response::Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body(None)
                .unwrap());
        }
        (&mut self.path).push_str(path);
        Ok(response)
    }
}

async fn handle_connection(peer: SocketAddr, stream: TcpStream) -> Result<()> {
    let mut cb = PathCapturingCallback {
        path: String::new()
    };
    let mut ws_stream = accept_hdr_async(stream, &mut cb).await?;
    let id = "123".to_owned();
    let msg = LoginServerMessage { id, _type: "login".to_owned() };
    let data = serde_json::to_string(&msg).unwrap();
    ws_stream.send(data.into()).await?;
    info!("New WebSocket connection: {} {}", peer, cb.path);

    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if msg.is_text() || msg.is_binary() {
            ws_stream.send(msg).await?;
        }
    }

    Ok(())
}


#[tokio::main]
async fn main() {
    env_logger::init();

    let addr = "127.0.0.1:9002";
    let listener = TcpListener::bind(&addr).await.expect("Can't listen");
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream.peer_addr().expect("connected streams should have a peer address");
        info!("Peer address: {}", peer);

        tokio::spawn(accept_connection(peer, stream));
    }
}


#[derive(Debug, Serialize, Deserialize)]
pub struct LoginServerMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub _type: String,
}
