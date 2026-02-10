use std::{collections::HashMap, fs::{self, OpenOptions, create_dir_all, remove_file}, io::{Read, Write}, path::PathBuf, sync::Arc};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::{mpsc, Mutex}, time::{timeout, Duration}};
use anyhow::{Context, bail};
use crate::event::{EventOp, FileEvent, FileChunk};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use crate::protocol::{HandshakeMsg, TransferMsg};

const CHUNK_SIZE: usize = 64 * 1024;

pub async fn start_server(port: u16, root: PathBuf, sender: mpsc::Sender<FileEvent>, instance_id: String) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("Server listening on port {}", port);

    loop {
        let (mut stream, addr) = listener.accept().await?;
        println!("New connection from {:?}", addr);
        // spawn a new task with tokio::spawn to handle multiple peers

        let tx = sender.clone();
        let root_clone = root.clone();
        let instance_id_clone = instance_id.clone();

        tokio::spawn(async move {
            // perform zero config handshake before receiving data
            match perform_server_handshake(&mut stream).await {
                Ok(peer_id) => println!("Trust established with ID: {}", peer_id),
                Err(e) => {
                    eprintln!("Handshake failed: {:?}", e);
                    return;
                }
            }

            let mut reader = BufReader::new(stream);

            loop {
                let mut len_buf = [0u8; 4];
                if let Err(_) = reader.read_exact(&mut len_buf).await { break; }
                
                let msg_len = u32::from_be_bytes(len_buf) as usize;
                let mut buffer = vec![0u8; msg_len];

                if let Err(e) = reader.read_exact(&mut buffer).await {
                    eprintln!("Failed to read message: {:?}", e);
                    break;
                }

                match bincode::deserialize::<TransferMsg>(&buffer) {
                    Ok(msg) => match msg {
                        TransferMsg::Event(event) => {
                            if let Some(origin) = event.origin_id() {
                                if origin == &instance_id_clone { continue; }
                            }

                            match event.operation() {
                                EventOp::Delete => {
                                    let full_path = root_clone.join(event.file_path());
                                    if full_path.exists() {
                                        let _ = remove_file(&full_path);
                                        println!("Deleted: {:?}", full_path);
                                    }
                                    let _ = tx.send(event).await;
                                }
                                EventOp::Create | EventOp::Modify => {
                                    let full_path = root_clone.join(event.file_path());

                                    if let Some(parent) = full_path.parent() {
                                        let _ = create_dir_all(parent);
                                    }
                                }
                            }
                        },
                        TransferMsg::Chunk(chunk) => {
                            let full_path = root_clone.join(&chunk.rel_path);

                            let part_path = full_path.with_extension("part");

                            let mut options = OpenOptions::new();
                            options.write(true).append(true).create(true);

                            match options.open(&part_path) {
                                Ok(mut file) => {
                                    if let Err(e) = file.write_all(&chunk.data) {
                                        eprintln!("Failed to write chunk: {:?}", e);
                                    }
                                }
                                Err(e) => eprintln!("Failed to append chunk to {:?}: {:?}", part_path, e),
                            }

                            // on last chunk -> rename .part to real file
                            if chunk.is_last {
                                if let Err(e) = fs::rename(&part_path, &full_path) {
                                    eprintln!("Failed to rename partial file: {:?}", e);
                                } else {
                                    println!("Received: {:?}", full_path);

                                    let event = FileEvent::new(
                                        EventOp::Create,
                                        chunk.rel_path,
                                        None
                                    ).with_origin("network".to_string());

                                    let _ = tx.send(event).await;
                                }
                            }
                        }
                    },
                    Err(e) => eprintln!("Deserialize error: {:?}", e),
                }
            }
        });
    }
}

pub struct ConnectionPool {
    streams: Mutex<HashMap<String, TcpStream>>,
    my_keypair: Arc<SigningKey>,
}

impl ConnectionPool {
    pub fn new(keypair: SigningKey) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            my_keypair: Arc::new(keypair),
        }
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent, root_path: &PathBuf) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;

        let event_msg = TransferMsg::Event(event.clone());
        let serialized = bincode::serialize(&event_msg)?;

        stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
        stream.write_all(&serialized).await?;

        if matches!(event.operation(), EventOp::Create | EventOp::Modify) {
            let full_path = root_path.join(event.file_path());

            if let Ok(mut file) = fs::File::open(&full_path) {
                let mut buffer = [0u8; CHUNK_SIZE];
                let mut offset = 0;

                loop {
                    let bytes_read = file.read(&mut buffer)?;
                    if bytes_read == 0 { break; }

                    let chunk = FileChunk {
                        rel_path: event.file_path().clone(),
                        offset,
                        data: buffer[..bytes_read].to_vec(),
                        is_last: bytes_read < CHUNK_SIZE,
                    };

                    let chunk_msg = TransferMsg::Chunk(chunk);
                    let serialized_chunk = bincode::serialize(&chunk_msg)?;

                    stream.write_all(&(serialized_chunk.len() as u32).to_be_bytes()).await?;
                    stream.write_all(&serialized_chunk).await?;

                    offset += bytes_read as u64;
                }
                println!("Sent: {:?}", event.file_path());
            }
        }
        self.store_stream(addr, stream).await;
        Ok(())
    }

    async fn acquire_stream(&self, addr: &str) -> anyhow::Result<TcpStream> {
        if let Some(stream) = {
            let mut guard = self.streams.lock().await;
            guard.remove(addr)
        } {
            return Ok(stream);
        }

        // add timeout to prevent freezing on one event
        let connect_future = TcpStream::connect(addr);
        let mut stream = match timeout(Duration::from_secs(3), connect_future).await {
            Ok(res) => res.with_context(|| format!("Failed to connect to {}", addr))?,
            Err(_) => bail!("Connection timed out to {}", addr),
        };

        // read challenge from server
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let msg: HandshakeMsg = bincode::deserialize(&buf)?;

        if let HandshakeMsg::Challenge(challenge) = msg {
            // sign the challenge
            let signature = self.my_keypair.sign(&challenge);

            // send response (public key + signature)
            let resp = HandshakeMsg::Response {
                public_key: self.my_keypair.verifying_key().to_bytes(),
                signature: signature.to_vec(),
            };
            let serialized = bincode::serialize(&resp)?;
            stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
            stream.write_all(&serialized).await?;

            // wait for ack
            let mut ack_len = [0u8; 4];
            stream.read_exact(&mut ack_len).await?;
            let mut ack_buf = vec![0u8; u32::from_be_bytes(ack_len) as usize];
            stream.read_exact(&mut ack_buf).await?;

            let ack_msg: HandshakeMsg = bincode::deserialize(&ack_buf)?;
            if matches!(ack_msg, HandshakeMsg::Ack) {
                return Ok(stream);
            }
        }
        bail!("Handshake failed with {}", addr)
    }

    async fn store_stream(&self, addr: &str, stream: TcpStream) {
        let mut guard = self.streams.lock().await;
        guard.insert(addr.to_string(), stream);
    }
}

pub async fn perform_server_handshake(stream: &mut TcpStream) -> anyhow::Result<String> {
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

            return Ok(hex::encode(public_key));  // return their ID
        }
    }

    // fail
    let rej = bincode::serialize(&HandshakeMsg::Reject)?;
    stream.write_all(&(rej.len() as u32).to_be_bytes()).await?;
    stream.write_all(&rej).await?;
    bail!("Invalid Handshake Signature")
}