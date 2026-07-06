use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 200;

#[derive(Clone, Default)]
pub struct Feed {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl Feed {
    pub fn push(&self, line: impl Into<String>) {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.len() == CAPACITY {
                guard.pop_front();
            }
            guard.push_back(line.into());
        }
    }

    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|guard| guard.iter().cloned().collect())
            .unwrap_or_default()
    }
}
