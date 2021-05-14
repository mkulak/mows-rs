use std::{env, sync::Arc};
use std::collections::{HashMap, HashSet};
use std::result::Result as StdResult;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};
use log::*;
use parking_lot::Mutex;
use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::http;
use tungstenite::protocol::Message;

fn create_spaces_state() -> SpacesState {
    SpacesState {
        rooms: HashMap::new(),
        players: HashMap::new(),
        clients: HashMap::new(),
        players_with_updates: HashSet::new(),
        next_player_id: 0,
    }
}

fn create_room(room_id: &String) -> Room {
    Room { id: room_id.clone(), participants: HashSet::new() }
}

fn create_player(player_id: PlayerId, room_id: RoomId) -> Player {
    Player { id: player_id, room_id, pos: XY { x: 0, y: 0 } }
}

struct Room {
    id: String,
    participants: HashSet<PlayerId>,
}

struct Player {
    id: PlayerId,
    room_id: RoomId,
    pos: XY,
}

struct SpacesState {
    rooms: HashMap<RoomId, Room>,
    players: HashMap<PlayerId, Player>,
    clients: HashMap<PlayerId, Tx>,
    players_with_updates: HashSet<PlayerId>,
    next_player_id: u32,
}


type RoomId = String;

type PlayerId = u32;

#[derive(Debug, Serialize, Deserialize)]
enum ServerMessage {
    Login { id: PlayerId, #[serde(rename = "type")] _type: String },
    Pong { id: i64, #[serde(rename = "type")] _type: String },
    FullUpdate { room_id: String, players: HashMap<PlayerId, XY>, #[serde(rename = "type")] _type: String }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClientCommand {
    #[serde(rename = "move")]
    Move { pos: XY },
    #[serde(rename = "ping")]
    Ping { id: i64 },
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
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
    info!("Listening on: {}", addr);
    while let Ok((stream, _addr)) = listener.accept().await {
        tokio::spawn(handle_connection(state.clone(), stream));
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

async fn handle_connection(state: Ams, stream: TcpStream) {
    let mut callback = PathCapturingCallback { path: String::new() };
    let ws_stream = accept_hdr_async(stream, &mut callback).await.unwrap();
    let room_id = callback.path[ROOMS_PREFIX.len()..].to_owned();

    let (tx, rx) = unbounded();
    let (outgoing, incoming) = ws_stream.split();

    let player_id = on_join(tx, &room_id, state.clone());
    debug!("Connected {} room {}", player_id, room_id.clone());

    let handle_incoming = incoming.try_for_each(|msg| {
        let vec = msg.into_data();
        let cmd: ClientCommand = serde_json::from_slice(&vec[..]).unwrap();
        debug!("Got message from {}: {:?}", player_id, cmd);
        handle_client_command(cmd, &player_id, state.clone());
        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(handle_incoming, receive_from_others);
    future::select(handle_incoming, receive_from_others).await;

    debug!("disconnected {}", player_id);
    on_leave(&room_id, &player_id, state);
}

fn on_leave(room_id: &RoomId, player_id: &PlayerId, state: Ams) {
    let mut s = state.lock();
    s.clients.remove(player_id);
    s.players_with_updates.remove(player_id);
    s.players.remove(player_id);
    s.rooms.get_mut(room_id).map(|room| room.participants.remove(player_id));
}

fn on_join(tx: UnboundedSender<Message>, room_id: &RoomId, state: Ams) -> PlayerId {
    let mut s = state.lock();
    let player_id = s.next_player_id;
    s.next_player_id += 1;
    let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
    room.participants.insert(player_id);
    s.players.insert(player_id.clone(), create_player(player_id, room_id.clone()));

    let msg = ServerMessage::Login { id: player_id, _type: "login".to_owned() };
    tx.unbounded_send(serde_json::to_string(&msg).unwrap().into()).unwrap();
    let msg = ServerMessage::FullUpdate {
        room_id: room_id.clone(),
        players: s.players.iter().map(|(id, player)| (*id, player.pos.clone())).collect(),
        _type: "full_update".to_owned()
    };
    tx.unbounded_send(serde_json::to_string(&msg).unwrap().into()).unwrap();
    s.clients.insert(player_id.clone(), tx);

    player_id
}

fn handle_client_command(cmd: ClientCommand, player_id: &PlayerId, state: Ams) {
    let mut s = state.lock();
    match cmd {
        ClientCommand::Move { pos } => {
            s.players.get_mut(player_id).map(|player| {
                debug!("Changing pos for {} to {:?}", player_id, pos);
                player.pos = pos;
            });
            s.players_with_updates.insert(player_id.clone());
        }
        ClientCommand::Ping { id } => {
            debug!("Pong {} for {}", id, player_id);
            let reply = ServerMessage::Pong{ id, _type: "pong".to_owned() };
            let msg = serde_json::to_string(&reply).unwrap();
            s.clients.get(player_id).map(|tx| tx.unbounded_send(msg.into()));
        }
    }
}

// let broadcast_recipients =
// s.clients.iter().filter(|(id, _)| id != &&player_id).map(|(_, ws_sink)| ws_sink);
//
// for recp in broadcast_recipients {
// // recp.unbounded_send(msg.clone().into()).unwrap();
// recp.unbounded_send("pong".clone().into()).unwrap();
// }
//
