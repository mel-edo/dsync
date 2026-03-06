use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;
use tracing::debug;

const DEFAULT_IGNORES: &[&str] = &[
    "*.part",
    ".dsyncignore",
];

const DEFAULT_IGNORE_TEMPLATE: &str = r#"# dsyncignore - files and patterns listed here will not be synced
# Uses glob syntax. Lines starting with # are comments.

# Version control
.git
.git/**

# Dependencies
node_modules
node_modules/**

# OS files
.DS_Store

# Temporary files
*.tmp
*.part
"#;

pub struct IgnoreList {
    globset: GlobSet,
}

impl IgnoreList {
    pub fn load(sync_folder: &Path, extra_patterns: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();

        // always on defaults
        for pattern in DEFAULT_IGNORES {
            builder.add(Glob::new(pattern).unwrap());
        }

        // load .dsyncignore from sync folder
        let ignore_file = sync_folder.join(".dsyncignore");
        if ignore_file.exists() {
            match std::fs::read_to_string(&ignore_file) {
                Ok(contents) => {
                    for line in contents.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') { continue; }
                        match Glob::new(line) {
                            Ok(g) => { builder.add(g); }
                            Err(e) => debug!("Invalid pattern in .dsyncignore '{}': {}", line, e),
                        }
                    }
                    debug!("Loaded .dsyncignore from {:?}", ignore_file);
                }
                Err(e) => debug!("Failed to read .dsyncignore: {}", e),
            }
        } else {
            let _ = std::fs::write(&ignore_file, DEFAULT_IGNORE_TEMPLATE);
            debug!("Created default .dsyncignore at {:?}", ignore_file);
        }

        // CLI --exclude patterns
        for pattern in extra_patterns {
            match Glob::new(pattern) {
                Ok(g) => { builder.add(g); }
                Err(e) => debug!("Invalid --exclude pattern '{}': {}", pattern, e),
            }
        }

        Self {
            globset: builder.build().unwrap_or_else(|_| GlobSet::empty()),
        }
    }

    pub fn is_ignored(&self, rel_path: &Path) -> bool {
        // check the full path and each component
        if self.globset.is_match(rel_path) {
            return true;
        }
        for component in rel_path.components() {
            let c = Path::new(component.as_os_str());
            if self.globset.is_match(c) {
                return true;
            }
        }
        false
    }
}