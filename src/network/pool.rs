use std::{collections::HashMap, fs, io::Write, path::PathBuf, sync::Arc};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::TcpStream, sync::{mpsc, Mutex}, time::{Duration, timeout}};
use anyhow::{Context, bail};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use tracing::{debug, info};
use blake3;
use lz4_flex::block::decompress_size_prepended;

use crate::{
    core::event::{EventOp, FileEvent},
    core::protocol::{HandshakeMsg, TransferMsg},
    sync::ignore::IgnoreList,
    network::transfer::{send_encrypted, decrypt_msg, stream_file_to_writer},
    network::progress::ProgressManager,
    sync::index as sync,
    util,
};

struct PooledConnection {
    writer: tokio::net::tcp::OwnedWriteHalf,
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    cipher: Arc<ChaCha20Poly1305>,
}

pub struct ConnectionPool {
    my_keypair: Arc<SigningKey>,
    progress_manager: Option<Arc<ProgressManager>>,
    ignore: Arc<IgnoreList>,
    connections: Arc<Mutex<HashMap<String, PooledConnection>>>,
}

impl ConnectionPool {
    pub fn new(keypair: SigningKey, ignore: Arc<IgnoreList>) -> Self {
        Self {
            my_keypair: Arc::new(keypair),
            progress_manager: Some(Arc::new(ProgressManager::new())),
            ignore,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn request_index(&self, addr: &str, root:PathBuf, tx: mpsc::Sender<FileEvent>) -> anyhow::Result<()> {
        let mut conn = self.checkout(addr).await?;

        send_encrypted(&mut conn.writer, &conn.cipher, &TransferMsg::IndexRequest).await?;

        let mut len_buf = [0u8; 4];
        conn.reader.read_exact(&mut len_buf).await?;
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        let mut buffer = vec![0u8; msg_len];
        conn.reader.read_exact(&mut buffer).await?;

        let remote_index = match decrypt_msg(&conn.cipher, &buffer)? {
            TransferMsg::IndexResponse(idx) => idx,
            _ => bail!("Expected IndexResponse"),
        };

        let local_index = sync::generate_local_index(&root, &self.ignore);
        let missing_files = sync::calculate_diff(&local_index, &remote_index);

        if missing_files.is_empty() {
            self.checkin(addr, conn).await;
            return Ok(());
        }
        info!("Syncing {} missing/outdated files from {}", missing_files.len(), addr);

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

        for path in &missing_files {
            send_encrypted(&mut conn.writer, &conn.cipher, &TransferMsg::RequestFile(path.clone())).await?;
        }

        let mut files_remaining = missing_files.len();
        let mut current_file: Option<std::fs::File> = None;
        let mut current_file_path: Option<String> = None;
        let mut current_hasher: Option<blake3::Hasher> = None;

        while files_remaining > 0 {
            let mut len_buf = [0u8; 4];
            if let Err(_) = conn.reader.read_exact(&mut len_buf).await { break; }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut event_buf = vec![0u8; len];
            conn.reader.read_exact(&mut event_buf).await?;

            match decrypt_msg(&conn.cipher, &event_buf)? {
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
                        let f = fs::OpenOptions::new().write(true).create(true).truncate(true)
                            .open(full_path.with_extension("part"))?;
                        current_file = Some(f);
                        current_file_path = Some(rel_path_str.clone());
                        current_hasher = Some(blake3::Hasher::new());
                    }

                    let data = if chunk.compressed {
                        decompress_size_prepended(&chunk.data)
                            .map_err(|e| anyhow::anyhow!("Decompression failed: {:?}", e))?
                    } else {
                        chunk.data.clone()
                    };

                    if let Some(file) = current_file.as_mut() {
                        file.write_all(&data)?;
                    }
                    if let Some(hasher) = current_hasher.as_mut() {
                        hasher.update(&data);
                    }

                    if let Some(pb) = file_progress_bars.get(&rel_path_str) {
                        pb.set_position(chunk.offset + data.len() as u64);
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
                            info!("Synced: {:?}", chunk.rel_path.display());
                        }
                        files_remaining -= 1;
                    }
                }
                _ => {}
            }
        }
        self.checkin(addr, conn).await;
        Ok(())
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent, root_path: &PathBuf) -> anyhow::Result<()> {

        let mut conn = self.checkout(addr).await?;
        send_encrypted(&mut conn.writer, &conn.cipher, &TransferMsg::Event(event.clone())).await?;

        if matches!(event.operation(), EventOp::Create | EventOp::Modify) {
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

            stream_file_to_writer(&mut conn.writer, root_path, event.file_path(), progress, &conn.cipher).await?;
        }
        self.checkin(addr, conn).await;
        Ok(())
    }

    async fn connect_and_handshake(&self, addr: &str) -> anyhow::Result<(TcpStream, ChaCha20Poly1305)> {
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
                let client_secret = EphemeralSecret::random_from_rng(OsRng);
                let client_public = X25519PublicKey::from(&client_secret);

                // receive server's X25519 public key
                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf).await?;
                let mut buf = vec![0u8; u32::from_be_bytes(len_buf) as usize];
                stream.read_exact(&mut buf).await?;

                let server_key_msg: HandshakeMsg = bincode::deserialize(&buf)?;
                if let HandshakeMsg::EphemeralKey(server_key_bytes) = server_key_msg {
                    // send our key back
                    let ack = HandshakeMsg::EphemeralKeyAck(client_public.to_bytes());
                    let serialized = bincode::serialize(&ack)?;
                    stream.write_all(&(serialized.len() as u32).to_be_bytes()).await?;
                    stream.write_all(&serialized).await?;

                    let shared_secret = client_secret.diffie_hellman(&X25519PublicKey::from(server_key_bytes));
                    let cipher = ChaCha20Poly1305::new_from_slice(shared_secret.as_bytes())
                        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {:?}", e))?;


                    return Ok((stream, cipher));
                }
            }
        }
        bail!("Handshake failed with {}", addr)
    }

    async fn checkout(&self, addr: &str) -> anyhow::Result<PooledConnection> {
        {
            let mut pool = self.connections.lock().await;
            if let Some(conn) = pool.remove(addr) {
                debug!("Reusing pooled connection to {}", addr);
                return Ok(conn);
            }
        }

        debug!("Creating new connection to {}", addr);
        let (stream, cipher) = self.connect_and_handshake(addr).await?;
        let cipher = Arc::new(cipher);
        let (reader_half, writer_half) = stream.into_split();
        Ok(PooledConnection {
            writer: writer_half,
            reader: BufReader::new(reader_half),
            cipher,
        })
    }

    async fn checkin(&self, addr: &str, conn: PooledConnection) {
        let mut pool = self.connections.lock().await;
        pool.insert(addr.to_string(), conn);
        debug!("Returned connection to pool for {}", addr);
    }
}