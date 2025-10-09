use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::mpsc};

use crate::event::FileEvent;


pub async fn start_server(port: u16, sender: mpsc::Sender<FileEvent>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!("Server listening on port {}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        println!("New connection from {:?}", addr);
        // spawn a new task with tokio::spawn to handle multiple peers

        let tx = sender.clone();

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