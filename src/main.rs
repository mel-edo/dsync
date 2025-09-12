use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::watcher::watch_folder;

mod event;
mod network;
mod sync;
mod util;
mod watcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(100);
    let _watcher = watch_folder(PathBuf::from("./test_folder"), tx).await?;

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("Got FileEvent in main: {:?}", event);
        }
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3000)).await;
    }
}
