use std::{collections::HashMap, fs, io::{Read, Write}, path::PathBuf, sync::Arc, time::Instant};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::mpsc, time::{timeout, Duration}};
use anyhow::{Context, bail};
use crate::event::{EventOp, FileEvent, FileChunk};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use crate::{protocol::{HandshakeMsg, TransferMsg}, util, sync};
use blake3;
use crate::progress::ProgressManager;
use tracing::{debug, error, info, warn};

const CHUNK_SIZE: usize = 1024 * 1024;

pub async fn start_server(port: u16, root: PathBuf, sender: mpsc::Sender<FileEvent>, instance_id: String) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("Server listening on port {}", port);

    loop {
        let (mut stream, addr) = listener.accept().await?;
        debug!("New connection from {:?}", addr);

        // spawn a new task with tokio::spawn to handle multiple peers
        let tx = sender.clone();
        let root_clone = root.clone();
        let instance_id_clone = instance_id.clone();

        tokio::spawn(async move {
            if let Err(e) = perform_server_handshake(&mut stream).await {
                warn!("Handshake failed: {:?}", e);
                return;
            }

            // split stream into read/write halves for bidirectional communication
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            
            let mut current_file_path: Option<String> = None;
            let mut current_file: Option<std::fs::File> = None;
            let mut current_hasher: Option<blake3::Hasher> = None;
            let mut pending_transfers: HashMap<PathBuf, Instant> = HashMap::new();

            let progress_manager = Some(ProgressManager::new());
            let mut active_progress = None;

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
                            debug!("Received Index Request");
                            let index = sync::generate_local_index(&root_clone);
                            send_msg(&mut writer, &TransferMsg::IndexResponse(index)).await.ok();
                        },

                        // peer sent us their list
                        TransferMsg::IndexResponse(remote_index) => {
                            debug!("Received Remote Index ({} files)", remote_index.len());
                            let local_index = sync::generate_local_index(&root_clone);
                            let missing = sync::calculate_diff(&local_index, &remote_index);

                            for path in missing {
                                debug!("Requesting: {}", path);
                                send_msg(&mut writer, &TransferMsg::RequestFile(path)).await.ok();
                            }
                        },

                        // peer requested a specific file, send it
                        TransferMsg::RequestFile(path) => {
                            debug!("Sending requested file: {}", path);
                            let file_path = PathBuf::from(&path);

                            // send metadata first
                            let event = FileEvent::new(EventOp::Create, file_path.clone(), None);
                            send_msg(&mut writer, &TransferMsg::Event(event)).await.ok();

                            // stream the file content using helper
                            if let Err(e) = stream_file_to_writer(&mut writer, &root_clone, &file_path, None).await {
                                error!("Failed to stream file {}: {:?}", path, e);
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

                                    if full_path.exists() {
                                        if full_path.is_dir() {
                                            match fs::remove_dir_all(&full_path) {
                                                Ok(_) => info!("Deleted directory: {:?}", event.file_path()),
                                                Err(e) => error!("Failed to delete directory {:?}: {}", event.file_path(), e),
                                            }
                                        } else {
                                            match fs::remove_file(&full_path) {
                                                Ok(_) => info!("Deleted: {:?}", event.file_path()),
                                                Err(_) => {}
                                            }
                                        }
                                    }
                                    let _ = tx.send(event).await;
                                }
                                EventOp::Create | EventOp::Modify => {
                                    let full_path = root_clone.join(event.file_path());
                                    util::ensure_parent(&full_path).ok();

                                    if let Some(ref pm) = progress_manager {
                                        let file_name = event.file_path()
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("unknown");
                                        active_progress = Some(pm.create_spinner(&format!("Receiving: {}", file_name)));
                                    }
                                }
                            }
                        },
                        TransferMsg::Chunk(chunk) => {
                            let rel_path_str = chunk.rel_path.to_string_lossy().to_string();
                            let full_path = root_clone.join(&chunk.rel_path);

                            if current_file_path.as_ref() != Some(&rel_path_str) {
                                util::ensure_parent(&full_path).ok();

                                match fs::OpenOptions::new().create(true).write(true).append(true).open(full_path.with_extension("part")) {
                                    Ok(f) => {
                                        current_file = Some(f);
                                        current_file_path = Some(rel_path_str.clone());
                                        current_hasher = Some(blake3::Hasher::new());
                                    }
                                    Err(e) => {
                                        error!("Failed to open part file: {:?}", e);
                                        current_file = None;
                                        current_hasher = None;
                                    }
                                }
                            }

                            if let Some(file) = current_file.as_mut() {
                                if let Err(e) = file.write_all(&chunk.data) {
                                    error!("Write error: {:?}", e);
                                    current_file = None;
                                } else {
                                    if let Some(ref pb) = active_progress {
                                        let file_name = chunk.rel_path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("unknown");
                                        pb.set_message(format!("Receiving: {}: {} bytes", file_name, chunk.offset + chunk.data.len() as u64));
                                    }
                                }
                            }
                            if let Some(hasher) = current_hasher.as_mut() {
                                hasher.update(&chunk.data);
                            }

                            if chunk.is_last {
                                let hash_str = current_hasher.take().map(|h| h.finalize().to_hex().to_string());
                                current_file = None;
                                current_file_path = None;

                                let event = FileEvent::new(EventOp::Create, chunk.rel_path.clone(), hash_str)
                                    .with_origin("network".to_string());
                                let _ = tx.send(event).await;

                                let part_path = full_path.with_extension("part");
                                if let Ok(_) = fs::rename(&part_path, &full_path) {
                                    let duration = if let Some(start) = pending_transfers.remove(&chunk.rel_path) {
                                        format!("{:.2?}", start.elapsed())
                                    } else {
                                        "??s".to_string()
                                    };

                                    if let Some(pb) = active_progress.take() {
                                        pb.finish_with_message(format!("✓ Received: {} ({})", chunk.rel_path.display(), duration));
                                    } else {
                                        info!("Received: {:?} ({})", chunk.rel_path, duration);
                                    }
                                }
                            }
                        },
                        TransferMsg::Goodbye => {
                            info!("Peer {} disconnected gracefully", addr);
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
        });
    }
}

pub struct ConnectionPool {
    // streams: Mutex<HashMap<String, TcpStream>>,
    my_keypair: Arc<SigningKey>,
    progress_manager: Option<Arc<ProgressManager>>,
}

impl ConnectionPool {
    pub fn new(keypair: SigningKey) -> Self {
        Self {
            // streams: Mutex::new(HashMap::new()),
            my_keypair: Arc::new(keypair),
            progress_manager: Some(Arc::new(ProgressManager::new()))
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
            // self.store_stream(addr, reader.into_inner()).await;
            return Ok(());
        }
        info!("Syncing {} missing files from {}", missing_files.len(), addr);

        let mut file_progress_bars = HashMap::new();

        if let Some(ref pm) = self.progress_manager {
            for path in &missing_files {
                if let Some(file_info) = remote_index.iter().find(|f| f.path == *path) {
                    let file_name = std::path::Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(path);
                    let pb = pm.create_receive_progress(file_name, file_info.size);
                    file_progress_bars.insert(path.clone(), pb);
                }
            }
        }

        let mut stream = reader.into_inner();

        for path in &missing_files {
            let req = TransferMsg::RequestFile(path.clone());
            let ser_req = bincode::serialize(&req)?;
            stream.write_all(&(ser_req.len() as u32).to_be_bytes()).await?;
            stream.write_all(&ser_req).await?;
        }

        let mut files_remaining = missing_files.len();
        let mut current_file: Option<std::fs::File> = None;
        let mut current_file_path: Option<String> = None;
        let mut current_hasher: Option<blake3::Hasher> = None;

        while files_remaining > 0 {
            let mut len_buf = [0u8; 4];
            if let Err(_) = stream.read_exact(&mut len_buf).await { break; }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut event_buf = vec![0u8; len];
            stream.read_exact(&mut event_buf).await?;

            match bincode::deserialize::<TransferMsg>(&event_buf)? {
                TransferMsg::Event(e) => {
                    if matches!(e.operation(), EventOp::Create | EventOp::Modify) {
                        let full = root.join(e.file_path());
                        util::ensure_parent(&full).ok();
                    }
                },
                TransferMsg::Chunk(chunk) => {
                    let rel_path_str = chunk.rel_path.to_string_lossy().to_string();
                    let full_path = root.join(&chunk.rel_path);

                    if current_file_path.as_ref() != Some(&rel_path_str) {
                        util::ensure_parent(&full_path)?;
                        let f = fs::OpenOptions::new().write(true).create(true).append(true)
                            .open(full_path.with_extension("part"))?;
                        current_file = Some(f);
                        current_file_path = Some(rel_path_str.clone());
                        current_hasher = Some(blake3::Hasher::new());
                    }

                    if let Some(file) = current_file.as_mut() {
                        file.write_all(&chunk.data)?;
                    }
                    if let Some(hasher) = current_hasher.as_mut() {
                        hasher.update(&chunk.data);
                    }

                    if let Some(pb) = file_progress_bars.get(&rel_path_str) {
                        pb.set_position(chunk.offset + chunk.data.len() as u64);
                    }

                    if chunk.is_last {
                        let hash_str = current_hasher.take().map(|h| h.finalize().to_hex().to_string());
                        current_file = None;
                        current_file_path = None;

                        let event = FileEvent::new(EventOp::Create, chunk.rel_path.clone(), hash_str)
                            .with_origin("network".to_string());
                        let _ = tx.send(event).await;

                        let part_path = full_path.with_extension("part");
                        fs::rename(&part_path, &full_path)?;

                        if let Some(pb) = file_progress_bars.remove(&rel_path_str) {
                            pb.finish_with_message(format!("✓ Synced"));
                        } else {
                            info!("Synced: {:?}", chunk.rel_path);
                        }
                        files_remaining -= 1;
                    }
                }
                _ => {}
            }
        }
        // self.store_stream(addr, stream).await;
        Ok(())
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent, root_path: &PathBuf) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;

        let event_msg = TransferMsg::Event(event.clone());
        let serialized = bincode::serialize(&event_msg)?;
        stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
        stream.write_all(&serialized).await?;

        if matches!(event.operation(), EventOp::Create | EventOp::Modify) {
            let (_reader, mut writer) = stream.into_split();

            let progress = if let Some(ref pm) = self.progress_manager {
                let full_path = root_path.join(event.file_path());
                if let Ok(metadata) = std::fs::metadata(&full_path) {
                    let file_size = metadata.len();
                    let file_name = event.file_path().file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    Some(pm.create_transfer_progress(file_name, file_size))
                } else {
                    None
                }
            } else {
                None
            };

            stream_file_to_writer(&mut writer, root_path, event.file_path(), progress).await?;

            // let reunited_stream = reader.reunite(writer).unwrap();
            // stream = reunited_stream;
            // if self.progress_manager.is_none() {
            //     println!("Sent: {:?}", event.file_path());
            // }
        }
        // self.store_stream(addr, stream).await;
        Ok(())
    }

    async fn acquire_stream(&self, addr: &str) -> anyhow::Result<TcpStream> {
        // if let Some(stream) = {
        //     let mut guard = self.streams.lock().await;
        //     guard.remove(addr)
        // } {
        //     return Ok(stream);
        // }

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

    // async fn store_stream(&self, addr: &str, stream: TcpStream) {
    //     let mut guard = self.streams.lock().await;
    //     guard.insert(addr.to_string(), stream);
    // }
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
async fn stream_file_to_writer(writer: &mut tokio::net::tcp::OwnedWriteHalf, root: &PathBuf, rel_path: &PathBuf, progress: Option<indicatif::ProgressBar>) -> anyhow::Result<()> {
    let full_path = root.join(rel_path);

    if let Ok(mut file) = fs::File::open(&full_path) {
        let file_size = file.metadata()?.len();

        if let Some(ref pb) = progress {
            pb.set_length(file_size);
        }

        if file_size == 0 {
            let chunk = FileChunk {
                rel_path: rel_path.clone(),
                offset: 0,
                data: vec![],
                is_last: true,
            };
            send_msg(writer, &TransferMsg::Chunk(chunk)).await?;
            if let Some(pb) = progress {
                pb.finish_with_message("✓ Sent".to_string());
            }
            return Ok(());
        }

        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut total_sent = 0u64;

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 { break; }

            total_sent += bytes_read as u64;
            let is_last = total_sent >= file_size;

            let chunk = FileChunk {
                rel_path: rel_path.clone(),
                offset: total_sent,
                data: buffer[..bytes_read].to_vec(),
                is_last,
            };
            let chunk_msg = TransferMsg::Chunk(chunk);
            send_msg(writer, &chunk_msg).await?;

            total_sent += bytes_read as u64;

            if let Some(ref pb) = progress {
                pb.set_position(total_sent);
            }
        }
        if let Some(pb) = progress {
            pb.finish_with_message("✓ Sent".to_string());
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