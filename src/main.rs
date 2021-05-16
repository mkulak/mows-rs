mod domain;
mod utils;

use std::{env, sync::Arc};
use std::collections::{HashMap, HashSet};
use std::result::Result as StdResult;
use std::time::Duration;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};
use log::*;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::http;
use tungstenite::protocol::Message;
use domain::*;
use utils::*;



#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7000".to_string());
    let state = Arc::new(Mutex::new(create_spaces_state()));
    tokio::spawn(schedule_tick(state.clone()));
    serve(&addr, state).await
}

async fn schedule_tick(state: Ams) {
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    loop {
        interval.tick().await;
        tick(state.clone());
    }
}

async fn serve(addr: &String, state: Ams) {
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
        if msg.is_text() {
            let vec = msg.into_data();
            let cmd: ClientCommand = serde_json::from_slice(&vec[..]).unwrap();
            debug!("Got message from {}: {:?}", player_id, cmd);
            handle_client_command(cmd, &player_id, &room_id, state.clone());
        }
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
    s.players.remove(player_id);
    s.rooms.get_mut(room_id).map(|room| {
        room.participants.remove(player_id);
        room.players_with_updates.remove(player_id);
    });
}

fn on_join(tx: UnboundedSender<Message>, room_id: &RoomId, state: Ams) -> PlayerId {
    let mut s = state.lock();
    let player_id = s.next_player_id;
    s.next_player_id += 1;
    let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
    room.participants.insert(player_id);
    s.players.insert(player_id.clone(), create_player(player_id, room_id.clone()));

    let msg = ServerMessage::Login { id: player_id };
    tx.unbounded_send(serde_json::to_string(&msg).unwrap().into()).unwrap();
    let msg = ServerMessage::FullUpdate {
        room_id: room_id.clone(),
        players: s.players.iter().map(|(id, player)| (*id, player.pos.clone())).collect(),
    };
    tx.unbounded_send(serde_json::to_string(&msg).unwrap().into()).unwrap();
    s.clients.insert(player_id.clone(), tx);

    player_id
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
            let msg = serde_json::to_string(&reply).unwrap();
            s.clients.get(player_id).map(|tx| tx.unbounded_send(msg.into()));
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
            for p_id in &(room.players_with_updates) {
                s.players.get(p_id).map(|player| {
                    ids.push(*p_id);
                    xs.push(player.pos.x);
                    ys.push(player.pos.y);
                });
            }
            let cmd = ServerMessage::Update { ids: &ids[..], xs: &xs[..], ys: &ys[..] };
            debug!("Send room update  {:?}", cmd);
            let msg = serde_json::to_vec(&cmd).unwrap();
            for p_id in &(room.participants) {
                s.clients.get(p_id).map(|tx| tx.unbounded_send(msg.clone().into()));
            }
        }
    }
    for room in s.rooms.values_mut() {
        room.players_with_updates.clear()
    }
}
