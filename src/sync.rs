use std::{fs, path::Path, time::UNIX_EPOCH};
use walkdir::WalkDir;
use crate::protocol::FileInfo;

// scan the folder and return a list of all files with their hashes
pub fn generate_local_index(root: &Path) -> Vec<FileInfo> {
    let mut index = Vec::new();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "part" { continue; }
            }

            if let Ok(rel_path) = path.strip_prefix(root) {
                let rel_path_str = rel_path.to_string_lossy().replace("\\", "/");

                if let Ok(metadata) = fs::metadata(path) {
                    let size = metadata.len();
                    let modified = metadata.modified()
                        .unwrap_or(UNIX_EPOCH)
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    
                    let hash = "CHECK_METADATA".to_string();

                    index.push(FileInfo {
                        path: rel_path_str,
                        hash,
                        size,
                        modified,
                    });
                }
            }
        }
    }
    index
}

// compare local index vs remote index
pub fn calculate_diff(local: &[FileInfo], remote: &[FileInfo]) -> Vec<String> {
    let mut missing_files = Vec::new();

    for remote_file in remote {
        let match_found = local.iter().find(|f| f.path == remote_file.path);

        match match_found {
            Some(local_file) => {
                if local_file.size != remote_file.size || local_file.modified < remote_file.modified {
                    println!("Outdated: {} (Size: {} vs {}, Time: {} vs {})", remote_file.path, local_file.size, remote_file.size, local_file.modified, remote_file.modified); // optional log
                    missing_files.push(remote_file.path.clone());
                }
            },
            None => {
                println!("Missing: {}", remote_file.path); // optional log
                missing_files.push(remote_file.path.clone());
            }
        }
    }
    missing_files
}