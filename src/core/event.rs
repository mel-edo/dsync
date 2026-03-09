use std::{path::PathBuf, time::SystemTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventOp {
    Create,
    Modify,
    Delete
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEvent {
    operation: EventOp,
    file_path: PathBuf,
    hash: Option<String>,
    timestamp: SystemTime,
    origin_id: Option<String>,
}

impl FileEvent {
    pub fn new(operation: EventOp, file_path: PathBuf, hash: Option<String>) -> Self {
        Self {
            operation,
            file_path,
            hash,
            timestamp: SystemTime::now(),
            origin_id: None,
        }
    }

    pub fn file_path(&self) -> &PathBuf { &self.file_path }
    pub fn operation(&self) -> &EventOp { &self.operation }
    // pub fn timestamp(&self) -> &SystemTime { &self.timestamp }
    
    pub fn with_origin(mut self, origin_id: String) -> Self {
        self.origin_id = Some(origin_id);
        self
    }
    pub fn origin_id(&self) -> Option<&String> { self.origin_id.as_ref() }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileChunk {
    pub rel_path: PathBuf,
    pub offset: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
    pub compressed: bool,
}
