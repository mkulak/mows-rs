use std::{env, sync::Arc};
use std::collections::{HashMap, HashSet};
use std::result::Result;
use std::time::Duration;

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
use crate::domain::*;

pub struct PathCapturingCallback {
    pub path: String,
}

impl Callback for &mut PathCapturingCallback {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
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


