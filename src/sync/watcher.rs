use std::{collections::HashMap, path::{PathBuf, Path}, sync::Arc, time::Instant};
use pathdiff::diff_paths;
use tokio::{fs::File, sync::{mpsc, Mutex}, io::AsyncReadExt};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::debug;
use blake3::Hasher;
use crate::{
    core::event::{EventOp, FileEvent},
    sync::ignore::IgnoreList,
};

const HASH_BUFFER_SIZE: usize = 64 * 1024;

pub async fn watch_folder(root: PathBuf, sender: mpsc::Sender<FileEvent>, ignore: Arc<IgnoreList>) -> notify::Result<RecommendedWatcher> {
    let root = root.canonicalize().unwrap_or(root);
    let debounce_map: Arc<Mutex<HashMap<PathBuf, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    let handle = tokio::runtime::Handle::current();

    let root_clone = root.clone();
    let sender_clone = sender.clone();
    let debounce_clone = debounce_map.clone();
    let ignore_clone = Arc::clone(&ignore);

    let mut file_watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let root = root_clone.clone();
            let tx = sender_clone.clone();
            let debounce = debounce_clone.clone();
            let handle = handle.clone();
            let ignore = ignore_clone.clone();

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
                    let should_process = {
                        let mut map = debounce.lock().await;
                        if let Some(&last_time) = map.get(&path) {
                            if last_time.elapsed() < std::time::Duration::from_secs(10) {
                                false
                            } else {
                                map.insert(path.clone(), Instant::now());
                                true
                            }
                        } else {
                            map.insert(path.clone(), Instant::now());
                            true
                        }
                    };

                    if !should_process {
                        return;
                    }

                    let _ = async {

                        let relative = match diff_paths(&path, &root) {
                            Some(p) => p,
                            None => return,
                        };

                        if ignore.is_ignored(&relative) {
                            debug!("Ignoring watcher event for {:?}", relative);
                            return;
                        }
        
                        match tokio::fs::metadata(&path).await {
                            Ok(meta) => {
                                if meta.is_dir() {
                                    return;
                                }
        
                                let mut last_size = meta.len();
                                let mut last_modified = meta.modified().ok();
        
                                loop {
                                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        
                                    match tokio::fs::metadata(&path).await {
                                        Ok(new_meta) => {
                                            let new_size = new_meta.len();
                                            let new_modified = new_meta.modified().ok();
        
                                            if new_size == last_size && new_modified == last_modified {
                                                break;
                                            }
        
                                            last_size = new_size;
                                            last_modified = new_modified;
                                        }
                                        Err(_) => return,
                                    }
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
                    }
                    .await;
                });
            }
        },
        Config::default(),
    )?;
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