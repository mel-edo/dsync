use std::{collections::HashMap, fs::{create_dir_all, metadata, remove_file, write}, path::{PathBuf}, sync::Arc};
use tokio::{io::{AsyncReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::{mpsc, Mutex}, time::{timeout, Duration}};
use anyhow::{Context, bail};
use crate::event::{EventOp, FileEvent};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{RngCore, rngs::OsRng};
use crate::protocol::HandshakeMsg;

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
                if let Err(_) = reader.read_exact(&mut len_buf).await {
                    break;
                }
                
                let msg_len = u32::from_be_bytes(len_buf) as usize;
                let mut buffer = vec![0u8; msg_len];

                if let Err(e) = reader.read_exact(&mut buffer).await {
                    println!("Failed to read message: {:?}", e);
                    break;
                }

                match bincode::deserialize::<FileEvent>(&buffer) {
                    Ok(event) => {
                        // skip events that originated from this instance
                        if let Some(origin) = event.origin_id() {
                            if origin == &instance_id_clone { continue; }
                        }

                        match event.operation() {
                            EventOp::Create | EventOp::Modify => {
                                if let Some(bytes) = event.data().clone() {
                                    let rel_path = event.file_path();

                                    let mut full_path = root_clone.clone();
                                    full_path.push(rel_path);

                                    // check if parent directory exists or not and create it if it doesen't
                                    if let Some(parent) = full_path.parent() {
                                        let _ = create_dir_all(parent);
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
                                            eprintln!("Write error: {:?}", e);
                                        } else {
                                            println!("Synced: {:?}", full_path);
                                        }
                                    }
                                }
                            }
                            EventOp::Delete => {
                                let mut full_path = root_clone.clone();
                                full_path.push(event.file_path());
                                if full_path.exists() {
                                    let _ = remove_file(&full_path);
                                    println!("Deleted: {:?}", full_path);
                                }
                            }
                        }
                        let _ = tx.send(event).await;
                    }
                    Err(e) => eprintln!("Deserialize error: {:?}", e),
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
    my_keypair: Arc<SigningKey>,
}

impl ConnectionPool {
    pub fn new(keypair: SigningKey) -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
            my_keypair: Arc::new(keypair),
        }
    }

    pub async fn send_event(&self, addr: &str, event: &FileEvent) -> anyhow::Result<()> {
        let mut stream = self.acquire_stream(addr).await?;
        let serialized = bincode::serialize(event)?;
        let len = serialized.len() as u32;

        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&serialized).await?;

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
            let len = u32::from_be_bytes(ack_len) as usize;
            let mut ack_buf = vec![0u8; len];
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
        let verifying_key = VerifyingKey::from_bytes(&public_key)?;
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