use serde::{Deserialize, Serialize};

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