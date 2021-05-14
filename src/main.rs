use std::{env, io::Error as IoError, net::SocketAddr, sync::Arc};
use std::collections::{HashMap, HashSet};
use std::result::Result as StdResult;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, SinkExt, stream::TryStreamExt, StreamExt};
use log::*;
use parking_lot::Mutex;
use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_hdr_async, tungstenite::Error, WebSocketStream};
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::{http, Result};
use tungstenite::protocol::Message;
use serde_json::value::Value as JValue;

fn create_spaces_state() -> SpacesState {
    SpacesState {
        rooms: HashMap::new(),
        players: HashMap::new(),
        clients: HashMap::new(),
        next_player_id: 0,
    }
}

fn create_room(room_id: &String) -> Room {
    Room { id: room_id.clone(), participants: HashSet::new() }
}

fn create_player(player_id: PlayerId, room_id: String) -> Player {
    Player { id: player_id, room_id, pos: Pos { x: 0, y: 0 } }
}

struct Room {
    id: String,
    participants: HashSet<PlayerId>,
}

struct Player {
    id: PlayerId,
    room_id: RoomId,
    pos: XY
}

struct SpacesState {
    rooms: HashMap<RoomId, Room>,
    players: HashMap<PlayerId, Player>,
    clients: HashMap<PlayerId, Tx>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct PongServerMessage {
    pub id: i64,
    #[serde(rename = "type")]
    pub _type: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClientCommand {
    #[serde(rename = "move")]
    Move { pos: XY },
    #[serde(rename = "ping")]
    Ping { id: i64 },
}

pub struct XY {
    pub x: u32,
    pub y: u32,
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

fn random_id() -> String {
    return thread_rng()
        .sample_iter(&Alphanumeric)
        .take(22)
        .map(char::from)
        .collect();
}


type Tx = UnboundedSender<Message>;
type Ams = Arc<Mutex<SpacesState>>;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7000".to_string());
    let state = Arc::new(Mutex::new(create_spaces_state()));
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on: {}", addr);
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(state.clone(), stream, addr));
    }
}

// async fn accept_connection(state: Ams, stream: TcpStream, peer: SocketAddr) {
//     debug!("Incoming TCP connection from: {}", peer);
//     if let Err(e) = handle_connection(state, stream, peer).await {
//         match e {
//             Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
//             err => error!("Error processing connection: {}", err),
//         }
//     }
// }

async fn handle_connection(state: Ams, stream: TcpStream, peer: SocketAddr) {
    let mut callback = PathCapturingCallback { path: String::new() };
    let ws_stream = accept_hdr_async(stream, &mut callback).await.unwrap();
    let room_id = callback.path[ROOMS_PREFIX.len()..].to_owned();
    debug!("New WebSocket connection: {} {}", peer, room_id);

    let (tx, rx) = unbounded();
    let (outgoing, incoming) = ws_stream.split();

    let player_id = {
        let mut s = state.lock();
        let player_id = s.next_player_id.to_string();
        s.next_player_id += 1;
        let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
        room.participants.insert(player_id.clone());
        s.players.insert(player_id.clone(), create_player(room_id, player_id.clone()));

        let msg = LoginServerMessage { id: player_id.clone(), _type: "login".to_owned() };
        let data = serde_json::to_string(&msg).unwrap();
        let msg: Message = data.into();
        tx.unbounded_send(msg).unwrap();
        s.clients.insert(player_id.clone(), tx);

        player_id
    };


    let handle_incoming = incoming.try_for_each(|msg| {
        let vec = msg.into_data();
        let msg: JValue = serde_json::from_slice(&vec[..]).unwrap();
        debug!("Received a message from {} {}: {}", player_id, peer, msg.clone().to_string());
        if let JValue::Object(t) = msg {
            let mt = t.get("type");
            debug!("message of type: {}", mt.unwrap_or(&JValue::Null).to_string());
        }
        let s = state.lock();
        let broadcast_recipients =
            s.clients.iter().filter(|(id, _)| id != &&player_id).map(|(_, ws_sink)| ws_sink);

        for recp in broadcast_recipients {
            // recp.unbounded_send(msg.clone().into()).unwrap();
            recp.unbounded_send("pong".clone().into()).unwrap();
        }

        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(handle_incoming, receive_from_others);
    future::select(handle_incoming, receive_from_others).await;

    debug!("{} {} disconnected", &peer, player_id);
    state.lock().clients.remove(&player_id);
}
