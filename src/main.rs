use std::{collections::HashMap, path::PathBuf, sync::Arc};
use clap::Parser;
use blake3::Hash;
use tokio::{sync::{mpsc, Mutex}, io::AsyncReadExt, signal};
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use tracing::{debug, error, info, warn};

use crate::{
    event::EventOp,
    network::ConnectionPool,
    watcher::watch_folder,
};

mod progress;
mod event;
mod network;
mod sync;
mod util;
mod watcher;
mod discovery;
pub mod protocol;

#[derive(Debug, serde::Deserialize, Default)]
struct Config {
    path: Option<String>,
    port: Option<u16>,
    peer: Option<String>,
    name: Option<String>,
    no_discovery: Option<bool>,
}

fn load_config() -> Config {
    let config_path = dirs::config_dir()
        .map(|d| d.join("dsync").join("dsync.toml"))
        .filter(|p| p.exists());

    let config_path = match config_path {
        Some(p) => p,
        None => return Config::default(),
    };

    match std::fs::read_to_string(&config_path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
            warn!("Failed to parse {:?}: {}", config_path, e);
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Folder path to sync
    #[arg(short = 'd', long)]
    path: Option<String>,

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

    let config = load_config();

    let path = args.path.or(config.path).expect("No sync folder specified. Use -d or set 'path' in dsync.toml");
    let port = if args.port != 9000 { args.port } else { config.port.unwrap_or(args.port) };
    let peer = args.peer.or(config.peer);
    let name = if args.name != "dsync-instance" { args.name } else { config.name.unwrap_or(args.name) };
    let no_discovery = args.no_discovery || config.no_discovery.unwrap_or(false);

    let log_level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .with_target(false)
        .init();

    // generate ephemeral identity
    let mut csprng = OsRng{};
    let keypair = SigningKey::generate(&mut csprng);
    let my_public_key = keypair.verifying_key().to_bytes();

    let my_id_hex = hex::encode(my_public_key);
    info!("Ephemeral ID: {}", &my_id_hex[..16]);

    let folder_path = PathBuf::from(path);
    let port = port;
    let peer_addr = peer;
    let instance_id = my_id_hex.clone();

    let (tx, mut rx) = mpsc::channel(100);
    let (peer_tx, mut peer_rx) = mpsc::channel(100);
    let _watcher = watch_folder(folder_path.clone(), tx.clone()).await?;

    let last_hashes: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::new(Mutex::new(HashMap::<PathBuf, Hash>::new()));
    let connection_pool = Arc::new(ConnectionPool::new(keypair));

    let initial_sync_complete = Arc::new(Mutex::new(false));
    // setup mdns discovery
    let peer_discovery = if !no_discovery {
        let discovery = discovery::PeerDiscovery::new(my_id_hex)?;
        discovery.register_service(port, &name)?;
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
            if let Err(e) = network::start_server(port, folder_clone, tx_clone, instance_id_clone).await {
                error!("Server error: {:?}", e);
            }
        }
    });

    {
        let pool = Arc::clone(&connection_pool);
        let folder = folder_path.clone();
        let tx_clone = tx.clone();
        let sync_complete = Arc::clone(&initial_sync_complete);

        tokio::spawn(async move {
            while let Some(peer_addr) = peer_rx.recv().await {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                debug!("Initiating sync with {}", peer_addr);

                if let Err(e) = pool.request_index(&peer_addr, folder.clone(), tx_clone.clone()).await {
                    error!("Sync failed with {}: {:?}", peer_addr, e);
                } else {
                    debug!("Sync check complete with {}", peer_addr);

                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                    let mut complete = sync_complete.lock().await;
                    if !*complete {
                        *complete = true;
                        info!("✓ Initial sync complete, file watcher now active");
                    }
                }
            }
        });
    }

    // forward events to peers
    {
        let connection_pool = Arc::clone(&connection_pool);
        let folder_path = folder_path.clone();
        let peer_addr = peer_addr.clone();
        let peer_discovery = peer_discovery.clone();
        let last_hashes = Arc::clone(&last_hashes);
        let instance_id = instance_id.clone();
        let initial_sync_complete = Arc::clone(&initial_sync_complete);

        tokio::spawn(async move {
            while let Some(mut event) = rx.recv().await {
                if event.origin_id().is_some() {
                    let abs_path = folder_path.join(event.file_path());
    
                    if let Ok(mut file) = tokio::fs::File::open(&abs_path).await {
                        let mut hasher = blake3::Hasher::new();
                        let mut buffer = vec![0u8; 64 * 1024];
    
                        while let Ok(n) = file.read(&mut buffer).await {
                            if n == 0 { break; }
                            hasher.update(&buffer[..n]);
                        }
    
                        let hash = hasher.finalize();
                        let mut map = last_hashes.lock().await;
                        map.insert(event.file_path().clone(), hash);

                        debug!("Stored hash for network file: {:?} -> {}", event.file_path(), hash.to_hex());
                    }
    
                    continue;
                }

                // skip watcher events during initial sync phase
                {
                    let sync_done = initial_sync_complete.lock().await;
                    if !*sync_done {
                        debug!("Skipping watcher event during initial sync: {:?}", event.file_path());
                        continue;
                    }
                }

                if matches!(event.operation(), EventOp::Delete) {
                    if let Some(parent) = event.file_path().parent() {
                        if !parent.as_os_str().is_empty() {
                            let parent_full = folder_path.join(parent);
                            if !parent_full.exists() {
                                debug!("Skipping file deletion {:?} (parent directory deleted)", event.file_path());
                                continue;
                            }
                        }
                    }
                }
    
                debug!("Got FileEvent in main: {:?}", event);

                let current_hash = if matches!(event.operation(), EventOp::Delete) {
                    None
                } else {
                    let abs_path = folder_path.join(event.file_path());
                    match tokio::fs::File::open(&abs_path).await {
                        Ok(mut file) => {
                            let mut hasher = blake3::Hasher::new();
                            let mut buffer = vec![0u8; 64 * 1024];
            
                            while let Ok(n) = file.read(&mut buffer).await {
                                if n == 0 { break; }
                                hasher.update(&buffer[..n]);
                            }
                            Some(hasher.finalize())
                        }
                        Err(_) => None,
                    }
                };

                {
                    let mut map = last_hashes.lock().await;
                    match current_hash {
                        Some(hash) => {
                            if let Some(prev_hash) = map.get(event.file_path().as_path()) {
                                if *prev_hash == hash {
                                    debug!("Skipping duplicated: {:?} (hash unchanged)", event.file_path());
                                    continue;
                                }
                            }
                            map.insert(event.file_path().clone(), hash);
                        }
                        None => { map.remove(event.file_path().as_path()); }
                    }
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
                        warn!("Failed to send event to {}: {:?}", peer, e);
                        }
                    }
                }
        });
    }
    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("\n Shutting down...");

            // clean up .part files
            info!("Cleaning up temporary files...");
            cleanup_part_files(&folder_path).await;

            info!("Cleanup complete, Goodbye!");
            std::process::exit(0);
        }
    }
}

async fn cleanup_part_files(path: &std::path::Path) {
    use walkdir::WalkDir;

    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("part") {
            if let Err(e) = std::fs::remove_file(path) {
                warn!("Failed to remove file {:?}: {}", path, e);
            } else {
                debug!("Removed: {:?}", path);
            }
        }
    }
}
