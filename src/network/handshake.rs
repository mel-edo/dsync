use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use anyhow::bail;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};

use crate::core::protocol::HandshakeMsg;

pub async fn perform_server_handshake(stream: &mut TcpStream) -> anyhow::Result<(String, ChaCha20Poly1305)> {
    // generate a random challenge
    let mut challenge = [0u8; 32];
    OsRng.fill_bytes(&mut challenge);

    // send challenge to client
    let msg = HandshakeMsg::Challenge(challenge);
    let serialized = bincode::serialize(&msg)?;

    stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
    stream.write_all(&serialized).await?;

    // wait for response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    let response: HandshakeMsg = bincode::deserialize(&buf)?;

    // verify the signature
    if let HandshakeMsg::Response { public_key, signature } = response {
        let verifying_key = VerifyingKey::from_bytes(&public_key.clone().try_into().unwrap())?;
        let signature_obj = Signature::from_slice(&signature)?;

        // verify that the signature matches the challenge we sent
        if verifying_key.verify(&challenge, &signature_obj).is_ok() {
            // send ack
            let ack = bincode::serialize(&HandshakeMsg::Ack)?;
            stream.write_all(&(ack.len() as u32).to_be_bytes()).await?;
            stream.write_all(&ack).await?;

            let server_secret = EphemeralSecret::random_from_rng(OsRng);
            let server_public = X25519PublicKey::from(&server_secret);

            // send our X25519 public key
            let key_msg = HandshakeMsg::EphemeralKey(server_public.to_bytes());
            let serialized = bincode::serialize(&key_msg)?;
            stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
            stream.write_all(&serialized).await?;

            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let mut buf = vec![0u8; u32::from_be_bytes(len_buf) as usize];
            stream.read_exact(&mut buf).await?;

            let peer_key_msg: HandshakeMsg = bincode::deserialize(&buf)?;
            let cipher = if let HandshakeMsg::EphemeralKeyAck(peer_key_bytes) = peer_key_msg {
                let peer_public = X25519PublicKey::from(peer_key_bytes);
                let shared_secret = server_secret.diffie_hellman(&peer_public);
                ChaCha20Poly1305::new_from_slice(shared_secret.as_bytes())
                    .map_err(|e| anyhow::anyhow!("Failed to create cipher: {:?}", e))?
            } else {
                bail!("Expected EphemeralKeyAck");
            };

            return Ok((hex::encode(public_key), cipher));  // return their ID
        }
    }

    // fail
    let rej = bincode::serialize(&HandshakeMsg::Reject)?;
    stream.write_all(&(rej.len() as u32).to_be_bytes()).await?;
    stream.write_all(&rej).await?;
    bail!("Invalid Handshake Signature")
}