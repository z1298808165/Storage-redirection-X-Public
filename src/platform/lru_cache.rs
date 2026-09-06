use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// 轻量有界 LRU 缓存，供路径与监控热路径复用。
pub struct LruCache<K, V> {
    capacity: usize,
    entries: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Eq + Hash + Clone, V> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let value = self.entries.get(key)?.clone();
        self.order.retain(|item| item != key);
        self.order.push_back(key.clone());
        Some(value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.entries.insert(key.clone(), value);
        self.order.retain(|item| item != &key);
        self.order.push_back(key);
        while self.entries.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}
