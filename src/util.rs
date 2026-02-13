use std::{fs::create_dir_all, path::PathBuf};

// ensure the parent directory exists for a given path
pub fn ensure_parent(path: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    Ok(())
}