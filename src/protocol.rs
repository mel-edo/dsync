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
    EphemeralKey([u8; 32]),
    EphemeralKeyAck([u8; 32]),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum TransferMsg {
    Event(FileEvent),
    Chunk(FileChunk),
    IndexRequest,
    IndexResponse(Vec<FileInfo>),
    RequestFile(String),
    Goodbye,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub modified: u64,
}