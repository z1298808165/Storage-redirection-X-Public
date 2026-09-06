use native_config_fixtures::platform::lru_cache::LruCache;

#[test]
fn lru_cache_promotes_hits_and_evicts_oldest_entry() {
    let mut cache = LruCache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);
    assert_eq!(cache.get(&"a"), Some(1));
    cache.insert("c", 3);
    assert_eq!(cache.get(&"a"), Some(1));
    assert_eq!(cache.get(&"b"), None);
    assert_eq!(cache.get(&"c"), Some(3));
}

#[test]
fn lru_cache_reinsertion_does_not_consume_capacity() {
    let mut cache = LruCache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);
    cache.insert("a", 3);
    cache.insert("c", 4);
    assert_eq!(cache.get(&"a"), Some(3));
    assert_eq!(cache.get(&"b"), None);
    assert_eq!(cache.get(&"c"), Some(4));
}

#[test]
fn zero_capacity_cache_never_retains_entries() {
    let mut cache = LruCache::new(0);
    cache.insert("a", 1);
    assert_eq!(cache.get(&"a"), None);
}
