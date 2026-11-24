//! Integration tests for `akku::cache`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use akku::cache::{Cache, EvictionReason, SyncCache};
use akku::time::{Duration as AkkuDuration, MockClock, OffsetDateTime};

fn make_clock() -> MockClock {
    let start = OffsetDateTime::from_unix_timestamp(1_700_000_000, 0).unwrap();
    MockClock::new(start)
}

#[test]
fn cache_basic_insert_get_remove_clear() {
    let cache = Cache::new(4);
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.capacity(), 4);

    assert_eq!(cache.insert("k1", 10), None);
    assert_eq!(cache.insert("k2", 20), None);
    assert_eq!(cache.len(), 2);
    assert!(!cache.is_empty());
    assert!(cache.contains_key(&"k1"));
    assert!(!cache.contains_key(&"k3"));

    assert_eq!(cache.get(&"k1"), Some(10));
    assert_eq!(cache.get(&"k2"), Some(20));
    assert_eq!(cache.get(&"missing"), None);

    // Overwrite returns previous value.
    assert_eq!(cache.insert("k1", 100), Some(10));
    assert_eq!(cache.get(&"k1"), Some(100));

    // Remove returns previous value and shrinks len.
    assert_eq!(cache.remove(&"k1"), Some(100));
    assert_eq!(cache.remove(&"k1"), None);
    assert_eq!(cache.len(), 1);

    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.get(&"k2"), None);
}

#[test]
fn cache_lru_eviction_order_o1() {
    let cache = Cache::new(3);
    cache.insert("a", 1);
    cache.insert("b", 2);
    cache.insert("c", 3);

    // Touch "a" so LRU order from MRU -> LRU is: a, c, b.
    assert_eq!(cache.get(&"a"), Some(1));

    // Inserting "d" must evict "b" (the tail).
    cache.insert("d", 4);
    assert_eq!(cache.get(&"b"), None);
    assert_eq!(cache.get(&"a"), Some(1));
    assert_eq!(cache.get(&"c"), Some(3));
    assert_eq!(cache.get(&"d"), Some(4));
}

#[test]
fn cache_arena_slot_recycling_across_many_evictions() {
    let cache = Cache::new(5);
    for i in 0..500 {
        cache.insert(i, i * 10);
        assert!(cache.len() <= 5);
    }
    assert_eq!(cache.len(), 5);
    for i in 495..500 {
        assert_eq!(cache.get(&i), Some(i * 10));
    }
    assert_eq!(cache.stats().evictions, 495);
}

#[test]
fn cache_peek_does_not_update_lru() {
    let cache = Cache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);

    // Peek at "a": returns Some(1) and counts as a hit, but leaves "a" as LRU tail.
    assert_eq!(cache.peek(&"a"), Some(1));
    assert_eq!(cache.stats().hits, 1);

    // Insert "c": "a" should be evicted, not "b".
    cache.insert("c", 3);
    assert_eq!(cache.peek(&"a"), None);
    assert_eq!(cache.peek(&"b"), Some(2));
    assert_eq!(cache.peek(&"c"), Some(3));
}

#[test]
fn cache_contains_key_does_not_promote_lru_or_alter_hits() {
    let cache = Cache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);

    assert!(cache.contains_key(&"a"));
    assert!(!cache.contains_key(&"z"));
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().misses, 0);

    // "a" should still be the LRU victim.
    cache.insert("c", 3);
    assert!(!cache.contains_key(&"a"));
    assert!(cache.contains_key(&"b"));
    assert!(cache.contains_key(&"c"));
}

#[test]
fn cache_update_existing_promotes_to_mru() {
    let cache = Cache::new(2);
    cache.insert("a", 1);
    cache.insert("b", 2);

    // Overwriting "a" promotes it to MRU, making "b" the LRU.
    assert_eq!(cache.insert("a", 10), Some(1));
    cache.insert("c", 3);

    assert_eq!(cache.get(&"b"), None);
    assert_eq!(cache.get(&"a"), Some(10));
    assert_eq!(cache.get(&"c"), Some(3));
}

#[test]
fn cache_default_ttl_expiration_with_mock_clock() {
    let clock = make_clock();
    let cache = Cache::builder(4)
        .clock(clock.clone())
        .default_ttl(Duration::from_secs(10))
        .build();

    cache.insert("k", "v");
    clock.advance(AkkuDuration::from_secs(9));
    assert_eq!(cache.get(&"k"), Some("v"));

    clock.advance(AkkuDuration::from_secs(2));
    assert_eq!(cache.get(&"k"), None);
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.stats().expirations, 1);
}

#[test]
fn cache_per_entry_ttl_overrides_default_ttl() {
    let clock = make_clock();
    let cache = Cache::builder(4)
        .clock(clock.clone())
        .default_ttl(Duration::from_secs(30))
        .build();

    cache.insert("default", 1);
    cache.insert_with_ttl("short", 2, Duration::from_secs(5));
    cache.insert_with_ttl("long", 3, Duration::from_secs(60));

    clock.advance(AkkuDuration::from_secs(6));
    assert_eq!(cache.get(&"short"), None);
    assert_eq!(cache.get(&"default"), Some(1));
    assert_eq!(cache.get(&"long"), Some(3));

    clock.advance(AkkuDuration::from_secs(25));
    assert_eq!(cache.get(&"default"), None);
    assert_eq!(cache.get(&"long"), Some(3));

    clock.advance(AkkuDuration::from_secs(30));
    assert_eq!(cache.get(&"long"), None);
}

#[test]
fn cache_zero_ttl_expires_immediately() {
    let clock = make_clock();
    let cache = Cache::builder(4).clock(clock).build();
    cache.insert_with_ttl("instant", 42, Duration::ZERO);
    assert_eq!(cache.get(&"instant"), None);
    assert_eq!(cache.stats().expirations, 1);
}

#[test]
fn cache_purge_expired_reaps_only_stale_entries() {
    let clock = make_clock();
    let cache = Cache::builder(10).clock(clock.clone()).build();

    cache.insert_with_ttl("t5", 1, Duration::from_secs(5));
    cache.insert_with_ttl("t10", 2, Duration::from_secs(10));
    cache.insert_with_ttl("t15", 3, Duration::from_secs(15));
    cache.insert_with_ttl("t20", 4, Duration::from_secs(20));
    cache.insert("forever", 5);

    clock.advance(AkkuDuration::from_secs(12));
    let purged = cache.purge_expired();
    assert_eq!(purged, 2);
    assert_eq!(cache.len(), 3);
    assert_eq!(cache.stats().expirations, 2);

    assert_eq!(cache.get(&"t5"), None);
    assert_eq!(cache.get(&"t10"), None);
    assert_eq!(cache.get(&"t15"), Some(3));
    assert_eq!(cache.get(&"t20"), Some(4));
    assert_eq!(cache.get(&"forever"), Some(5));
}

#[test]
fn cache_overwrite_expired_entry_returns_none_and_records_expiration() {
    let clock = make_clock();
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    let cache = Cache::builder(4)
        .clock(clock.clone())
        .on_evict(move |k, v, reason| {
            events_clone.lock().unwrap().push((*k, v, reason));
        })
        .build();

    cache.insert_with_ttl("k", 10, Duration::from_secs(5));
    clock.advance(AkkuDuration::from_secs(6));

    // Overwriting an already-expired entry returns None (not the stale 10) and emits Expired.
    assert_eq!(cache.insert("k", 20), None);
    assert_eq!(cache.get(&"k"), Some(20));
    assert_eq!(cache.stats().expirations, 1);
    assert_eq!(
        *events.lock().unwrap(),
        vec![("k", 10, EvictionReason::Expired)]
    );
}

#[test]
fn cache_remove_expired_entry_returns_none() {
    let clock = make_clock();
    let cache = Cache::builder(4).clock(clock.clone()).build();
    cache.insert_with_ttl("k", 99, Duration::from_secs(5));
    clock.advance(AkkuDuration::from_secs(10));

    assert_eq!(cache.remove(&"k"), None);
    assert_eq!(cache.stats().expirations, 1);
}

#[test]
fn cache_peek_expired_entry_evicts_and_returns_none() {
    let clock = make_clock();
    let cache = Cache::builder(4).clock(clock.clone()).build();
    cache.insert_with_ttl("k", 99, Duration::from_secs(5));
    clock.advance(AkkuDuration::from_secs(5));

    assert_eq!(cache.peek(&"k"), None);
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.stats().expirations, 1);
    assert_eq!(cache.stats().misses, 1);
}

#[test]
fn cache_get_or_insert_with_computes_only_on_miss_or_expired() {
    let clock = make_clock();
    let cache = Cache::builder(4)
        .clock(clock.clone())
        .default_ttl(Duration::from_secs(10))
        .build();
    let calls = AtomicUsize::new(0);

    let v1 = cache.get_or_insert_with("key", || {
        calls.fetch_add(1, Ordering::SeqCst);
        100
    });
    assert_eq!(v1, 100);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Second call is a hit: closure must not run.
    let v2 = cache.get_or_insert_with("key", || {
        calls.fetch_add(1, Ordering::SeqCst);
        200
    });
    assert_eq!(v2, 100);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Advance past TTL: closure must run again.
    clock.advance(AkkuDuration::from_secs(11));
    let v3 = cache.get_or_insert_with("key", || {
        calls.fetch_add(1, Ordering::SeqCst);
        300
    });
    assert_eq!(v3, 300);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn cache_get_or_try_insert_with_propagates_error_without_inserting() {
    let cache: Cache<&str, i32> = Cache::new(4);

    let res: Result<i32, &str> = cache.get_or_try_insert_with("fail", || Err("boom"));
    assert_eq!(res, Err("boom"));
    assert!(!cache.contains_key(&"fail"));
    assert_eq!(cache.len(), 0);

    let ok_res: Result<i32, &str> = cache.get_or_try_insert_with("fail", || Ok(77));
    assert_eq!(ok_res, Ok(77));
    assert_eq!(cache.get(&"fail"), Some(77));
}

#[test]
fn cache_eviction_hook_all_reasons() {
    let clock = make_clock();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log_clone = Arc::clone(&log);

    let cache = Cache::builder(2)
        .clock(clock.clone())
        .on_evict(move |k: &&str, v: i32, reason| {
            log_clone.lock().unwrap().push((*k, v, reason));
        })
        .build();

    cache.insert("a", 1);
    cache.insert("b", 2);
    // Capacity eviction of "a"
    cache.insert("c", 3);
    // Overwrite removal of "b"
    cache.insert("b", 20);
    // Explicit removal of "c"
    cache.remove(&"c");
    // TTL expiration of "t"
    cache.insert_with_ttl("t", 99, Duration::from_secs(3));
    clock.advance(AkkuDuration::from_secs(4));
    cache.purge_expired();
    // Clear removal of "b"
    cache.clear();

    let recorded = log.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            ("a", 1, EvictionReason::Capacity),
            ("b", 2, EvictionReason::Removed),
            ("c", 3, EvictionReason::Removed),
            ("t", 99, EvictionReason::Expired),
            ("b", 20, EvictionReason::Removed),
        ]
    );
}

#[test]
fn cache_on_evict_reentrancy_no_deadlock() {
    let counter = Arc::new(AtomicUsize::new(0));
    let slot: Arc<Mutex<Option<Cache<i32, i32>>>> = Arc::new(Mutex::new(None));
    let slot_for_hook = Arc::clone(&slot);
    let counter_for_hook = Arc::clone(&counter);

    let cache = Cache::builder(2)
        .on_evict(move |_k, _v, _reason| {
            counter_for_hook.fetch_add(1, Ordering::SeqCst);
            if let Some(c) = slot_for_hook.lock().unwrap().as_ref() {
                // Calling methods on the same Cache inside on_evict must not deadlock!
                let _ = c.len();
                let _ = c.stats();
                let _ = c.peek(&1);
            }
        })
        .build();

    *slot.lock().unwrap() = Some(cache.clone());

    cache.insert(1, 10);
    cache.insert(2, 20);
    cache.insert(3, 30); // triggers eviction of 1
    cache.remove(&2);
    cache.clear();

    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn cache_stats_and_hit_rate_accuracy() {
    let cache = Cache::new(2);
    assert_eq!(cache.stats().hit_rate(), 0.0);

    cache.insert("a", 1);
    cache.insert("b", 2);

    assert_eq!(cache.get(&"a"), Some(1)); // hit 1
    assert_eq!(cache.peek(&"b"), Some(2)); // hit 2
    assert_eq!(cache.get(&"c"), None); // miss 1
    assert_eq!(cache.peek(&"d"), None); // miss 2

    cache.insert("c", 3); // evicts "b" (since "a" was touched by get)

    let stats = cache.stats();
    assert_eq!(stats.hits, 2);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.evictions, 1);
    assert_eq!(stats.expirations, 0);
    assert_eq!(stats.len, 2);
    assert_eq!(stats.capacity, 2);
    assert!((stats.hit_rate() - 0.5).abs() < 1e-9);
}

#[test]
fn cache_capacity_one_edge_case() {
    let clock = make_clock();
    let cache = Cache::builder(1).clock(clock.clone()).build();

    assert_eq!(cache.insert("a", 1), None);
    assert_eq!(cache.get(&"a"), Some(1));
    assert_eq!(cache.insert("a", 2), Some(1));
    assert_eq!(cache.get(&"a"), Some(2));

    // Inserting "b" evicts "a".
    assert_eq!(cache.insert_with_ttl("b", 3, Duration::from_secs(5)), None);
    assert_eq!(cache.get(&"a"), None);
    assert_eq!(cache.get(&"b"), Some(3));
    assert_eq!(cache.len(), 1);

    clock.advance(AkkuDuration::from_secs(6));
    assert_eq!(cache.get(&"b"), None);
    assert_eq!(cache.len(), 0);
}

#[test]
fn cache_weight_limit_evicts_multiple_lru_entries() {
    let evicted = Arc::new(Mutex::new(Vec::new()));
    let evicted_clone = Arc::clone(&evicted);

    let cache = Cache::builder(100)
        .max_weight(10)
        .weigher(|_k: &&str, v: &String| v.len() as u64)
        .on_evict(move |k, _v, reason| {
            evicted_clone.lock().unwrap().push((*k, reason));
        })
        .build();

    cache.insert("k1", "1234".to_string()); // weight 4
    cache.insert("k2", "1234".to_string()); // weight 4 (total 8)
    assert_eq!(cache.weight(), 8);
    assert_eq!(cache.max_weight(), Some(10));

    // Touch "k1" so "k2" becomes LRU.
    assert_eq!(cache.get(&"k1").as_deref(), Some("1234"));

    // Insert "k3" with weight 5: 8 + 5 > 10, so "k2" (weight 4) is evicted -> 4 + 5 = 9 <= 10.
    cache.insert("k3", "12345".to_string());
    assert_eq!(cache.weight(), 9);
    assert_eq!(cache.len(), 2);
    assert!(cache.contains_key(&"k1"));
    assert!(!cache.contains_key(&"k2"));
    assert!(cache.contains_key(&"k3"));

    // Insert "k4" with weight 8: must evict both "k1" (LRU, weight 4) and "k3" (weight 5) to fit 8 <= 10.
    cache.insert("k4", "12345678".to_string());
    assert_eq!(cache.weight(), 8);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&"k4"));
}

#[test]
fn cache_oversized_single_entry_exceeding_max_weight_is_rejected() {
    let evicted = Arc::new(Mutex::new(Vec::new()));
    let evicted_clone = Arc::clone(&evicted);

    let cache = Cache::builder(10)
        .max_weight(5)
        .weigher(|_k: &&str, v: &Vec<u8>| v.len() as u64)
        .on_evict(move |k, _v, reason| {
            evicted_clone.lock().unwrap().push((*k, reason));
        })
        .build();

    cache.insert("small", vec![1, 2, 3]);
    assert_eq!(cache.len(), 1);

    // Oversized item (weight 8 > max_weight 5) is immediately evicted without evicting "small".
    cache.insert("huge", vec![0; 8]);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&"small"));
    assert!(!cache.contains_key(&"huge"));
    assert_eq!(
        *evicted.lock().unwrap(),
        vec![("huge", EvictionReason::Capacity)]
    );
}

#[test]
fn cache_weight_updated_correctly_on_overwrite_and_remove() {
    let cache = Cache::builder(10)
        .max_weight(100)
        .weigher(|_k: &&str, v: &u64| *v)
        .build();

    cache.insert("a", 15);
    cache.insert("b", 25);
    assert_eq!(cache.weight(), 40);

    // Overwrite "a" with weight 30 -> total weight 55.
    cache.insert("a", 30);
    assert_eq!(cache.weight(), 55);

    // Remove "b" -> total weight 30.
    cache.remove(&"b");
    assert_eq!(cache.weight(), 30);

    cache.clear();
    assert_eq!(cache.weight(), 0);
}

#[test]
fn sync_cache_wrapper_full_api() {
    let clock = make_clock();
    let sync_cache = SyncCache::builder(3)
        .clock(clock.clone())
        .default_ttl(Duration::from_secs(20))
        .build_sync();

    assert!(sync_cache.is_empty());
    assert_eq!(sync_cache.capacity(), 3);

    sync_cache.insert("a", 1);
    sync_cache.insert_with_ttl("b", 2, Duration::from_secs(5));
    assert_eq!(sync_cache.get_or_insert_with("c", || 3), 3);
    assert_eq!(
        sync_cache.get_or_try_insert_with::<_, ()>("c", || Ok(99)),
        Ok(3)
    );

    assert_eq!(sync_cache.len(), 3);
    assert_eq!(sync_cache.weight(), 3);
    assert!(sync_cache.contains_key(&"a"));
    assert_eq!(sync_cache.peek(&"a"), Some(1));
    assert_eq!(sync_cache.get(&"a"), Some(1));

    clock.advance(AkkuDuration::from_secs(6));
    assert_eq!(sync_cache.purge_expired(), 1);
    assert_eq!(sync_cache.len(), 2);

    assert_eq!(sync_cache.remove(&"a"), Some(1));
    sync_cache.clear();
    assert!(sync_cache.is_empty());
    assert!(sync_cache.as_cache().is_empty());
}

#[test]
fn cache_concurrent_multithread_stress() {
    let clock = make_clock();
    let cache = SyncCache::builder(64)
        .clock(clock.clone())
        .default_ttl(Duration::from_secs(30))
        .build_sync();

    let mut handles = Vec::new();
    for tid in 0..8u64 {
        let c = cache.clone();
        let clk = clock.clone();
        handles.push(thread::spawn(move || {
            for i in 0..300u64 {
                let key = (tid * 17 + i) % 96;
                match i % 6 {
                    0 => {
                        c.insert(key, i);
                    }
                    1 => {
                        c.insert_with_ttl(key, i, Duration::from_secs(10));
                    }
                    2 => {
                        let _ = c.get(&key);
                    }
                    3 => {
                        let _ = c.peek(&key);
                    }
                    4 => {
                        let _ = c.get_or_insert_with(key, || i * 2);
                    }
                    _ => {
                        if i % 30 == 0 {
                            clk.advance(AkkuDuration::from_secs(3));
                            let _ = c.purge_expired();
                        } else {
                            let _ = c.remove(&key);
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("worker thread panicked");
    }

    let stats = cache.stats();
    assert!(stats.len <= 64);
    assert_eq!(stats.len, cache.len());
    assert!(stats.hits + stats.misses > 0);
}

#[test]
fn cache_debug_formatting_and_from_conversion() {
    let c = Cache::new(4);
    c.insert("x", 1);
    let sc = SyncCache::from(c.clone());
    let sc2 = SyncCache::from_cache(c.clone());
    assert!(format!("{c:?}").contains("Cache"));
    assert!(format!("{sc:?}").contains("SyncCache"));
    assert_eq!(sc2.get(&"x"), Some(1));
}

#[test]
#[should_panic(expected = "Cache capacity must be greater than zero")]
fn cache_zero_capacity_panics() {
    let _ = Cache::<String, String>::new(0);
}
