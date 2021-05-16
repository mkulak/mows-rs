use std::result::Result;

use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Response};
use tungstenite::http;

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

#[allow(dead_code)]
fn random_id() -> String {
    thread_rng().sample_iter(&Alphanumeric).take(22).map(char::from).collect()
}


