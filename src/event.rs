use std::{path::PathBuf, time::SystemTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum EventOp {
    Create,
    Modify,
    Delete
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FileEvent {
    operation: EventOp,
    file_path: PathBuf,
    hash: Option<String>,
    timestamp: SystemTime
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
        };

        let encoded = bincode::serialize(&event).unwrap();

        let decoded: FileEvent = bincode::deserialize(&encoded).unwrap();

        assert_eq!(event.operation, decoded.operation);
        assert_eq!(event.file_path, decoded.file_path);
        assert_eq!(event.hash, decoded.hash);
    }
}