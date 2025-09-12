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
                    match event.kind {
                        notify::EventKind::Create(_) => {},
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_)) => {},
                        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {},
                        notify::EventKind::Remove(_) => {},
                        _ => {},
                    }

                    println!("Got event: {:?}", event);
                    let tx2 = tx.clone();

                    handle.spawn(async move {
                        // later map 'event' -> file event and send it
                        // loggging for now
                        println!("Would send event via channel: {:?}", event);
                        // tx2.send(my_file_event).await.unwrap();
                    });

                },
                Err(e) => println!("Watch error: {:?}", e),
            }

        }, Config::default())?;

    file_watcher.watch(&path, RecursiveMode::Recursive)?;

    Ok(file_watcher)
}
