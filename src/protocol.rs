use serde::{Deserialize, Serialize};
use crate::event::{FileChunk, FileEvent};

#[derive(Serialize, Deserialize, Debug)]
pub enum HandshakeMsg {
    Challenge([u8; 32]),
    Response {
        public_key: [u8; 32],
        signature: Vec<u8>,
    },
    Ack,
    Reject,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum TransferMsg {
    Event(FileEvent),
    Chunk(FileChunk),
}