use std::{fs::{self, OpenOptions, create_dir_all}, io::Write, path::PathBuf};
use anyhow::{Context, Ok};

// ensure the parent directory exists for a given path
pub fn ensure_parent(path: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    Ok(())
}

// append chunk to .part file
pub fn write_chunk_atomic(root: &PathBuf, rel_path: &PathBuf, data: &[u8], is_last: bool) -> anyhow::Result<bool> {
    let full_path = root.join(rel_path);
    let part_path = full_path.with_extension("part");

    ensure_parent(&full_path)?;

    let mut options = OpenOptions::new();
    options.write(true).append(true).create(true);

    let mut file = options.open(&part_path)
        .with_context(|| format!("Failed to open part file: {:?}", part_path))?;
    
    file.write_all(data)?;

    if is_last {
        fs::rename(&part_path, &full_path)
            .with_context(|| format!("Failed to rename {:?} to {:?}", part_path, full_path))?;
        return Ok(true);
    }
    Ok(false)
}