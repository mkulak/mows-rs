// use std::borrow::BorrowMut;
// use std::cell::RefCell;
// use std::collections::{HashMap, HashSet};
// use std::net::SocketAddr;
// use std::rc::Rc;
// use std::result::Result as StdResult;
// use std::sync::Arc;
// use std::thread;
//
// use futures_util::{SinkExt, StreamExt};
// use log::*;
// use parking_lot::Mutex;
// use serde::{Deserialize, Serialize};
// use tokio::net::{TcpListener, TcpStream};
// use tokio_tungstenite::{accept_hdr_async, tungstenite::Error, WebSocketStream};
// use tokio_tungstenite::tungstenite::handshake::client::Request;
// use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
// use tungstenite::{http, Result};
//
// use rand::{Rng, thread_rng};
// use rand::distributions::Alphanumeric;
// use tokio::sync::mpsc;
//
// #[tokio::main(flavor = "current_thread")]
// async fn main() {
//     env_logger::init();
//
//     let addr = "127.0.0.1:9002";
//     let listener = TcpListener::bind(&addr).await.expect("Can't listen");
//     info!("Listening on: {}", addr);
//
//     let spaces_server = Arc::new(Mutex::new(create_spaces_state()));
//     while let Ok((stream, _)) = listener.accept().await {
//         let peer = stream.peer_addr().expect("connected streams should have a peer address");
//         info!("Peer address: {}", peer);
//         let rc = spaces_server.clone();
//         tokio::spawn(accept_connection(rc, peer, stream));
//     }
// }
//
// async fn accept_connection(mut space: Arc<Mutex<SpacesState>>, peer: SocketAddr, stream: TcpStream) {
//     if let Err(e) = handle_connection(space, peer, stream).await {
//         match e {
//             Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
//             err => error!("Error processing connection: {}", err),
//         }
//     }
// }
//
// async fn handle_connection(mut space: Arc<Mutex<SpacesState>>, peer: SocketAddr, stream: TcpStream) -> Result<()> {
//     let mut cb = PathCapturingCallback {
//         path: String::new()
//     };
//
//     let mut ws_stream = accept_hdr_async(stream, &mut cb).await?;
//     let room_id = cb.path[ROOMS_PREFIX.len()..].to_owned();
//     info!("New WebSocket connection: {} {}", peer, room_id);
//     let player_id = {
//         let mut s = space.lock();
//         let player_id = s.next_player_id.to_string();
//         s.next_player_id += 1;
//         let room = s.rooms.entry(room_id.clone()).or_insert_with(|| create_room(&room_id));
//         room.participants.insert(player_id.clone());
//         s.player2room.insert(player_id.clone(), room_id);
//         player_id
//     };
//     // let (client_sender, mut client_rcv) = mpsc::unbounded_channel();
//     // client_rcv.forward(ws_stream).map(|result| {
//     //     if let Err(e) = result {
//     //         eprintln!("error sending websocket msg: {}", e);
//     //     }
//     // }).await;
//     // client_sender.
//
//     // let msg = LoginServerMessage { id, _type: "login".to_owned() };
//     // let data = serde_json::to_string(&msg).unwrap();
//     // ws.send(data.into()).await?;
//
//     // while let Some(msg) = ws.next().await {
//     //     let msg = msg?;
//     //     if msg.is_text() {
//     //         // let serde_json::from_slice(msg.into());
//     //         info!("got: {}", msg.clone().into_text().unwrap());
//     //         ws.send(msg).await?;
//     //     }
//     // }
//
//     Ok(())
// }
//
// fn create_spaces_state() -> SpacesState {
//     SpacesState { rooms: HashMap::new(), player2room: HashMap::new(), next_player_id: 0 }
// }
//
// fn create_room(room_id: &String) -> Room {
//     Room { id: room_id.clone(), participants: HashSet::new() }
// }
//
// struct Room {
//     id: String,
//     participants: HashSet<PlayerId>
// }
//
// struct SpacesState {
//     rooms: HashMap<RoomId, Room>,
//     player2room: HashMap<PlayerId, RoomId>,
//     next_player_id: u32,
// }
//
// struct Clients {
//     clients: HashMap<PlayerId, >
// }
//
// type RoomId = String;
//
// type PlayerId = String;
//
// #[derive(Debug, Serialize, Deserialize)]
// pub struct LoginServerMessage {
//     pub id: String,
//     #[serde(rename = "type")]
//     pub _type: String,
// }
//
// const ROOMS_PREFIX: &'static str = "/rooms/";
//
// struct PathCapturingCallback {
//     path: String,
// }
//
// impl Callback for &mut PathCapturingCallback {
//     fn on_request(self, request: &Request, response: Response) -> StdResult<Response, ErrorResponse> {
//         let path = request.uri().path();
//         if !path.starts_with(ROOMS_PREFIX) {
//             return Err(http::response::Response::builder()
//                 .status(http::StatusCode::NOT_FOUND)
//                 .body(None)
//                 .unwrap());
//         }
//         (&mut self.path).push_str(path);
//         Ok(response)
//     }
// }
//
// fn random_id() -> String {
//     return thread_rng()
//         .sample_iter(&Alphanumeric)
//         .take(22)
//         .map(char::from)
//         .collect();
// }



use std::{
    collections::HashMap,
    env,
    io::Error as IoError,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};

use tokio::net::{TcpListener, TcpStream};
use tungstenite::protocol::Message;

type Tx = UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<SocketAddr, Tx>>>;

async fn handle_connection(peer_map: PeerMap, raw_stream: TcpStream, addr: SocketAddr) {
    println!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .expect("Error during the websocket handshake occurred");
    println!("WebSocket connection established: {}", addr);

    // Insert the write part of this peer to the peer map.
    let (tx, rx) = unbounded();
    peer_map.lock().unwrap().insert(addr, tx);

    let (outgoing, incoming) = ws_stream.split();

    let broadcast_incoming = incoming.try_for_each(|msg| {
        println!("Received a message from {}: {}", addr, msg.to_text().unwrap());
        let peers = peer_map.lock().unwrap();

        // We want to broadcast the message to everyone except ourselves.
        let broadcast_recipients =
            peers.iter().filter(|(peer_addr, _)| peer_addr != &&addr).map(|(_, ws_sink)| ws_sink);

        for recp in broadcast_recipients {
            recp.unbounded_send(msg.clone()).unwrap();
        }

        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(broadcast_incoming, receive_from_others);
    future::select(broadcast_incoming, receive_from_others).await;

    println!("{} disconnected", &addr);
    peer_map.lock().unwrap().remove(&addr);
}

#[tokio::main]
async fn main() -> Result<(), IoError> {
    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let state = PeerMap::new(Mutex::new(HashMap::new()));

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    println!("Listening on: {}", addr);

    // Let's spawn the handling of each connection in a separate task.
    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(state.clone(), stream, addr));
    }

    Ok(())
}
