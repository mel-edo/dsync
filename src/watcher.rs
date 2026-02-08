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
                        notify::EventKind::Create(_) => {EventOp::Create},
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_)) => {EventOp::Modify},
                        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {EventOp::Modify},
                        notify::EventKind::Remove(_) => {EventOp::Delete},
                        _ => {return;},
                    };

                    // println!("Got event: {:?}", event);
                    if event.paths.is_empty() {
                        return;
                    }

                    let absolute_path = &event.paths[0];

                    let canonical_root = match root.canonicalize() {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Failed to canonicalize root path {:?}: {:?}", root, e);
                            return;
                        }
                    };

                    let canonical_absolute = match absolute_path.canonicalize() {
                        Ok(p) => p,
                        Err(_) => {
                            absolute_path.clone()
                        }
                    };

                    let relative_path = match diff_paths(&canonical_absolute, &canonical_root) {
                        Some(p) => p,
                        None => {
                            eprintln!("Could not compute relative path for {:?}", absolute_path);
                            return;
                        }
                    };

                    let tx2 = tx.clone();
                    let rel_path = relative_path.clone();
                    let abs_path = canonical_absolute.clone();
                    let op = operation.clone();

                    handle.spawn(async move {

                        // FIX: some files end up being 0 bytes so waiting 200ms
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        let file_event = match op {
                            EventOp::Delete => FileEvent::new(EventOp::Delete, rel_path.clone(), None, None),
                            _ => match fs::read(&abs_path).await {
                                Ok(bytes) => {
                                    let hash = blake3::hash(&bytes).to_hex().to_string();
                                    FileEvent::new(op, rel_path.clone(), Some(hash), Some(bytes))
                                }
                                Err(err) if err.kind() == ErrorKind::NotFound => {
                                    FileEvent::new(EventOp::Delete, rel_path.clone(), None, None)
                                }
                                Err(err) => {
                                    eprintln!("Failed to read file {:?}: {:?}", abs_path, err);
                                    return;
                                }
                            },
                        };

                        if let Err(e) = tx2.send(file_event).await {
                            eprintln!("Channel send error: {:?}", e);
                        }
                    });

                },
                Err(e) => println!("Watch error: {:?}", e),
            }

        }, Config::default())?;

    file_watcher.watch(root.as_path(), RecursiveMode::Recursive)?;

    Ok(file_watcher)
}
