use std::{collections::HashMap, fs, io::Read, path::PathBuf, sync::Arc, time::Instant};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::{mpsc, Mutex}, time::{timeout, Duration}};
use anyhow::{Context, bail};
use crate::event::{EventOp, FileEvent, FileChunk};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use crate::{protocol::{HandshakeMsg, TransferMsg}, util, sync};

const CHUNK_SIZE: usize = 64 * 1024;

pub async fn start_server(port: u16, root: PathBuf, sender: mpsc::Sender<FileEvent>, instance_id: String, verbose: bool) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    if verbose {
        println!("Server listening on port {}", port);
    }

    loop {
        let (mut stream, addr) = listener.accept().await?;
        if verbose { println!("New connection from {:?}", addr); }

        // spawn a new task with tokio::spawn to handle multiple peers
        let tx = sender.clone();
        let root_clone = root.clone();
        let instance_id_clone = instance_id.clone();

        tokio::spawn(async move {
            if let Err(e) = perform_server_handshake(&mut stream).await {
                eprintln!("Handshake failed: {:?}", e);
                return;
            }

            // split stream into read/write halves for bidirectional communication
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut pending_transfers: HashMap<PathBuf, Instant> = HashMap::new();

            loop {
                let mut len_buf = [0u8; 4];
                if let Err(_) = buf_reader.read_exact(&mut len_buf).await { break; }
                let msg_len = u32::from_be_bytes(len_buf) as usize;

                let mut buffer = vec![0u8; msg_len];
                if let Err(_) = buf_reader.read_exact(&mut buffer).await { break; }

                match bincode::deserialize::<TransferMsg>(&buffer) {
                    
                    // peer wants our file list
                    Ok(msg) => match msg {
                        TransferMsg::IndexRequest => {
                            if verbose { println!("Received Index Request"); }
                            let index = sync::generate_local_index(&root_clone);
                            send_msg(&mut writer, &TransferMsg::IndexResponse(index)).await.ok();
                        },

                        // peer sent us their list
                        TransferMsg::IndexResponse(remote_index) => {
                            if verbose { println!("Received Remote Index ({} files)", remote_index.len()); }
                            let local_index = sync::generate_local_index(&root_clone);
                            let missing = sync::calculate_diff(&local_index, &remote_index);

                            for path in missing {
                                if verbose { println!("Requesting: {}", path); }
                                send_msg(&mut writer, &TransferMsg::RequestFile(path)).await.ok();
                            }
                        },

                        // peer requested a specific file, send it
                        TransferMsg::RequestFile(path) => {
                            if verbose { println!("Sending requested file: {}", path); }
                            let file_path = PathBuf::from(&path);

                            // send metadata first
                            let event = FileEvent::new(EventOp::Create, file_path.clone(), None);
                            if let Err(e) = send_msg(&mut writer, &TransferMsg::Event(event.clone())).await {
                                eprintln!("Failed to send event: {:?}", e);
                            }

                            // stream the file content using helper
                            if let Err(e) = stream_file_to_writer(&mut writer, &root_clone, &file_path).await {
                                eprintln!("Failed to stream file {}: {:?}", path, e);
                            }
                        },

                        TransferMsg::Event(event) => {
                            if let Some(origin) = event.origin_id() {
                                if origin == &instance_id_clone { continue; }
                            }

                            pending_transfers.insert(event.file_path().clone(), Instant::now());

                            match event.operation() {
                                EventOp::Delete => {
                                    let full_path = root_clone.join(event.file_path());
                                    if full_path.exists() { let _ = fs::remove_file(&full_path); }
                                    println!("Deleted: {:?}", event.file_path());
                                    let _ = tx.send(event).await;
                                }
                                EventOp::Create | EventOp::Modify => {
                                    let full_path = root_clone.join(event.file_path());
                                    util::ensure_parent(&full_path).ok();
                                }
                            }
                        },
                        TransferMsg::Chunk(chunk) => {
                            match util::write_chunk_atomic(&root_clone, &chunk.rel_path, &chunk.data, chunk.is_last) {
                                Ok(is_complete) => {
                                    if is_complete {
                                        let duration = if let Some(start) = pending_transfers.remove(&chunk.rel_path) {
                                            format!("{:.2?}", start.elapsed())
                                        } else {
                                            "??s".to_string()
                                        };
                                        println!("Received: {:?} ({})", chunk.rel_path, duration);

                                        let event = FileEvent::new(EventOp::Create, chunk.rel_path, None)
                                            .with_origin("network".to_string());
                                        let _ = tx.send(event).await;
                                    }
                                }
                                Err(e) => eprintln!("Write error: {:?}", e),
                            }
                        }
                    },
                    Err(_) => break,
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

    pub async fn request_index(&self, addr: &str, root:PathBuf, tx: mpsc::Sender<FileEvent>) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;

        let msg = TransferMsg::IndexRequest;
        let serialized = bincode::serialize(&msg)?;
        stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
        stream.write_all(&serialized).await?;

        let mut reader = BufReader::new(stream);
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf).await?;
        let msg_len = u32::from_be_bytes(len_buf) as usize;

        let mut buffer = vec![0u8; msg_len];
        reader.read_exact(&mut buffer).await?;

        let remote_index = match bincode::deserialize::<TransferMsg>(&buffer)? {
            TransferMsg::IndexResponse(idx) => idx,
            _ => bail!("Expected IndexResponse"),
        };

        let local_index = sync::generate_local_index(&root);
        let missing_files = sync::calculate_diff(&local_index, &remote_index);

        if missing_files.is_empty() {
            self.store_stream(addr, reader.into_inner()).await;
            return Ok(());
        }
        println!("Syncing {} missing files from {}", missing_files.len(), addr);

        let mut stream = reader.into_inner();

        for path in missing_files {
            let req = TransferMsg::RequestFile(path.clone());
            let ser_req = bincode::serialize(&req)?;
            stream.write_all(&(ser_req.len() as u32).to_be_bytes()).await?;
            stream.write_all(&ser_req).await?;

            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut event_buf = vec![0u8; len];
            stream.read_exact(&mut event_buf).await?;

            let _event_msg = bincode::deserialize::<TransferMsg>(&event_buf)?;

            loop {
                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf).await?;
                let chunk_len = u32::from_be_bytes(len_buf) as usize;

                let mut chunk_buf = vec![0u8; chunk_len];
                stream.read_exact(&mut chunk_buf).await?;

                if let TransferMsg::Chunk(chunk) = bincode::deserialize(&chunk_buf)? {
                    let is_last = chunk.is_last;
                    let rel_path = chunk.rel_path.clone();

                    util::write_chunk_atomic(&root, &rel_path, &chunk.data, is_last)?;

                    if is_last {
                        println!("Synced: {:?}", rel_path);
                        let event = FileEvent::new(EventOp::Create, rel_path, None)
                            .with_origin("network".to_string());
                        let _ = tx.send(event).await;
                        break;
                    }
                } else {
                    bail!("Expected Chunk");
                }
            }
        }
        self.store_stream(addr, stream).await;
        Ok(())
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent, root_path: &PathBuf) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;

        let event_msg = TransferMsg::Event(event.clone());
        let serialized = bincode::serialize(&event_msg)?;
        stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
        stream.write_all(&serialized).await?;

        if matches!(event.operation(), EventOp::Create | EventOp::Modify) {
            let (reader, mut writer) = stream.into_split();

            stream_file_to_writer(&mut writer, root_path, event.file_path()).await?;

            let reunited_stream = reader.reunite(writer).unwrap();
            stream = reunited_stream;

            println!("Sent: {:?}", event.file_path());
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

// helpers

// helper to send any TransferMsg
async fn send_msg(writer: &mut tokio::net::tcp::OwnedWriteHalf, msg: &TransferMsg) -> anyhow::Result<()> {
    let serialized = bincode::serialize(msg)?;
    writer.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
    writer.write_all(&serialized).await?;
    Ok(())
}

// helper to chunk and stream a file
async fn stream_file_to_writer(writer: &mut tokio::net::tcp::OwnedWriteHalf, root: &PathBuf, rel_path: &PathBuf) -> anyhow::Result<()> {
    let full_path = root.join(rel_path);
    if let Ok(mut file) = fs::File::open(&full_path) {
        let mut buffer = [0u8; CHUNK_SIZE];
        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 { break; }

            let chunk = FileChunk {
                rel_path: rel_path.clone(),
                offset: 0,
                data: buffer[..bytes_read].to_vec(),
                is_last: bytes_read < CHUNK_SIZE,
            };

            let chunk_msg = TransferMsg::Chunk(chunk);
            send_msg(writer, &chunk_msg).await?;
        }
    }
    Ok(())
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