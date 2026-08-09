use std::collections::HashMap;

pub use crate::utils::traits::StorageBackend;

pub struct PutOptions {
    pub content_type: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl Default for PutOptions {
    fn default() -> Self {
        Self {
            content_type: None,
            metadata: HashMap::new(),
        }
    }
}
