pub mod backend;
pub mod health;
pub mod local;

use std::collections::HashMap;
use std::sync::Arc;

use backend::StorageBackend;

pub struct StorageRegistry {
    backends: HashMap<String, Arc<dyn StorageBackend>>,
}

impl StorageRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: impl Into<String>, backend: Arc<dyn StorageBackend>) {
        self.backends.insert(name.into(), backend);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn StorageBackend>> {
        self.backends.get(name).cloned()
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.backends.remove(name).is_some()
    }

    pub fn list_backends(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }
}
