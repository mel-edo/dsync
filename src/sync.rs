use std::{fs, path::Path};
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

                if let Ok(contents) = fs::read(path) {
                    let hash = blake3::hash(&contents).to_string();

                    index.push(FileInfo {
                        path: rel_path_str,
                        hash,
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
                if local_file.hash != remote_file.hash {
                    println!("Outdated: {}", remote_file.path); // optional log
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