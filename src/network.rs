use std::{fs::{create_dir_all, metadata, remove_file, write}, path::{PathBuf}};

use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::mpsc};

use crate::event::{EventOp, FileEvent};


pub async fn start_server(port: u16, root: PathBuf, sender: mpsc::Sender<FileEvent>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!("Server listening on port {}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        println!("New connection from {:?}", addr);
        // spawn a new task with tokio::spawn to handle multiple peers

        let tx = sender.clone();
        let root_clone = root.clone();

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
                        match event.operation() {
                            EventOp::Create | EventOp::Modify => {
                                if let Some(bytes) = event.data().clone() {
                                    let mut full_path = root_clone.clone();
                                    full_path.push(event.file_path());

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
                                                modified < *event.timestamp()
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

pub async fn send_event(addr: &str, event: FileEvent) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(addr).await?;
    let serialized = bincode::serialize(&event)?;

    let len = serialized.len() as u32;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&serialized).await?;

    Ok(())
}