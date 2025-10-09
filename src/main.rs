use std::{collections::HashMap, fs::read, path::PathBuf, sync::Arc};

use blake3::Hash;
use tokio::sync::{mpsc, Mutex};

use crate::watcher::watch_folder;

mod event;
mod network;
mod sync;
mod util;
mod watcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(100);
    let _watcher = watch_folder(PathBuf::from("./test_folder"), tx.clone()).await?;

    let last_hashes: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::new(Mutex::new(HashMap::<PathBuf, Hash>::new()));
    let last_hashes_clone: Arc<Mutex<HashMap<PathBuf, Hash>>> = Arc::clone(&last_hashes);

    // start tcp server
    tokio::spawn({
        let tx_clone = tx.clone();
        async move {
            if let Err(e) = network::start_server(9000, tx_clone).await {
                eprintln!("Server error: {:?}", e);
            }
        }
    });

    // forward events to peers
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("Got FileEvent in main: {:?}", event);

            // don't forward loopback events
            if event.file_path().to_string_lossy().contains("./test_folder") {
                println!("Skipping loopback events {:?}", event.file_path());
                continue;
            }

            let hash = match read(&event.file_path()) {
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

            if let Err(e) = network::send_event("127.0.0.1:9000", event).await {
                eprintln!("Failed to send event: {:?}", e);
            }
        }
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3000)).await;
    }
}
