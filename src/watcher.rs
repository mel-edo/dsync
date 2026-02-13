use std::{collections::HashMap, path::{PathBuf, Path}, sync::Arc, time::Instant};
use pathdiff::diff_paths;
use tokio::{fs::File, sync::{mpsc, Mutex}, io::AsyncReadExt};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use crate::event::{FileEvent, EventOp};
use blake3::Hasher;

const DEBOUNCE_MS: u64 = 500;
const HASH_BUFFER_SIZE: usize = 64 * 1024;

pub async fn watch_folder(root: PathBuf, sender: mpsc::Sender<FileEvent>) -> notify::Result<RecommendedWatcher> {
    let root = root.canonicalize().unwrap_or(root);
    let debounce_map: Arc<Mutex<HashMap<PathBuf, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let handle = tokio::runtime::Handle::current();

    let root_clone = root.clone();
    let sender_clone = sender.clone();
    let debounce_clone = debounce_map.clone();

    let mut file_watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let root = root_clone.clone();
            let tx = sender_clone.clone();
            let debounce = debounce_clone.clone();
            let handle = handle.clone();

            if let Ok(event) = res {
                if event.paths.is_empty() {
                    return;
                }
                let path = event.paths[0].clone();
            
                // ignore .part files
                if path.extension().map(|e| e == "part").unwrap_or(false) {
                    return;
                }

                let root = root.clone();

                handle.spawn(async move {
                    // debounce per file
                    {
                        let mut map = debounce.lock().await;
                        let now = Instant::now();

                        if let Some(last) = map.get(&path) {
                            if now.duration_since(*last).as_millis() < DEBOUNCE_MS as u128 {
                                return;
                            }
                        }
                        map.insert(path.clone(), now);
                    }

                    let relative = match diff_paths(&path, &root) {
                        Some(p) => p,
                        None => return,
                    };

                    match tokio::fs::metadata(&path).await {
                        Ok(meta) => {
                            if meta.is_dir() {
                                return;
                            }

                            match hash_file(&path).await {
                                Ok(hash) => {
                                    let event = FileEvent::new(EventOp::Modify, relative, Some(hash));
                                    let _ = tx.send(event).await;
                                }
                                Err(_) => {}
                            }
                        }
                        Err(e) => {
                            if e.kind() == std::io::ErrorKind::NotFound {
                                let event = FileEvent::new(EventOp::Delete, relative, None);
                                let _ = tx.send(event).await;
                            }
                        }
                    }
                });
            }
        },
        Config::default())?;
    file_watcher.watch(&root, RecursiveMode::Recursive)?;
    Ok(file_watcher)
}

async fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; HASH_BUFFER_SIZE];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}