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
    data: Option<Vec<u8>>,
    origin_id: Option<String>,
}

impl FileEvent {
    pub fn new(operation: EventOp, file_path: PathBuf, hash: Option<String>, data: Option<Vec<u8>>) -> Self {
        Self {
            operation,
            file_path,
            hash,
            timestamp: SystemTime::now(),
            data,
            origin_id: None,
        }
    }

    pub fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
    pub fn operation(&self) -> &EventOp {
        &self.operation
    }
    // pub fn hash(&self) -> Option<&String> {
        // self.hash.as_ref()
    // }
    pub fn data(&self) -> &Option<Vec<u8>> {
        &self.data
    }
    pub fn timestamp(&self) -> &SystemTime {
        &self.timestamp
    }
    pub fn with_origin(mut self, origin_id: String) -> Self {
        self.origin_id = Some(origin_id);
        self
    }
    pub fn origin_id(&self) -> Option<&String> {
        self.origin_id.as_ref()
    }
}
