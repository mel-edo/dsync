use std::{collections::HashMap, fs, io::Write, net::IpAddr, path::PathBuf, sync::Arc, time::Instant};
use tokio::{io::{AsyncReadExt, BufReader}, net::TcpListener, sync::mpsc, time::Duration};
use governor::{Quota, RateLimiter};
use nonzero_ext::nonzero;
use tracing::{debug, info, warn, error};
use lz4_flex::block::decompress_size_prepended;

use crate::{
    core::event::{EventOp, FileEvent},
    core::protocol::TransferMsg,
    sync::ignore::IgnoreList,
    network::handshake::perform_server_handshake,
    network::transfer::{send_encrypted, decrypt_msg, stream_file_to_writer},
    network::progress::ProgressManager,
    sync::index as sync,
    util,
};

pub async fn start_server(
    port: u16,
    root: PathBuf,
    sender: mpsc::Sender<FileEvent>,
    instance_id: String,
    ignore: Arc<IgnoreList>,
    progress: Arc<ProgressManager>,
) -> anyhow::Result<()> {

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("Server listening on port {}", port);

    let limiter: Arc<RateLimiter<IpAddr, _, _>> = Arc::new(RateLimiter::dashmap(Quota::per_minute(nonzero!(50u32))));

    loop {
        let (mut stream, addr) = listener.accept().await?;
        debug!("New connection from {:?}", addr);

        // spawn a new task with tokio::spawn to handle multiple peers
        let tx = sender.clone();
        let root_clone = root.clone();
        let instance_id_clone = instance_id.clone();
        let ignore_clone = Arc::clone(&ignore);
        let progress_clone = Arc::clone(&progress);

        let ip = addr.ip();
        if limiter.check_key(&ip).is_err() {
            warn!("Rate limit exceeded from {}, dropping connection", ip);
            continue;
        }

        tokio::spawn(async move {
            let cipher = match perform_server_handshake(&mut stream).await {
                Ok((peer_id, cipher)) => {
                    info!("[Trust Established] Verified peer {}", &peer_id[..16]);
                    cipher
                },
                Err(e) => {
                    warn!("Handshake failed: {:?}", e);
                    return;
                }
            };

            // split stream into read/write halves for bidirectional communication
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            
            let mut current_file_path: Option<String> = None;
            let mut current_file: Option<std::fs::File> = None;
            let mut current_hasher: Option<blake3::Hasher> = None;
            let mut pending_transfers: HashMap<PathBuf, Instant> = HashMap::new();

            let mut active_progress = None;

            loop {
                let mut len_buf = [0u8; 4];
                match tokio::time::timeout(
                    Duration::from_secs(300),
                    buf_reader.read_exact(&mut len_buf)
                ).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => break,
                    Err(_) => {
                        warn!("Connection timed out from {}", addr);
                        break;
                    }
                }

                let msg_len = u32::from_be_bytes(len_buf) as usize;

                let mut buffer = vec![0u8; msg_len];
                if let Err(_) = buf_reader.read_exact(&mut buffer).await { break; }

                match decrypt_msg(&cipher, &buffer) {
                    
                    // peer wants our file list
                    Ok(msg) => match msg {
                        TransferMsg::IndexRequest => {
                            debug!("Received Index Request");
                            let index = sync::generate_local_index(&root_clone, &ignore_clone);
                            send_encrypted(&mut writer, &cipher, &TransferMsg::IndexResponse(index)).await.ok();
                        },

                        // peer sent us their list
                        TransferMsg::IndexResponse(remote_index) => {
                            debug!("Received Remote Index ({} files)", remote_index.len());
                            let local_index = sync::generate_local_index(&root_clone, &ignore_clone);
                            let missing = sync::calculate_diff(&local_index, &remote_index);

                            for path in missing {
                                debug!("Requesting: {}", path);
                                send_encrypted(&mut writer, &cipher, &TransferMsg::RequestFile(path)).await.ok();
                            }
                        },

                        // peer requested a specific file, send it
                        TransferMsg::RequestFile(path) => {
                            debug!("Sending requested file: {}", path);
                            let file_path = PathBuf::from(&path);

                            // send metadata first
                            let event = FileEvent::new(EventOp::Create, file_path.clone(), None);
                            send_encrypted(&mut writer, &cipher, &TransferMsg::Event(event)).await.ok();

                            // stream the file content using helper
                            if let Err(e) = stream_file_to_writer(&mut writer, &root_clone, &file_path, None, &cipher).await {
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

                                    let file_name = event.file_path()
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("unknown");
                                    active_progress = Some(progress_clone.create_spinner(&format!("Receiving: {}", file_name)));
                                }
                            }
                        },
                        TransferMsg::Chunk(chunk) => {
                            let rel_path_str = chunk.rel_path.to_string_lossy().to_string();
                            let full_path = root_clone.join(&chunk.rel_path);

                            if current_file_path.as_ref() != Some(&rel_path_str) {
                                util::ensure_parent(&full_path).ok();

                                match fs::OpenOptions::new().create(true).write(true).truncate(true).open(full_path.with_extension("part")) {
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

                            let data = if chunk.compressed {
                                match decompress_size_prepended(&chunk.data) {
                                    Ok(d) => d,
                                    Err(e) => {
                                        error!("Decompression failed for {}: {:?}", rel_path_str, e);
                                        current_file = None;
                                        continue;
                                    }
                                }
                            } else {
                                chunk.data.clone()
                            };
                            if let Some(file) = current_file.as_mut() {
                                if let Err(e) = file.write_all(&data) {
                                    error!("Write error for {}: {:?}", rel_path_str, e);
                                    current_file = None;
                                } else {
                                    if let Some(ref pb) = active_progress {
                                        let file_name = chunk.rel_path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("unknown");
                                        pb.set_message(format!("Receiving {} ({:.1} MB)", file_name, (chunk.offset + data.len() as u64) as f64 / 1_048_576.0));
                                    }
                                }
                            }
                            if let Some(hasher) = current_hasher.as_mut() {
                                hasher.update(&data);
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
                    Err(e) => {
                        warn!("Failed to decrypt message: {:?}", e);
                        break;
                    }
                }
            }
        });
    }
}