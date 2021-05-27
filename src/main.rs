use std::{sync::Arc};
use std::time::Duration;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};
use log::*;
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_hdr_async, accept_hdr_async_with_config};
use tungstenite::protocol::{Message, WebSocketConfig};

use domain::*;
use utils::*;
use std::collections::HashSet;
use std::net::SocketAddr;
use tungstenite::error::Error;

mod domain;
mod utils;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();
    let port = std::env::var("WONDER_PORT").unwrap_or("7000".to_string());
    let tick_interval = std::env::var("TICK_INTERVAL")
        .map(|v| v.parse::<u64>().unwrap())
        .unwrap_or(200);
    let addr = format!("0.0.0.0:{}", port);
    let state = Arc::new(Mutex::new(create_spaces_state()));
    tokio::spawn(schedule_tick(state.clone(), tick_interval));
    serve(&addr, state).await
}

async fn schedule_tick(state: Ams, tick_interval: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(tick_interval));
    loop {
        interval.tick().await;
        tick(state.clone());
    }
}

async fn serve(addr: &String, state: Ams) {
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    info!("Listening on: {}", addr);
    while let Ok((stream, peer)) = listener.accept().await {
        tokio::spawn(accept_connection(state.clone(), stream, peer));
    }
}

async fn accept_connection(state: Ams, stream: TcpStream, peer: SocketAddr) {
    debug!("Incoming TCP connection from: {}", peer);
    if let Err(e) = handle_connection(state, stream).await {
        error!("Error processing connection: {:?}", e)
    }
}

async fn handle_connection(state: Ams, stream: TcpStream) -> Result<(), MowsError> {
    let mut callback = PathCapturingCallback { path: String::new() };
    let config = WebSocketConfig { max_frame_size: Some(65536), ..WebSocketConfig::default() };
    let ws_stream = accept_hdr_async_with_config(stream, &mut callback, Some(config))
        .await.map_err(|e| e.into())?;
    let room_id = callback.path[ROOMS_PREFIX.len()..].to_owned();

    let (tx, rx) = unbounded();
    let (outgoing, incoming) = ws_stream.split();

    let player_id = on_join(tx, &room_id, state.clone());
    debug!("Connected {} room {}", player_id, room_id.clone());

    let handle_incoming = incoming.try_for_each(|msg| {
        if msg.is_text() {
            let vec = msg.into_data();
            let result = serde_json::from_slice::<ClientCommand>(&vec[..]);
            match result {
                Ok(cmd) => {
                    debug!("Got message from {}: {:?}", player_id, cmd);
                    handle_client_command(cmd, &player_id, &room_id, state.clone());
                    future::ok(())
                }
                Err(_) => future::err(Error::Utf8) // TODO: figure out how to provide actual error
            }
        } else {
            future::ok(())
        }
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(handle_incoming, receive_from_others);
    future::select(handle_incoming, receive_from_others).await;

    debug!("disconnected {}", player_id);
    on_leave(&room_id, &player_id, state);
    Ok(())
}

fn on_leave(room_id: &RoomId, player_id: &PlayerId, state: Ams) {
    let mut s = state.lock();
    s.clients.remove(player_id);
    s.players.remove(player_id);
    let participants = s.rooms.get_mut(room_id).map(|room| {
        room.participants.remove(player_id);
        room.players_with_updates.remove(player_id);
        room.participants.clone()
    }).unwrap_or(HashSet::new());
    send_to_all(&*s, participants.iter(), &ServerMessage::RemovePlayer { id: *player_id })
}

fn on_join(tx: UnboundedSender<Message>, room_id: &RoomId, state: Ams) -> PlayerId {
    let mut s = state.lock();
    let player_id = s.next_player_id;
    s.next_player_id += 1;
    let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
    let participants = room.participants.clone();
    room.participants.insert(player_id);
    
    let player = create_player(player_id, room_id.clone());
    let pos = player.pos;
    s.players.insert(player_id.clone(), player);
    s.clients.insert(player_id.clone(), tx);

    let msg = ServerMessage::Login { id: player_id };
    send(&*s, &player_id, &msg);

    let players = s.players.iter().map(|(id, player)| (*id, player.pos.clone())).collect();
    let msg = ServerMessage::FullUpdate { room_id: room_id.clone(), players };
    send(&s, &player_id, &msg);
    send_to_all(&*s, participants.iter(), &ServerMessage::AddPlayer { id: player_id, pos });
    player_id
}

fn send(s: &SpacesState, player_id: &PlayerId, msg: &ServerMessage) {
    s.clients.get(player_id).map(|tx|
        tx.unbounded_send(serde_json::to_string(&msg).unwrap().into()).unwrap()
    );
}

fn send_to_all<'a>(s: &SpacesState, ids: impl Iterator<Item=&'a PlayerId>, msg: &ServerMessage) {
    ids.for_each(|id| send(s, &id, msg));
}

fn handle_client_command(cmd: ClientCommand, player_id: &PlayerId, room_id: &RoomId, state: Ams) {
    let mut s = state.lock();
    match cmd {
        ClientCommand::Move { pos } => {
            s.players.get_mut(player_id).map(|player|
                player.pos = pos
            );
            s.rooms.get_mut(room_id).map(|room|
                room.players_with_updates.insert(player_id.clone())
            );
        }
        ClientCommand::Ping { id } => {
            let reply = ServerMessage::Pong { id };
            send(&*s, &player_id, &reply);
        }
    }
}

fn tick(state: Ams) {
    let mut s = state.lock();
    for room in s.rooms.values() {
        if !room.players_with_updates.is_empty() {
            let mut ids = Vec::new();
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            for player_id in &(room.players_with_updates) {
                s.players.get(player_id).map(|player| {
                    ids.push(*player_id);
                    xs.push(player.pos.x);
                    ys.push(player.pos.y);
                });
            }
            let cmd = ServerMessage::Update { ids: &ids[..], xs: &xs[..], ys: &ys[..] };
            debug!("Send room update  {:?} to {} participants", cmd, room.participants.len());
            send_to_all(&*s, room.participants.iter(), &cmd);
        }
    }
    for room in s.rooms.values_mut() {
        room.players_with_updates.clear()
    }
}

#[derive(Debug)]
struct MowsError {
    reason: String
}

impl Into<MowsError> for tungstenite::error::Error {
    fn into(self) -> MowsError {
        MowsError { reason: format!("{:?}", self)}
    }
}

impl Into<MowsError> for serde_json::error::Error {
    fn into(self) -> MowsError {
        MowsError { reason: format!("{:?}", self)}
    }
}

