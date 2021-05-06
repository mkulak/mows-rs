use std::result::Result as StdResult;
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc};
// use std::sync::{Mutex};
use tokio::sync::{Mutex};

use futures_util::{SinkExt, StreamExt};
use log::*;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_hdr_async, tungstenite::Error};
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::{http, Result};
use std::thread;


#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let addr = "127.0.0.1:9002";
    let listener = TcpListener::bind(&addr).await.expect("Can't listen");
    info!("Listening on: {}", addr);

    let spaces_server = Arc::new(Mutex::new(SpaceServer { rooms: HashMap::new() }));
    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream.peer_addr().expect("connected streams should have a peer address");
        info!("Peer address: {}", peer);
        let rc = spaces_server.clone();
        tokio::spawn(accept_connection(rc, peer, stream));
    }
}
// fn main() {
//     let counter = Arc::new(Mutex::new(0));
//     let mut handles = vec![];
//     let mut a: i32 = 0;
//     inc(&mut a);
//     println!("a: {}", a);
//
//
//     for _ in 0..10 {
//         let counter = Arc::clone(&counter);
//         let handle = thread::spawn(move || {
//             let mut num = counter.lock().unwrap();
//             inc(&mut (*num))
//             // *num += 1;
//         });
//         handles.push(handle);
//     }
//
//     for handle in handles {
//         handle.join().unwrap();
//     }
//
//     println!("Result: {}", *counter.lock().unwrap());
// }
//
// fn inc(value: &mut i32) {
//     *value += 1;
// }

struct Room {
    id: String,
    next_user_id: u32,
}

struct SpaceServer {
    rooms: HashMap<String, Room>,
}

async fn accept_connection(mut space: Arc<Mutex<SpaceServer>>, peer: SocketAddr, stream: TcpStream) {
    if let Err(e) = handle_connection(space, peer, stream).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(mut space: Arc<Mutex<SpaceServer>>, peer: SocketAddr, stream: TcpStream) -> Result<()> {
    let mut cb = PathCapturingCallback {
        path: String::new()
    };
    let mut ws_stream = accept_hdr_async(stream, &mut cb).await?;
    let room_id = cb.path[ROOMS_PREFIX.len()..].to_owned();
    info!("New WebSocket connection: {} {}", peer, room_id);
    let id = {
        let mut s = space.lock().await;
        let room = s.rooms.entry(room_id.clone())
            .or_insert_with(|| Room { id: room_id.clone(), next_user_id: 0 });
        let id = room.next_user_id.to_string();
        room.next_user_id += 1;
        id
    };
    let msg = LoginServerMessage { id, _type: "login".to_owned() };
    let data = serde_json::to_string(&msg).unwrap();
    ws_stream.send(data.into()).await?;

    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if msg.is_text() || msg.is_binary() {
            ws_stream.send(msg).await?;
        }
    }

    Ok(())
}


#[derive(Debug, Serialize, Deserialize)]
pub struct LoginServerMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub _type: String,
}

const ROOMS_PREFIX: &'static str = "/rooms/";

struct PathCapturingCallback {
    path: String,
}

impl Callback for &mut PathCapturingCallback {
    fn on_request(self, request: &Request, response: Response) -> StdResult<Response, ErrorResponse> {
        let path = request.uri().path();
        if !path.starts_with(ROOMS_PREFIX) {
            return Err(http::response::Response::builder()
                .status(http::StatusCode::NOT_FOUND)
                .body(None)
                .unwrap());
        }
        (&mut self.path).push_str(path);
        Ok(response)
    }
}
