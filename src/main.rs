use std::{collections::HashMap, fs::read, path::PathBuf, sync::Arc};
use clap::Parser;
use blake3::Hash;
use tokio::sync::{mpsc, Mutex};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

use crate::{
    event::EventOp,
    network::ConnectionPool,
    watcher::watch_folder,
};

mod event;
mod network;
mod sync;
mod util;
mod watcher;
mod discovery;
pub mod protocol;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Folder path to sync
    #[arg(short = 'd', long)]
    path: String,

    /// Port to listen on (default: 9000)
    #[arg(short = 'p', long, default_value_t = 9000)]
    port: u16,

    /// Peer address to connect to (format: ip:port)
    #[arg(short = 'a', long)]
    peer: Option<String>,

    /// Unique name for this instance
    #[arg(short = 'n', long, default_value = "dsync-instance")]
    name: String,

    /// Disable automatic local peer discovery
    #[arg(long, default_value_t = false)]
    no_discovery: bool,

    /// Show detailed connection logs
    #[arg(short = 'v', long, default_value_t = false)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let verbose = args.verbose;

    // generate ephemeral identity
    let mut csprng = OsRng{};
    let keypair = SigningKey::generate(&mut csprng);
    let my_public_key = keypair.verifying_key().to_bytes();

    let my_id_hex = hex::encode(my_public_key);
    println!("Ephemeral ID: {}", my_id_hex);

    let folder_path = PathBuf::from(args.path);
    let port = args.port;
    let peer_addr = args.peer;
    let instance_id = args.name.clone();

    let (tx, mut rx) = mpsc::channel(100);
    let (peer_tx, mut peer_rx) = mpsc::channel(100);
    let _watcher = watch_folder(folder_path.clone(), tx.clone()).await?;

    let last_hashes: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::new(Mutex::new(HashMap::<PathBuf, Hash>::new()));
    let last_hashes_clone: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::clone(&last_hashes);
    let connection_pool = Arc::new(ConnectionPool::new(keypair));

    // setup mdns discovery
    let peer_discovery = if !args.no_discovery {
        let discovery = discovery::PeerDiscovery::new(my_id_hex)?;
        discovery.register_service(port, &args.name)?;
        discovery.start_browsing(peer_tx).await?;
        Some(Arc::new(discovery))
    } else {
        None
    };

    // start tcp server
    tokio::spawn({
        let tx_clone = tx.clone();
        let folder_clone = folder_path.clone();
        let instance_id_clone = instance_id.clone();
        async move {
            if let Err(e) = network::start_server(port, folder_clone, tx_clone, instance_id_clone, verbose).await {
                eprintln!("Server error: {:?}", e);
            }
        }
    });

    let pool_clone = Arc::clone(&connection_pool);
    let folder_clone_2 = folder_path.clone();
    let tx_clone_2 = tx.clone();

    tokio::spawn(async move {
        while let Some(peer_addr) = peer_rx.recv().await {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            if verbose { println!("Initiating sync with {}", peer_addr); }

            if let Err(e) = pool_clone.request_index(&peer_addr, folder_clone_2.clone(), tx_clone_2.clone()).await {
                eprintln!("Sync failed with {}: {:?}", peer_addr, e);
            } else {
                if verbose { println!("Sync check complete with {}", peer_addr); }
            }
        }
    });

    // forward events to peers
    tokio::spawn(async move {
        let connection_pool = Arc::clone(&connection_pool);
        while let Some(mut event) = rx.recv().await {
            if event.origin_id().is_some() {
                let abs_path = folder_path.join(event.file_path());
                if let Ok(contents) = read(&abs_path) {
                    let hash = blake3::hash(&contents);
                    let mut map = last_hashes_clone.lock().await;
                    map.insert(event.file_path().clone(), hash);
                }
                continue;
            }
            
            if verbose {
                println!("Got FileEvent in main: {:?}", event);
            }

            let current_hash = if matches!(event.operation(), EventOp::Delete) {
                None
            } else {
                let abs_path = folder_path.join(event.file_path());
                match read(&abs_path) {
                    Ok(contents) => Some(blake3::hash(&contents)),
                    Err(_) => None,
                }
            };

            let mut map = last_hashes_clone.lock().await;
            match current_hash {
                Some(hash) => {
                    if let Some(prev_hash) = map.get(event.file_path().as_path()) {
                        if *prev_hash == hash { continue; }
                    }
                    map.insert(event.file_path().clone(), hash);
                }
                None => { map.remove(event.file_path().as_path()); }
            }
            
            // adding orgin id to prevent loops
            event = event.with_origin(instance_id.clone());

            // get peers to send to
            let mut peers = Vec::new();
            if let Some(ref peer) = peer_addr {
                peers.push(peer.clone());
            }
            if let Some(ref discovery) = peer_discovery {
                peers.extend(discovery.get_peers().await);
            }

            for peer in peers {
                if let Err(e) = connection_pool.send_event(&peer, &event, &folder_path).await {
                    eprintln!("Failed to send event to {}: {:?}", peer, e);
                }
            }
        }
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3000)).await;
    }
}
