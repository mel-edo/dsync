use std::{collections::HashMap, fs::read, path::PathBuf, sync::Arc};
use clap::Parser;

use blake3::Hash;
use tokio::sync::{mpsc, Mutex};

use crate::watcher::watch_folder;

mod event;
mod network;
mod sync;
mod util;
mod watcher;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    // Folder to sync
    #[arg(short = 'd', long, default_value = "./test_folder")]
    path: String,

    // Port to listen on
    #[arg(short = 'p', long, default_value_t = 9000)]
    port: u16,

    // peer address
    #[arg(short = 'a', long)]
    peer: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let folder_path = PathBuf::from(args.path);
    let port = args.port;
    let peer_addr = args.peer;

    let (tx, mut rx) = mpsc::channel(100);
    let _watcher = watch_folder(folder_path.clone(), tx.clone()).await?;

    let last_hashes: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::new(Mutex::new(HashMap::<PathBuf, Hash>::new()));
    let last_hashes_clone: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::clone(&last_hashes);

    // start tcp server
    tokio::spawn({
        let tx_clone = tx.clone();
        let folder_clone = folder_path.clone();
        async move {
            if let Err(e) = network::start_server(port, folder_clone, tx_clone).await {
                eprintln!("Server error: {:?}", e);
            }
        }
    });

    // forward events to peers
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("Got FileEvent in main: {:?}", event);

            // don't forward loopback events
            // if event.file_path().starts_with(&folder_path) {
                // println!("Skipping loopback events {:?}", event.file_path());
                // continue;
            // }

            let abs_path = folder_path.join(event.file_path());
            let hash = match read(&abs_path) {
                Ok(contents) => blake3::hash(&contents),
                Err(_) => {
                    // file may be deleted for which we would have to delete it
                    let mut map = last_hashes_clone.lock().await;
                    map.remove(event.file_path().as_path());
                    continue;
                }
            };

            let mut map = last_hashes_clone.lock().await;
            if let Some(prev_hash) = map.get(event.file_path().as_path()) {
                if *prev_hash == hash {
                    continue;
                }
            }
            map.insert(event.file_path().clone(), hash);

            if let Some(ref peer) = peer_addr {
                if let Err(e) = network::send_event(peer, event).await {
                    eprintln!("Failed to send event: {:?}", e);
                }
            } else {
                println!("No peer specified, skipping send");
            }
        }
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3000)).await;
    }
}
