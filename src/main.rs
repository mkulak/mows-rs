use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::rc::Rc;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::thread;

use futures_util::{SinkExt, StreamExt};
use log::*;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_hdr_async, tungstenite::Error, WebSocketStream};
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::{http, Result};

use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let addr = "127.0.0.1:9002";
    let listener = TcpListener::bind(&addr).await.expect("Can't listen");
    info!("Listening on: {}", addr);

    let spaces_server = Arc::new(Mutex::new(create_spaces_state()));
    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream.peer_addr().expect("connected streams should have a peer address");
        info!("Peer address: {}", peer);
        let rc = spaces_server.clone();
        tokio::spawn(accept_connection(rc, peer, stream));
    }
}

async fn accept_connection(mut space: Arc<Mutex<SpacesState>>, peer: SocketAddr, stream: TcpStream) {
    if let Err(e) = handle_connection(space, peer, stream).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(mut space: Arc<Mutex<SpacesState>>, peer: SocketAddr, stream: TcpStream) -> Result<()> {
    let mut cb = PathCapturingCallback {
        path: String::new()
    };
    let mut ws_stream = accept_hdr_async(stream, &mut cb).await?;
    let room_id = cb.path[ROOMS_PREFIX.len()..].to_owned();
    info!("New WebSocket connection: {} {}", peer, room_id);
    let id = {
        let mut s = space.lock();
        let id = s.next_player_id.to_string();
        s.next_player_id += 1;
        let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
        room.participants.insert(id.clone());
        id
    };
    let msg = LoginServerMessage { id, _type: "login".to_owned() };
    let data = serde_json::to_string(&msg).unwrap();
    ws_stream.send(data.into()).await?;

    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if msg.is_text() {
            // let serde_json::from_slice(msg.into());
            info!("got: {}", msg.clone().into_text().unwrap());
            ws_stream.send(msg).await?;
        }
    }

    Ok(())
}

fn create_spaces_state() -> SpacesState {
    SpacesState { rooms: HashMap::new(), clients: HashMap::new(), next_player_id: 0 }
}

fn create_room(room_id: &String) -> Room {
    Room { id: room_id.clone(), participants: HashSet::new() }
}

struct Room {
    id: String,
    participants: HashSet<PlayerId>
}

struct SpacesState {
    rooms: HashMap<RoomId, Room>,
    clients: HashMap<PlayerId, WebSocketStream<TcpStream>>,
    next_player_id: u32,
}
type RoomId = String;

type PlayerId = String;


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

fn nextId() -> String {
    return thread_rng()
        .sample_iter(&Alphanumeric)
        .take(22)
        .map(char::from)
        .collect();
}
