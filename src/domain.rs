use std::{sync::Arc};
use std::collections::{HashMap, HashSet};

use futures_channel::mpsc::{UnboundedSender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tungstenite::protocol::Message;

pub fn create_spaces_state() -> SpacesState {
    SpacesState {
        rooms: HashMap::new(),
        players: HashMap::new(),
        clients: HashMap::new(),
        next_player_id: 0,
    }
}

pub fn create_room(_: &RoomId) -> Room {
    Room {
        participants: HashSet::new(),
        players_with_updates: HashSet::new(),
    }
}

pub fn create_player(_: PlayerId, _: RoomId) -> Player {
    Player { pos: XY { x: 0.0, y: 0.0 } }
}

pub struct Room {
    pub participants: HashSet<PlayerId>,
    pub players_with_updates: HashSet<PlayerId>,
}

pub struct Player {
    pub pos: XY,
}

pub struct SpacesState {
    pub rooms: HashMap<RoomId, Room>,
    pub players: HashMap<PlayerId, Player>,
    pub clients: HashMap<PlayerId, Tx>,
    pub next_player_id: u32,
}

pub type RoomId = String;

pub type PlayerId = u32;

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage<'a> {
    #[serde(rename = "login")]
    Login { id: PlayerId },
    #[serde(rename = "pong")]
    Pong { id: i64 },
    #[serde(rename = "full_update")]
    FullUpdate { #[serde(rename = "roomId")] room_id: String, players: HashMap<PlayerId, XY> },
    #[serde(rename = "update")]
    Update { ids: &'a [PlayerId], xs: &'a [f64], ys: &'a [f64] },
    #[serde(rename = "add")]
    AddPlayer { id: PlayerId, pos: XY },
    #[serde(rename = "remove")]
    RemovePlayer { id: PlayerId },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientCommand {
    #[serde(rename = "move")]
    Move { pos: XY },
    #[serde(rename = "ping")]
    Ping { id: i64 },
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct XY {
    pub x: f64,
    pub y: f64,
}

pub const ROOMS_PREFIX: &'static str = "/rooms/";

pub type Tx = UnboundedSender<Message>;

pub type Ams = Arc<Mutex<SpacesState>>;
