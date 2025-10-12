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
    data: Option<Vec<u8>>
}

impl FileEvent {
    pub fn new(operation: EventOp, file_path: PathBuf, hash: Option<String>, data: Option<Vec<u8>>) -> Self {
        Self {
            operation,
            file_path,
            hash,
            timestamp: SystemTime::now(),
            data
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode;

    #[test]
    fn test_event_roundtrip() {
        let event = FileEvent {
            operation: EventOp::Create,
            file_path: PathBuf::from("example.txt"),
            hash: Some("abc123".to_string()),
            timestamp: SystemTime::now(),
            data: Some(vec![1, 2, 3])
        };

        let encoded = bincode::serialize(&event).unwrap();

        let decoded: FileEvent = bincode::deserialize(&encoded).unwrap();

        assert_eq!(event.operation, decoded.operation);
        assert_eq!(event.file_path, decoded.file_path);
        assert_eq!(event.hash, decoded.hash);
    }
}