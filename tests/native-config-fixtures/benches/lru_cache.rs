use native_config_fixtures::platform::lru_cache::LruCache;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let mut cache = LruCache::new(256);
    let started = Instant::now();
    for index in 0..100_000u32 {
        let key = index % 512;
        cache.insert(key, key);
        black_box(cache.get(&key));
    }
    println!("lru_cache 100000 operations: {:?}", started.elapsed());
}
