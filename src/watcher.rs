use std::{io::ErrorKind, path::PathBuf};
use pathdiff::diff_paths;
use tokio::{fs, sync::mpsc};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher, Error};
use crate::event::{FileEvent, EventOp};
use blake3;

pub async fn watch_folder(root: PathBuf, sender: mpsc::Sender<FileEvent>) -> notify::Result<RecommendedWatcher> {
    let tx = sender.clone();
    let handle = tokio::runtime::Handle::current();
    let root_for_watcher = root.clone();

    let mut file_watcher = RecommendedWatcher::new(
        move |res: Result<Event, Error>| {
            let root = root_for_watcher.clone();
            let handle = handle.clone();

            match res {
                Ok(event) => {
                    let operation = match event.kind {
                        notify::EventKind::Create(_) => EventOp::Create,
                        notify::EventKind::Modify(_) => EventOp::Modify,
                        notify::EventKind::Remove(_) => EventOp::Delete,
                        _ => return,
                    };

                    if event.paths.is_empty() { return; }
                    let absolute_path = event.paths[0].clone();

                    // resolve paths
                    let canonical_root = match root.canonicalize() {
                        Ok(p) => p,
                        Err(_) => root.clone(),
                    };
                    let relative_path = match diff_paths(&absolute_path, &canonical_root) {
                        Some(p) => p,
                        None => match diff_paths(&absolute_path, &root) {
                            Some(p) => p,
                            None => return,
                        }
                    };

                    let tx2 = tx.clone();
                    let op = operation.clone();

                    handle.spawn(async move {

                        // debounce to wait for write to finish
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        let file_event = match op {
                            EventOp::Delete => FileEvent::new(EventOp::Delete, relative_path, None),
                            _ => {
                                match fs::read(&absolute_path).await {
                                    Ok(bytes) => {
                                        let hash = blake3::hash(&bytes).to_hex().to_string();
                                        FileEvent::new(op, relative_path, Some(hash))
                                    }
                                    Err(e) => {
                                        if e.kind() == ErrorKind::NotFound {
                                            FileEvent::new(EventOp::Delete, relative_path, None)
                                        } else {
                                            eprintln!("Error reading file: {:?}", e);
                                            return;
                                        }
                                    }
                                }
                            }
                        };
                        let _ = tx2.send(file_event).await;
                    });
                },
                Err(e) => println!("Watch error: {:?}", e),
            }
        }, Config::default())?;

    file_watcher.watch(root.as_path(), RecursiveMode::Recursive)?;
    Ok(file_watcher)
}
