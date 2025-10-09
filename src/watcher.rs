use std::{path::PathBuf, time::SystemTime};
use tokio::sync::mpsc;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher, Error};
use crate::event::{FileEvent, EventOp};
use blake3;

pub async fn watch_folder(path: PathBuf, sender: mpsc::Sender<FileEvent>) -> notify::Result<RecommendedWatcher> {

    let tx = sender.clone();
    let handle = tokio::runtime::Handle::current();

    let mut file_watcher = RecommendedWatcher::new(
        move |res: Result<Event, Error>| {

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
                    let tx2 = tx.clone();

                    if event.paths.is_empty() {
                        return;
                    }

                    let file_event = if operation == EventOp::Delete {
                        FileEvent::new(operation, event.paths[0].clone(), None)
                    } else {
                        FileEvent::new(operation, event.paths[0].clone(),hash_file(&event.paths[0]))
                    };

                    handle.spawn(async move {
                        if let Err(e) = tx2.send(file_event).await {
                            eprintln!("Channel send error: {:?}", e);
                        }
                    });

                },
                Err(e) => println!("Watch error: {:?}", e),
            }

        }, Config::default())?;

    file_watcher.watch(&path, RecursiveMode::Recursive)?;

    Ok(file_watcher)
}

fn hash_file(path: &PathBuf) -> Option<String> {
    std::fs::read(path).ok().map(|data| blake3::hash(&data).to_hex().to_string())
}