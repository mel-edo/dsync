use std::{collections::HashMap, fs::{create_dir_all, metadata, remove_file, write}, path::{PathBuf}};

use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::{mpsc, Mutex}};

use anyhow::Context;

use crate::event::{EventOp, FileEvent};


pub async fn start_server(port: u16, root: PathBuf, sender: mpsc::Sender<FileEvent>, instance_id: String) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!("Server listening on port {}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        println!("New connection from {:?}", addr);
        // spawn a new task with tokio::spawn to handle multiple peers

        let tx = sender.clone();
        let root_clone = root.clone();
        let instance_id_clone = instance_id.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stream);

            loop {
                let mut len_buf = [0u8; 4];
                if let Err(e) = reader.read_exact(&mut len_buf).await {
                    println!("Connection closed on error {:?}", e);
                    break;
                }
                
                let msg_len = u32::from_be_bytes(len_buf) as usize;
                let mut buffer = vec![0u8; msg_len];

                if let Err(e) = reader.read_exact(&mut buffer).await {
                    println!("Failed to read full message: {:?}", e);
                    break;
                }

                match bincode::deserialize::<FileEvent>(&buffer) {
                    Ok(event) => {
                        // skip events that originated from this instance
                        if let Some(origin) = event.origin_id() {
                            if origin == &instance_id_clone {
                                println!("Skipping event from self (origin: {})", origin);
                                continue;
                            }
                        }
                        match event.operation() {
                            EventOp::Create | EventOp::Modify => {
                                if let Some(bytes) = event.data().clone() {
                                    let rel_path = event.file_path();
                                    println!("Received relative path: {:?}", rel_path);

                                    let mut full_path = root_clone.clone();
                                    full_path.push(rel_path);

                                    // check if parent directory exists or not and create it if it doesen't
                                    if let Some(parent) = full_path.parent() {
                                        if let Err(e) = create_dir_all(parent) {
                                            eprintln!("Failed to create parent directories for {:?} with error {:?}", full_path, e);
                                            continue;
                                        }
                                    }
                                    // check last write
                                    let should_write = match metadata(&full_path) {
                                        Ok(meta) => {
                                            if let Ok(modified) = meta.modified() {
                                                // changing '<' to '<=' to accept updates happening in the same millisecond
                                                modified <= *event.timestamp()
                                            } else { true }
                                        }
                                        Err(_) => true, // file doesen't exist
                                    };

                                    if should_write {
                                        if let Err(e) = write(&full_path, &bytes) {
                                            eprintln!("Failed to write file {:?}: {:?}", full_path, e);
                                        } else {
                                            println!("File written: {:?}", full_path);
                                        }
                                    } else {
                                        println!("Skipped {:?}, local file is newer.", full_path);
                                    }
                                }
                            }
                            EventOp::Delete => {
                                let mut full_path = root_clone.clone();
                                full_path.push(event.file_path());
                                if full_path.exists() {
                                    if let Err(e) = remove_file(&full_path) {
                                        eprintln!("Failed to delete file {:?}: {:?}", full_path, e);
                                    } else {
                                        println!("File deleted: {:?}", full_path);
                                    }
                                }
                            }
                        }
                        if let Err(e) = tx.send(event).await {
                            eprintln!("Channel send error: {:?}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to deserialize event: {:?}", e);
                        break;
                    }
                }
            }
        });
    }
    //if should_stop() { Add a stop condn for graceful shutdown
    //    break;
    //}
    //Ok(())
}

pub struct ConnectionPool {
    streams: Mutex<HashMap<String, TcpStream>>,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;
        let serialized = bincode::serialize(event)?;
        let len = serialized.len() as u32;

        stream
            .write_all(&len.to_be_bytes())
            .await
            .with_context(|| format!("failed to send length to {addr}"))?;

        stream
            .write_all(&serialized)
            .await
            .with_context(|| format!("failed to send payload to {addr}"))?;

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

        TcpStream::connect(addr)
            .await
            .with_context(|| format!("failed to connect to {addr}"))
    }

    async fn store_stream(&self, addr: &str, stream: TcpStream) {
        let mut guard = self.streams.lock().await;
        guard.insert(addr.to_string(), stream);
    }
}