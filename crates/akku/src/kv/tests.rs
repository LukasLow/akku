//! Integration tests for `akku::kv`.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Cursor, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use akku::fs::TempDir;
use akku::kv::file::WAL_MAGIC;
use akku::kv::redis::{RedisClient, RespLimits, RespValue};
use akku::kv::{BatchOp, FileStore, KvError, KvLimits, KvStore, MemoryStore};
use akku::time::{Duration as AkkuDuration, MockClock, OffsetDateTime};

fn make_clock() -> MockClock {
    let start = OffsetDateTime::from_unix_timestamp(1_700_000_000, 0).unwrap();
    MockClock::new(start)
}

#[test]
fn memory_store_crud_and_batch() {
    let store = MemoryStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);

    store.set("k1", b"v1").unwrap();
    store.set("k2", b"v2").unwrap();
    assert_eq!(store.len(), 2);
    assert!(!store.is_empty());
    assert!(store.exists("k1").unwrap());
    assert!(!store.exists("k3").unwrap());

    assert_eq!(store.get("k1").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(store.get("missing").unwrap(), None);

    assert!(store.delete("k1").unwrap());
    assert!(!store.delete("k1").unwrap());
    assert_eq!(store.len(), 1);

    store
        .batch(&[
            BatchOp::set("b1", b"val1".to_vec()),
            BatchOp::set("b2", b"val2".to_vec()),
            BatchOp::delete("k2"),
        ])
        .unwrap();

    assert_eq!(store.get("k2").unwrap(), None);
    assert_eq!(store.get("b1").unwrap(), Some(b"val1".to_vec()));
    assert_eq!(store.get("b2").unwrap(), Some(b"val2".to_vec()));

    store.clear();
    assert!(store.is_empty());
}

#[test]
fn memory_store_ttl_with_mock_clock() {
    let clock = make_clock();
    let store = MemoryStore::with_clock(clock.clone());

    store
        .set_ex("ephemeral", b"secret", Duration::from_secs(10))
        .unwrap();
    store.set("persistent", b"forever").unwrap();

    clock.advance(AkkuDuration::from_secs(9));
    assert!(store.exists("ephemeral").unwrap());
    assert_eq!(store.get("ephemeral").unwrap(), Some(b"secret".to_vec()));

    clock.advance(AkkuDuration::from_secs(2));
    assert!(!store.exists("ephemeral").unwrap());
    assert_eq!(store.get("ephemeral").unwrap(), None);
    assert_eq!(store.get("persistent").unwrap(), Some(b"forever".to_vec()));

    // Overwriting a TTL key with `set` clears its TTL.
    store
        .set_ex("reset_me", b"v1", Duration::from_secs(5))
        .unwrap();
    store.set("reset_me", b"v2").unwrap();
    clock.advance(AkkuDuration::from_secs(20));
    assert_eq!(store.get("reset_me").unwrap(), Some(b"v2".to_vec()));
}

#[test]
fn memory_store_purge_expired() {
    let clock = make_clock();
    let store = MemoryStore::with_clock(clock.clone());

    store.set_ex("a", b"1", Duration::from_secs(5)).unwrap();
    store.set_ex("b", b"2", Duration::from_secs(10)).unwrap();
    store.set_ex("c", b"3", Duration::from_secs(20)).unwrap();

    clock.advance(AkkuDuration::from_secs(12));
    assert_eq!(store.purge_expired(), 2);
    assert_eq!(store.len(), 1);
    assert_eq!(store.get("c").unwrap(), Some(b"3".to_vec()));
}

#[test]
fn memory_store_scan_prefix_and_scan_range() {
    let clock = make_clock();
    let store = MemoryStore::with_clock(clock.clone());

    store.set("app:3", b"three").unwrap();
    store.set("app:1", b"one").unwrap();
    store
        .set_ex("app:2", b"two", Duration::from_secs(5))
        .unwrap();
    store.set("appe:1", b"other").unwrap();
    store.set("agn:1", b"before").unwrap();

    clock.advance(AkkuDuration::from_secs(6));

    let prefix_rows = store.scan_prefix("app:").unwrap();
    assert_eq!(
        prefix_rows,
        vec![
            ("app:1".to_string(), b"one".to_vec()),
            ("app:3".to_string(), b"three".to_vec()),
        ]
    );

    let range_rows = store.scan_range("agn:1", "app:3").unwrap();
    assert_eq!(
        range_rows,
        vec![
            ("agn:1".to_string(), b"before".to_vec()),
            ("app:1".to_string(), b"one".to_vec()),
        ]
    );

    // Inverted range returns empty Vec.
    assert!(store.scan_range("z", "a").unwrap().is_empty());
}

#[test]
fn memory_store_dos_key_and_value_size_limits() {
    let store = MemoryStore::new().with_limits(KvLimits::new(8, 16));
    assert_eq!(store.limits(), KvLimits::new(8, 16));

    // Empty key is rejected.
    assert!(matches!(
        store.set("", b"val"),
        Err(KvError::InvalidInput(_))
    ));

    // Oversized key (9 > 8) is rejected.
    assert!(matches!(
        store.set("123456789", b"val"),
        Err(KvError::InvalidInput(_))
    ));

    // Oversized value (17 > 16) is rejected.
    assert!(matches!(
        store.set("validkey", &[0u8; 17]),
        Err(KvError::InvalidInput(_))
    ));

    // Batch with an invalid item fails atomically without applying earlier items.
    let res = store.batch(&[
        BatchOp::set("ok", b"123".to_vec()),
        BatchOp::set("too_long_key", b"123".to_vec()),
    ]);
    assert!(matches!(res, Err(KvError::InvalidInput(_))));
    assert_eq!(store.get("ok").unwrap(), None);
}

#[test]
fn file_store_persist_and_reopen() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("test.wal");

    {
        let store = FileStore::open(&wal_path).unwrap();
        assert_eq!(store.path(), wal_path.as_path());
        assert!(store.is_empty().unwrap());

        store.set("user:1", b"Alice").unwrap();
        store.set("user:2", b"Bob").unwrap();
        store.set("user:1", b"Alice_v2").unwrap();
        store
            .batch(&[
                BatchOp::set("user:3", b"Charlie".to_vec()),
                BatchOp::delete("user:2"),
            ])
            .unwrap();
        store.sync().unwrap();
        assert_eq!(store.len().unwrap(), 2);
    }

    // Reopen and verify WAL replay restored exact state.
    let reopened = FileStore::open(&wal_path).unwrap();
    assert_eq!(reopened.len().unwrap(), 2);
    assert_eq!(reopened.get("user:1").unwrap(), Some(b"Alice_v2".to_vec()));
    assert_eq!(reopened.get("user:2").unwrap(), None);
    assert_eq!(reopened.get("user:3").unwrap(), Some(b"Charlie".to_vec()));
}

#[test]
fn file_store_ttl_persistence_across_reopen() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("ttl.wal");
    let clock = make_clock();

    {
        let store = FileStore::open_with_clock(&wal_path, clock.clone()).unwrap();
        store
            .set_ex("short", b"10s", Duration::from_secs(10))
            .unwrap();
        store
            .set_ex("long", b"60s", Duration::from_secs(60))
            .unwrap();
        store.set("forever", b"inf").unwrap();
    }

    // Advance clock 15s and reopen: "short" is already expired and discarded on replay.
    clock.advance(AkkuDuration::from_secs(15));
    let reopened = FileStore::open_with_clock(&wal_path, clock.clone()).unwrap();
    assert_eq!(reopened.get("short").unwrap(), None);
    assert_eq!(reopened.get("long").unwrap(), Some(b"60s".to_vec()));
    assert_eq!(reopened.get("forever").unwrap(), Some(b"inf".to_vec()));

    // Advance another 50s while open: "long" expires lazily.
    clock.advance(AkkuDuration::from_secs(50));
    assert_eq!(reopened.get("long").unwrap(), None);
    assert!(!reopened.exists("long").unwrap());
    assert_eq!(reopened.len().unwrap(), 1);
}

#[test]
fn file_store_crash_recovery_truncated_wal_tail_header() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("crash_hdr.wal");

    let valid_size = {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("k1", b"v1").unwrap();
        store.set("k2", b"v2").unwrap();
        store.disk_size().unwrap()
    };

    // Simulate power loss after writing only 7 bytes of a new 21-byte frame header.
    {
        let mut f = OpenOptions::new().append(true).open(&wal_path).unwrap();
        f.write_all(&[0x12, 0x34, 0x56, 0x78, 1, 0, 0]).unwrap();
        f.sync_all().unwrap();
    }
    assert_eq!(fs::metadata(&wal_path).unwrap().len(), valid_size + 7);

    // Reopen must automatically truncate the torn tail back to `valid_size` and recover k1 & k2.
    let recovered = FileStore::open(&wal_path).unwrap();
    assert_eq!(recovered.disk_size().unwrap(), valid_size);
    assert_eq!(recovered.get("k1").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(recovered.get("k2").unwrap(), Some(b"v2".to_vec()));

    // Subsequent writes after recovery must succeed cleanly.
    recovered.set("k3", b"v3").unwrap();
    drop(recovered);

    let final_store = FileStore::open(&wal_path).unwrap();
    assert_eq!(final_store.get("k3").unwrap(), Some(b"v3".to_vec()));
}

#[test]
fn file_store_crash_recovery_truncated_wal_tail_payload() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("crash_payload.wal");

    {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("good_key", b"good_value").unwrap();
        store
            .set("torn_key", b"complete_payload_that_gets_chopped")
            .unwrap();
    }

    // Truncate the last 10 bytes of the second record's value payload to simulate an interrupted write.
    let full_len = fs::metadata(&wal_path).unwrap().len();
    {
        let f = OpenOptions::new().write(true).open(&wal_path).unwrap();
        f.set_len(full_len - 10).unwrap();
        f.sync_all().unwrap();
    }

    let recovered = FileStore::open(&wal_path).unwrap();
    assert_eq!(
        recovered.get("good_key").unwrap(),
        Some(b"good_value".to_vec())
    );
    assert_eq!(recovered.get("torn_key").unwrap(), None);
    assert_eq!(recovered.len().unwrap(), 1);
}

#[test]
fn file_store_corrupt_crc32_detection() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("corrupt.wal");

    {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("alpha", b"hello_world").unwrap();
        store.set("beta", b"second_record").unwrap();
    }

    // Flip one bit inside the first record's value payload without changing file length.
    {
        let mut bytes = fs::read(&wal_path).unwrap();
        // Header (8) + FrameHeader (21) + key "alpha" (5) = offset 34 is first byte of "hello_world".
        bytes[34] ^= 0xFF;
        fs::write(&wal_path, &bytes).unwrap();
    }

    let res = FileStore::open(&wal_path);
    assert!(
        matches!(res, Err(KvError::CorruptData(ref msg)) if msg.contains("CRC32 mismatch")),
        "expected CorruptData(CRC32 mismatch), got {res:?}"
    );
}

#[test]
fn file_store_invalid_magic_header_rejected() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("bad_magic.wal");
    fs::write(&wal_path, b"NOTAKKU!12345678").unwrap();

    let res = FileStore::open(&wal_path);
    assert!(matches!(res, Err(KvError::CorruptData(_))));

    // Also test a file shorter than 8 bytes (e.g. 4 bytes).
    let short_path = dir.as_path().join("short_magic.wal");
    fs::write(&short_path, b"AKKU").unwrap();
    assert!(matches!(
        FileStore::open(&short_path),
        Err(KvError::CorruptData(_))
    ));
}

#[test]
fn file_store_unknown_op_tag_rejected() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("bad_op.wal");

    // A frame with invalid op_tag = 99 followed by a valid record is mid-log corruption.
    {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("a", b"1").unwrap();
        store.set("b", b"2").unwrap();
    }
    let mut bytes = fs::read(&wal_path).unwrap();
    bytes[WAL_MAGIC.len() + 4] = 99;
    fs::write(&wal_path, &bytes).unwrap();

    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::CorruptData(ref msg)) if msg.contains("unknown operation tag")
    ));
    // The file must not have been truncated by the failed open.
    assert_eq!(fs::read(&wal_path).unwrap(), bytes);
}

/// Builds a WAL with three records and returns `(path, bytes, record_offsets)`.
fn three_record_wal(dir: &TempDir, name: &str) -> (std::path::PathBuf, Vec<u8>, Vec<usize>) {
    let wal_path = dir.as_path().join(name);
    let mut offsets = Vec::new();
    {
        let store = FileStore::open(&wal_path).unwrap();
        for (k, v) in [
            ("k1", "value-one"),
            ("k2", "value-two"),
            ("k3", "value-three"),
        ] {
            offsets.push(store.disk_size().unwrap() as usize);
            store.set(k, v.as_bytes()).unwrap();
        }
    }
    let bytes = fs::read(&wal_path).unwrap();
    (wal_path, bytes, offsets)
}

#[test]
fn file_store_zero_filled_crash_tail_recovers() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, _) = three_record_wal(&dir, "zero_tail.wal");

    // Simulate a preallocated / zero-filled tail after a crash (larger than one frame header).
    let mut f = OpenOptions::new().append(true).open(&wal_path).unwrap();
    f.write_all(&vec![0u8; 4096]).unwrap();
    f.sync_all().unwrap();
    drop(f);

    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.disk_size().unwrap(), bytes.len() as u64);
    assert_eq!(store.get("k3").unwrap(), Some(b"value-three".to_vec()));
    assert_eq!(store.len().unwrap(), 3);
    store.set("k4", b"after").unwrap();
    drop(store);

    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.get("k4").unwrap(), Some(b"after".to_vec()));
    assert_eq!(store.len().unwrap(), 4);
}

#[test]
fn file_store_zero_filled_short_tail_and_zeroed_last_record_recover() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, offsets) = three_record_wal(&dir, "zero_short.wal");

    // A few zero bytes (shorter than a frame header).
    let mut short = bytes.clone();
    short.extend_from_slice(&[0u8; 5]);
    fs::write(&wal_path, &short).unwrap();
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.len().unwrap(), 3);
    drop(store);

    // The last record's region zeroed out (data never reached disk): recovers k1 and k2.
    let mut zeroed = bytes.clone();
    for b in &mut zeroed[offsets[2]..] {
        *b = 0;
    }
    fs::write(&wal_path, &zeroed).unwrap();
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.disk_size().unwrap(), offsets[2] as u64);
    assert_eq!(store.get("k2").unwrap(), Some(b"value-two".to_vec()));
    assert_eq!(store.get("k3").unwrap(), None);
}

#[test]
fn file_store_torn_last_record_garbage_recovers() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, mut bytes, offsets) = three_record_wal(&dir, "torn_garbage.wal");

    // Garble the payload of the final record (torn sector): CRC fails, nothing valid follows.
    let last = bytes.len() - 1;
    bytes[last] ^= 0x5A;
    fs::write(&wal_path, &bytes).unwrap();

    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.disk_size().unwrap(), offsets[2] as u64);
    assert_eq!(store.get("k1").unwrap(), Some(b"value-one".to_vec()));
    assert_eq!(store.get("k2").unwrap(), Some(b"value-two".to_vec()));
    assert_eq!(store.get("k3").unwrap(), None);
}

#[test]
fn file_store_corrupt_val_len_in_middle_is_error_not_silent_loss() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, offsets) = three_record_wal(&dir, "mid_val_len.wal");
    let val_len_at = offsets[1] + 17;

    // Case 1: val_len inflated past EOF on the middle record; k3 follows and is valid.
    let mut inflated = bytes.clone();
    inflated[val_len_at..val_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
    fs::write(&wal_path, &inflated).unwrap();
    let res = FileStore::open(&wal_path);
    assert!(
        matches!(res, Err(KvError::CorruptData(ref m)) if m.contains("valid record follows")),
        "expected mid-log corruption error, got {res:?}"
    );
    assert_eq!(
        fs::read(&wal_path).unwrap(),
        inflated,
        "file must be untouched"
    );

    // Case 2: val_len shrunk (still in-bounds): CRC fails, k3 follows -> error.
    let mut shrunk = bytes.clone();
    shrunk[val_len_at..val_len_at + 4].copy_from_slice(&2u32.to_le_bytes());
    fs::write(&wal_path, &shrunk).unwrap();
    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::CorruptData(ref m)) if m.contains("CRC32 mismatch")
    ));

    // Case 3: val_len = u32::MAX must not allocate 4 GiB; it is bounded by the file size.
    let mut huge = bytes.clone();
    huge[val_len_at..val_len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&wal_path, &huge).unwrap();
    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::CorruptData(_))
    ));

    // Case 4: the same inflated val_len on the *last* record is a torn tail -> recovered.
    let mut tail = bytes.clone();
    let last_len_at = offsets[2] + 17;
    tail[last_len_at..last_len_at + 4].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
    fs::write(&wal_path, &tail).unwrap();
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.len().unwrap(), 2);
    assert_eq!(store.disk_size().unwrap(), offsets[2] as u64);
}

#[test]
fn file_store_zero_gap_before_valid_record_is_error() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, offsets) = three_record_wal(&dir, "zero_gap.wal");

    // Zero out the middle record only: valid k3 still follows -> must not truncate.
    let mut gapped = bytes.clone();
    for b in &mut gapped[offsets[1]..offsets[2]] {
        *b = 0;
    }
    fs::write(&wal_path, &gapped).unwrap();
    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::CorruptData(_))
    ));
    assert_eq!(fs::read(&wal_path).unwrap(), gapped);
}

#[test]
fn file_store_large_garbage_tail_is_refused_not_truncated() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("big_garbage.wal");
    {
        let store =
            FileStore::open_with_limits(&wal_path, make_clock(), KvLimits::new(16, 64)).unwrap();
        store.set("k", b"v").unwrap();
    }
    // Non-zero garbage far larger than any single record can be under these limits.
    let mut f = OpenOptions::new().append(true).open(&wal_path).unwrap();
    f.write_all(&vec![0xAB; 3 * 1024 * 1024]).unwrap();
    drop(f);
    let before = fs::metadata(&wal_path).unwrap().len();

    let res = FileStore::open_with_limits(&wal_path, make_clock(), KvLimits::new(16, 64));
    assert!(matches!(res, Err(KvError::CorruptData(_))), "got {res:?}");
    assert_eq!(fs::metadata(&wal_path).unwrap().len(), before);
}

#[test]
fn file_store_atomic_compaction_shrinks_wal_and_preserves_live_keys() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("compact.wal");

    let store = FileStore::open(&wal_path).unwrap();
    for round in 0..40u32 {
        for k in 0..10u32 {
            let key = format!("key:{k:02}");
            let val = format!("round-{round}-value-{k}");
            store.set(&key, val.as_bytes()).unwrap();
        }
    }
    // Delete half the keys.
    for k in 0..5u32 {
        let key = format!("key:{k:02}");
        assert!(store.delete(&key).unwrap());
    }

    let uncompacted_size = store.disk_size().unwrap();
    store.compact().unwrap();
    let compacted_size = store.disk_size().unwrap();

    assert!(
        compacted_size < uncompacted_size / 10,
        "expected compacted size ({compacted_size}) << uncompacted size ({uncompacted_size})"
    );
    assert_eq!(store.len().unwrap(), 5);

    // Post-compaction writes and reopen must work seamlessly.
    store.set("post_compact", b"ok").unwrap();
    drop(store);

    let reopened = FileStore::open(&wal_path).unwrap();
    assert_eq!(reopened.len().unwrap(), 6);
    for k in 0..5u32 {
        assert_eq!(reopened.get(&format!("key:{k:02}")).unwrap(), None);
    }
    for k in 5..10u32 {
        let expected = format!("round-39-value-{k}").into_bytes();
        assert_eq!(
            reopened.get(&format!("key:{k:02}")).unwrap(),
            Some(expected)
        );
    }
    assert_eq!(reopened.get("post_compact").unwrap(), Some(b"ok".to_vec()));
}

#[test]
fn file_store_auto_compact_threshold_triggers_compaction() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("auto_compact.wal");

    let store = FileStore::open(&wal_path)
        .unwrap()
        .with_auto_compact_threshold(10);

    for i in 0..25u32 {
        store.set("hot_key", format!("val-{i}").as_bytes()).unwrap();
    }

    // With threshold 10, overwriting the same key 25 times triggers auto-compaction,
    // keeping the WAL file near a single-record size (< 200 bytes).
    assert!(store.disk_size().unwrap() < 250);
    assert_eq!(store.get("hot_key").unwrap(), Some(b"val-24".to_vec()));
}

#[test]
fn file_store_scan_prefix_and_scan_range() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("scan.wal");
    let store = FileStore::open(&wal_path).unwrap();

    store.set("item:b", b"2").unwrap();
    store.set("item:a", b"1").unwrap();
    store.set("item:c", b"3").unwrap();
    store.set("other:1", b"x").unwrap();

    let pref = store.scan_prefix("item:").unwrap();
    assert_eq!(
        pref,
        vec![
            ("item:a".to_string(), b"1".to_vec()),
            ("item:b".to_string(), b"2".to_vec()),
            ("item:c".to_string(), b"3".to_vec()),
        ]
    );

    let rng = store.scan_range("item:a", "item:c").unwrap();
    assert_eq!(
        rng,
        vec![
            ("item:a".to_string(), b"1".to_vec()),
            ("item:b".to_string(), b"2".to_vec()),
        ]
    );
}

#[test]
fn file_store_safe_path_and_dos_limits() {
    let dir = TempDir::new("akku-kv").unwrap();

    // Opening a directory path as FileStore is rejected with InvalidInput.
    assert!(matches!(
        FileStore::open(dir.as_path()),
        Err(KvError::InvalidInput(_))
    ));

    // Empty path is rejected.
    assert!(matches!(FileStore::open(""), Err(KvError::InvalidInput(_))));

    // Oversized key / value rejected by configured KvLimits.
    let wal_path = dir.as_path().join("limited.wal");
    let store = FileStore::open_with_limits(&wal_path, make_clock(), KvLimits::new(5, 10)).unwrap();
    assert_eq!(store.limits(), KvLimits::new(5, 10));

    assert!(matches!(
        store.set("123456", b"ok"),
        Err(KvError::InvalidInput(_))
    ));
    assert!(matches!(
        store.set("12345", &[b'x'; 11]),
        Err(KvError::InvalidInput(_))
    ));
}

#[test]
fn resp_value_roundtrip_all_variants() {
    let frames = vec![
        RespValue::SimpleString("OK".to_string()),
        RespValue::Error("ERR unknown command".to_string()),
        RespValue::Integer(0),
        RespValue::Integer(-42),
        RespValue::Integer(9_876_543_210),
        RespValue::BulkString(b"binary\r\n\0payload".to_vec()),
        RespValue::BulkString(Vec::new()),
        RespValue::Null,
        RespValue::Array(vec![
            RespValue::SimpleString("PONG".to_string()),
            RespValue::Integer(7),
            RespValue::Null,
            RespValue::Array(vec![RespValue::BulkString(b"nested".to_vec())]),
        ]),
    ];

    for frame in frames {
        let bytes = frame.to_bytes().unwrap();
        let mut cursor = Cursor::new(bytes);
        let decoded = RespValue::decode(&mut cursor).unwrap();
        assert_eq!(decoded, frame);
    }
}

#[test]
fn resp_value_resp3_interoperability() {
    // RESP3 Null `_\r\n`
    let mut r1 = Cursor::new(b"_\r\n".to_vec());
    assert_eq!(RespValue::decode(&mut r1).unwrap(), RespValue::Null);

    // RESP3 Booleans `#t\r\n` and `#f\r\n`
    let mut r2 = Cursor::new(b"#t\r\n".to_vec());
    assert_eq!(RespValue::decode(&mut r2).unwrap(), RespValue::Integer(1));
    let mut r3 = Cursor::new(b"#f\r\n".to_vec());
    assert_eq!(RespValue::decode(&mut r3).unwrap(), RespValue::Integer(0));

    // RESP3 Map `%2\r\n+k1\r\n:10\r\n+k2\r\n:20\r\n`
    let mut r4 = Cursor::new(b"%2\r\n+k1\r\n:10\r\n+k2\r\n:20\r\n".to_vec());
    assert_eq!(
        RespValue::decode(&mut r4).unwrap(),
        RespValue::Array(vec![
            RespValue::SimpleString("k1".to_string()),
            RespValue::Integer(10),
            RespValue::SimpleString("k2".to_string()),
            RespValue::Integer(20),
        ])
    );

    // RESP2 Null Array `*-1\r\n`
    let mut r5 = Cursor::new(b"*-1\r\n".to_vec());
    assert_eq!(RespValue::decode(&mut r5).unwrap(), RespValue::Null);
}

#[test]
fn resp_value_accessors_and_into_result() {
    let s = RespValue::SimpleString("hello".to_string());
    assert_eq!(s.as_str(), Some("hello"));
    assert_eq!(s.as_bytes(), Some(b"hello".as_slice()));
    assert_eq!(s.as_i64(), None);
    assert!(!s.is_null());

    let b = RespValue::BulkString(b"world".to_vec());
    assert_eq!(b.as_str(), Some("world"));
    assert_eq!(b.as_bytes(), Some(b"world".as_slice()));

    let i = RespValue::Integer(123);
    assert_eq!(i.as_i64(), Some(123));
    assert_eq!(i.as_str(), None);

    let n = RespValue::Null;
    assert!(n.is_null());

    let err = RespValue::Error("ERR nope".to_string());
    assert!(matches!(err.into_result(), Err(KvError::Server(_))));
}

#[test]
fn resp_dos_cap_max_depth_exceeded() {
    let limits = RespLimits {
        max_depth: 4,
        ..RespLimits::default()
    };

    // Build 6 nested arrays `*1\r\n*1\r\n...`
    let mut wire = Vec::new();
    for _ in 0..6 {
        wire.extend_from_slice(b"*1\r\n");
    }
    wire.extend_from_slice(b"+OK\r\n");

    let mut cursor = Cursor::new(wire);
    let res = RespValue::decode_with_limits(&mut cursor, limits);
    assert!(
        matches!(res, Err(KvError::Protocol(ref msg)) if msg.contains("max_depth")),
        "expected max_depth error, got {res:?}"
    );
}

#[test]
fn resp_dos_cap_max_bulk_and_array_size_exceeded() {
    let limits = RespLimits {
        max_bulk_size: 1024,
        max_array_len: 16,
        ..RespLimits::default()
    };

    // Oversized bulk header rejected before reading payload bytes.
    let mut c1 = Cursor::new(b"$100000000\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut c1, limits),
        Err(KvError::Protocol(ref msg)) if msg.contains("max_bulk_size")
    ));

    // Oversized array count rejected immediately.
    let mut c2 = Cursor::new(b"*100000\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut c2, limits),
        Err(KvError::Protocol(ref msg)) if msg.contains("max_array_len")
    ));
}

#[test]
fn resp_dos_cap_max_line_len_and_malformed_frames() {
    let limits = RespLimits {
        max_line_len: 32,
        ..RespLimits::default()
    };

    // Inline header exceeding 32 bytes.
    let mut long_line = vec![b'+'];
    long_line.extend_from_slice(&[b'A'; 40]);
    long_line.extend_from_slice(b"\r\n");
    let mut c1 = Cursor::new(long_line);
    assert!(matches!(
        RespValue::decode_with_limits(&mut c1, limits),
        Err(KvError::Protocol(ref msg)) if msg.contains("max_line_len")
    ));

    // Bulk string missing trailing CRLF.
    let mut c2 = Cursor::new(b"$3\r\nabcXX".to_vec());
    assert!(matches!(
        RespValue::decode(&mut c2),
        Err(KvError::Protocol(_))
    ));

    // SimpleString with embedded newline rejected on encode.
    assert!(matches!(
        RespValue::SimpleString("bad\nline".to_string()).to_bytes(),
        Err(KvError::Protocol(_))
    ));
}

/// Spawns a lightweight in-process mock Redis TCP server on `127.0.0.1:0` speaking RESP2.
fn spawn_mock_redis_server() -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::new(Mutex::new(BTreeMap::new()));

    let handle = thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;

            while let Ok(frame) = RespValue::decode(&mut reader) {
                let args: Vec<Vec<u8>> = match frame {
                    RespValue::Array(items) => items
                        .into_iter()
                        .filter_map(|i| i.as_bytes().map(|b| b.to_vec()))
                        .collect(),
                    _ => break,
                };
                if args.is_empty() {
                    break;
                }
                let cmd = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
                let reply = match cmd.as_str() {
                    "PING" => RespValue::SimpleString("PONG".to_string()),
                    "GET" if args.len() == 2 => {
                        let key = String::from_utf8_lossy(&args[1]).to_string();
                        match state.lock().unwrap().get(&key) {
                            Some(v) => RespValue::BulkString(v.clone()),
                            None => RespValue::Null,
                        }
                    }
                    "SET" if args.len() >= 3 => {
                        let key = String::from_utf8_lossy(&args[1]).to_string();
                        let val = args[2].clone();
                        state.lock().unwrap().insert(key, val);
                        RespValue::SimpleString("OK".to_string())
                    }
                    "DEL" if args.len() >= 2 => {
                        let mut removed = 0i64;
                        let mut map = state.lock().unwrap();
                        for k_bytes in &args[1..] {
                            let k = String::from_utf8_lossy(k_bytes).to_string();
                            if map.remove(&k).is_some() {
                                removed += 1;
                            }
                        }
                        RespValue::Integer(removed)
                    }
                    "EXISTS" if args.len() == 2 => {
                        let key = String::from_utf8_lossy(&args[1]).to_string();
                        let present = i64::from(state.lock().unwrap().contains_key(&key));
                        RespValue::Integer(present)
                    }
                    "KEYS" if args.len() == 2 => {
                        let pat = String::from_utf8_lossy(&args[1]);
                        let prefix = pat.strip_suffix('*').unwrap_or(&pat);
                        let map = state.lock().unwrap();
                        let items = map
                            .keys()
                            .filter(|k| k.starts_with(prefix))
                            .map(|k| RespValue::BulkString(k.as_bytes().to_vec()))
                            .collect();
                        RespValue::Array(items)
                    }
                    "SCAN" if args.len() >= 2 => {
                        let mut prefix = String::new();
                        let mut i = 2;
                        while i + 1 < args.len() {
                            if String::from_utf8_lossy(&args[i]).eq_ignore_ascii_case("MATCH") {
                                let pat = String::from_utf8_lossy(&args[i + 1]);
                                let unescaped = pat
                                    .strip_suffix('*')
                                    .unwrap_or(&pat)
                                    .replace("\\*", "*")
                                    .replace("\\?", "?");
                                prefix = unescaped;
                            }
                            i += 2;
                        }
                        let map = state.lock().unwrap();
                        let items: Vec<RespValue> = map
                            .keys()
                            .filter(|k| k.starts_with(&prefix))
                            .map(|k| RespValue::BulkString(k.as_bytes().to_vec()))
                            .collect();
                        RespValue::Array(vec![
                            RespValue::BulkString(b"0".to_vec()),
                            RespValue::Array(items),
                        ])
                    }
                    _ => RespValue::Error(format!("ERR unknown command '{cmd}'")),
                };

                let mut out = Vec::new();
                reply.encode(&mut out).unwrap();
                if writer.write_all(&out).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        }
    });

    (addr, handle)
}

#[test]
fn redis_client_loopback_mock_server_commands() {
    let (addr, handle) = spawn_mock_redis_server();
    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();
    client.set_timeout(Some(Duration::from_secs(5))).unwrap();

    assert_eq!(client.ping().unwrap(), "PONG");
    assert_eq!(client.get("missing").unwrap(), None);

    client.set("k1", b"val1").unwrap();
    client
        .set_ex("k2", b"val2", Duration::from_secs(30))
        .unwrap();
    assert_eq!(client.get("k1").unwrap(), Some(b"val1".to_vec()));
    assert_eq!(client.get("k2").unwrap(), Some(b"val2".to_vec()));

    assert_eq!(
        client.keys("k*").unwrap(),
        vec!["k1".to_string(), "k2".to_string()]
    );
    assert_eq!(
        client.scan("k*").unwrap(),
        vec!["k1".to_string(), "k2".to_string()]
    );

    assert_eq!(client.del(&["k1", "nonexistent"]).unwrap(), 1);
    assert_eq!(client.get("k1").unwrap(), None);

    // Server error `-ERR ...` maps to `KvError::Server`.
    let err = client.command([b"FOOBAR".as_slice()]).unwrap_err();
    assert!(matches!(err, KvError::Server(ref msg) if msg.contains("unknown command")));

    drop(client);
    handle.join().unwrap();
}

#[test]
fn redis_client_kv_store_trait_batch_and_scan_prefix() {
    let (addr, handle) = spawn_mock_redis_server();
    let client = RedisClient::connect(addr)
        .unwrap()
        .with_limits(RespLimits::default())
        .with_kv_limits(KvLimits::default());

    let store: &dyn KvStore = &client;
    store
        .batch(&[
            BatchOp::set("ns:1", b"one".to_vec()),
            BatchOp::set_ex("ns:2", b"two".to_vec(), Duration::from_secs(60)),
            BatchOp::set("other:1", b"x".to_vec()),
        ])
        .unwrap();

    assert!(store.exists("ns:1").unwrap());
    assert!(!store.exists("ns:99").unwrap());

    let scanned = store.scan_prefix("ns:").unwrap();
    assert_eq!(
        scanned,
        vec![
            ("ns:1".to_string(), b"one".to_vec()),
            ("ns:2".to_string(), b"two".to_vec()),
        ]
    );

    let ranged = store.scan_range("ns:1", "ns:2").unwrap();
    assert_eq!(ranged, vec![("ns:1".to_string(), b"one".to_vec())]);

    assert!(store.delete("ns:1").unwrap());
    assert!(!store.delete("ns:1").unwrap());

    drop(client);
    handle.join().unwrap();
}

fn verify_kv_store_compliance(store: &dyn KvStore) {
    store.set("p:b", b"2").unwrap();
    store.set("p:a", b"1").unwrap();
    store.set_ex("p:c", b"3", Duration::from_secs(120)).unwrap();

    assert!(store.exists("p:a").unwrap());
    assert_eq!(store.get("p:a").unwrap(), Some(b"1".to_vec()));

    let rows = store.scan_prefix("p:").unwrap();
    assert_eq!(
        rows,
        vec![
            ("p:a".to_string(), b"1".to_vec()),
            ("p:b".to_string(), b"2".to_vec()),
            ("p:c".to_string(), b"3".to_vec()),
        ]
    );

    store
        .batch(&[
            BatchOp::set("p:a", b"1_updated".to_vec()),
            BatchOp::delete("p:b"),
        ])
        .unwrap();

    assert_eq!(store.get("p:a").unwrap(), Some(b"1_updated".to_vec()));
    assert_eq!(store.get("p:b").unwrap(), None);
}

#[test]
fn kv_polymorphic_trait_compliance_across_all_three_backends() {
    let mem = MemoryStore::new();
    verify_kv_store_compliance(&mem);

    let dir = TempDir::new("akku-kv").unwrap();
    let file = FileStore::open(dir.as_path().join("poly.wal")).unwrap();
    verify_kv_store_compliance(&file);

    let (addr, handle) = spawn_mock_redis_server();
    let redis = RedisClient::connect(addr).unwrap();
    verify_kv_store_compliance(&redis);
    drop(redis);
    handle.join().unwrap();
}

#[test]
fn kv_error_display_and_source_formatting() {
    let io_err = KvError::from(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "missing file",
    ));
    assert!(io_err.to_string().contains("missing file"));
    assert!(std::error::Error::source(&io_err).is_some());

    let corrupt = KvError::CorruptData("bad crc".to_string());
    assert!(corrupt.to_string().contains("bad crc"));
    assert!(std::error::Error::source(&corrupt).is_none());

    let proto = KvError::Protocol("bad frame".to_string());
    assert!(proto.to_string().contains("bad frame"));

    let srv = KvError::Server("ERR auth".to_string());
    assert!(srv.to_string().contains("ERR auth"));

    let inv = KvError::InvalidInput("empty key".to_string());
    assert!(inv.to_string().contains("empty key"));
}

#[test]
fn resp_bulk_and_aggregate_reply_budgets() {
    // A huge claimed bulk length with few bytes behind it errors instead of allocating upfront.
    let mut c = Cursor::new(b"$16000000\r\nab".to_vec());
    assert!(matches!(
        RespValue::decode(&mut c),
        Err(KvError::Protocol(_))
    ));

    // Nested arrays share one element budget per reply.
    let limits = RespLimits {
        max_array_len: 4,
        ..RespLimits::default()
    };
    let mut c = Cursor::new(b"*2\r\n*3\r\n:1\r\n:1\r\n:1\r\n*3\r\n:1\r\n:1\r\n:1\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut c, limits),
        Err(KvError::Protocol(ref m)) if m.contains("total element count")
    ));

    // Total bulk payload across the reply is capped by max_reply_bytes.
    let limits = RespLimits {
        max_reply_bytes: 5,
        ..RespLimits::default()
    };
    let mut c = Cursor::new(b"*2\r\n$3\r\nabc\r\n$3\r\nabc\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut c, limits),
        Err(KvError::Protocol(ref m)) if m.contains("max_reply_bytes")
    ));
    let mut c = Cursor::new(b"*2\r\n$2\r\nab\r\n$3\r\nabc\r\n".to_vec());
    assert!(RespValue::decode_with_limits(&mut c, limits).is_ok());
}

type ConnHandler = Box<dyn FnOnce(BufReader<std::net::TcpStream>, std::net::TcpStream) + Send>;

/// Reads one command frame and returns its arguments as UTF-8 strings.
fn read_cmd(reader: &mut BufReader<std::net::TcpStream>) -> Option<Vec<String>> {
    match RespValue::decode(reader).ok()? {
        RespValue::Array(items) => Some(
            items
                .iter()
                .map(|i| String::from_utf8_lossy(i.as_bytes().unwrap_or_default()).to_string())
                .collect(),
        ),
        _ => None,
    }
}

/// Serves `GET k -> "value-of-k"` and `+OK` for everything else until the peer disconnects.
fn serve_normal(mut reader: BufReader<std::net::TcpStream>, mut writer: std::net::TcpStream) {
    while let Some(args) = read_cmd(&mut reader) {
        let reply = if args.first().map(String::as_str) == Some("GET") && args.len() == 2 {
            let v = format!("value-of-{}", args[1]);
            format!("${}\r\n{v}\r\n", v.len())
        } else {
            "+OK\r\n".to_string()
        };
        if writer.write_all(reply.as_bytes()).is_err() {
            break;
        }
    }
}

/// Spawns a scripted server: the n-th accepted connection is driven by `handlers[n]`.
/// Returns the address, the accept counter, and the server thread handle.
fn spawn_scripted_server(
    handlers: Vec<ConnHandler>,
) -> (
    SocketAddr,
    Arc<std::sync::atomic::AtomicUsize>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    let handle = thread::spawn(move || {
        let mut workers = Vec::new();
        for handler in handlers {
            let Ok((stream, _)) = listener.accept() else {
                break;
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let reader = BufReader::new(stream.try_clone().unwrap());
            workers.push(thread::spawn(move || handler(reader, stream)));
        }
        for w in workers {
            let _ = w.join();
        }
    });
    (addr, accepted, handle)
}

#[test]
fn redis_client_timeout_mid_reply_poisons_and_reconnects() {
    let first: ConnHandler = Box::new(|mut reader, mut writer| {
        let _ = read_cmd(&mut reader); // GET slow
        let _ = writer.write_all(b"$11\r\nstale");
        let _ = writer.flush();
        thread::sleep(Duration::from_millis(500));
        // The late remainder of the old reply must never reach a later command.
        let _ = writer.write_all(b"-value\r\n");
        serve_normal(reader, writer);
    });
    let (addr, accepted, handle) = spawn_scripted_server(vec![first, Box::new(serve_normal)]);

    let client = RedisClient::connect_timeout(addr, Duration::from_millis(150)).unwrap();
    assert!(client.get("slow").is_err(), "partial reply must time out");
    thread::sleep(Duration::from_millis(600));

    assert_eq!(client.get("k2").unwrap(), Some(b"value-of-k2".to_vec()));
    assert_eq!(client.get("k3").unwrap(), Some(b"value-of-k3".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(client);
    handle.join().unwrap();
}

#[test]
fn redis_client_garbled_reply_never_leaks_stale_value() {
    let first: ConnHandler = Box::new(|mut reader, mut writer| {
        let _ = read_cmd(&mut reader); // GET a
                                       // Bulk payload without CRLF, followed by a would-be "next" reply.
        let _ = writer.write_all(b"$3\r\nabcXX$5\r\nstale\r\n");
        serve_normal(reader, writer);
    });
    let (addr, accepted, handle) = spawn_scripted_server(vec![first, Box::new(serve_normal)]);

    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();
    assert!(matches!(client.get("a"), Err(KvError::Protocol(_))));
    let next = client.get("b").unwrap();
    assert_ne!(next, Some(b"stale".to_vec()));
    assert_eq!(next, Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(client);
    handle.join().unwrap();
}

#[test]
fn redis_client_batch_mid_pipeline_error_poisons_connection() {
    let first: ConnHandler = Box::new(|mut reader, mut writer| {
        for _ in 0..3 {
            let _ = read_cmd(&mut reader);
        }
        let _ = writer.write_all(b"+OK\r\n!garbage\r\n+OK\r\n");
        serve_normal(reader, writer);
    });
    let (addr, accepted, handle) = spawn_scripted_server(vec![first, Box::new(serve_normal)]);

    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();
    let ops = [
        BatchOp::set("x", b"1".to_vec()),
        BatchOp::set("y", b"2".to_vec()),
        BatchOp::set("z", b"3".to_vec()),
    ];
    assert!(matches!(
        KvStore::batch(&client, &ops),
        Err(KvError::Protocol(_))
    ));
    assert_eq!(client.get("q").unwrap(), Some(b"value-of-q".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(client);
    handle.join().unwrap();
}

#[test]
fn redis_client_server_error_reply_keeps_connection() {
    let only: ConnHandler = Box::new(|mut reader, mut writer| {
        let _ = read_cmd(&mut reader);
        let _ = writer.write_all(b"-ERR boom\r\n");
        serve_normal(reader, writer);
    });
    let (addr, accepted, handle) = spawn_scripted_server(vec![only]);

    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();
    assert!(matches!(client.get("a"), Err(KvError::Server(ref m)) if m == "ERR boom"));
    assert_eq!(client.get("b").unwrap(), Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(client);
    handle.join().unwrap();
}

// ---------------------------------------------------------------------------
// Fix-pass regressions: batch envelope, tail scan, locking, poisoning, Redis session.
// ---------------------------------------------------------------------------

/// Writes `a` plus a 3-op batch and returns `(path, bytes, offset_of_batch_begin)`.
fn batch_wal(dir: &TempDir, name: &str) -> (std::path::PathBuf, Vec<u8>, u64) {
    let wal_path = dir.as_path().join(name);
    let begin = {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("a", b"base").unwrap();
        let begin = store.disk_size().unwrap();
        store
            .batch(&[
                BatchOp::set("b1", b"one".to_vec()),
                BatchOp::delete("a"),
                BatchOp::set("b3", b"three".to_vec()),
            ])
            .unwrap();
        begin
    };
    (wal_path.clone(), fs::read(&wal_path).unwrap(), begin)
}

#[test]
fn file_store_committed_batch_replays_fully() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, _, _) = batch_wal(&dir, "batch_ok.wal");
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.get("a").unwrap(), None);
    assert_eq!(store.get("b1").unwrap(), Some(b"one".to_vec()));
    assert_eq!(store.get("b3").unwrap(), Some(b"three".to_vec()));
}

#[test]
fn file_store_torn_batch_is_dropped_as_a_whole() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, begin) = batch_wal(&dir, "batch_torn.wal");

    // Torn inside the last batch record (commit marker never written): no prefix applies.
    let commit_len = 21;
    for cut in [commit_len + 3, commit_len] {
        fs::write(&wal_path, &bytes[..bytes.len() - cut]).unwrap();
        let store = FileStore::open(&wal_path).unwrap();
        assert_eq!(store.get("a").unwrap(), Some(b"base".to_vec()), "cut {cut}");
        assert_eq!(store.get("b1").unwrap(), None, "cut {cut}");
        assert_eq!(store.get("b3").unwrap(), None, "cut {cut}");
        assert_eq!(
            store.disk_size().unwrap(),
            begin,
            "uncommitted envelope truncated"
        );
        drop(store);
    }

    // Torn inside the commit marker itself.
    fs::write(&wal_path, &bytes[..bytes.len() - 5]).unwrap();
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.get("b1").unwrap(), None);
    assert_eq!(store.disk_size().unwrap(), begin);
    // The recovered log accepts new writes and replays them.
    store.set("after", b"x").unwrap();
    drop(store);
    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.get("after").unwrap(), Some(b"x".to_vec()));
    assert_eq!(store.get("a").unwrap(), Some(b"base".to_vec()));
}

#[test]
fn file_store_batch_commit_without_begin_is_corrupt() {
    let dir = TempDir::new("akku-kv").unwrap();
    let (wal_path, bytes, begin) = batch_wal(&dir, "batch_nobegin.wal");
    let begin = begin as usize;
    let mut cut = bytes[..begin].to_vec();
    cut.extend_from_slice(&bytes[begin + 21..]);
    fs::write(&wal_path, &cut).unwrap();
    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::CorruptData(ref m)) if m.contains("without begin")
    ));
    assert_eq!(fs::read(&wal_path).unwrap(), cut, "file must be untouched");
}

#[test]
fn file_store_single_op_batch_and_legacy_records_need_no_envelope() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("single.wal");
    let store = FileStore::open(&wal_path).unwrap();
    let before = store.disk_size().unwrap();
    store.batch(&[BatchOp::set("k", b"v".to_vec())]).unwrap();
    // 21-byte header + key + value, no begin/commit markers.
    assert_eq!(store.disk_size().unwrap() - before, 21 + 1 + 1);
    store.batch(&[]).unwrap();
    assert_eq!(store.disk_size().unwrap() - before, 23);
}

#[test]
fn file_store_embedded_frame_in_torn_value_is_not_false_corruption() {
    let dir = TempDir::new("akku-kv").unwrap();

    // A complete, CRC-valid frame to embed inside another record's value bytes.
    let donor = dir.as_path().join("donor.wal");
    {
        let store = FileStore::open(&donor).unwrap();
        store.set("zz", b"embedded").unwrap();
    }
    let frame = fs::read(&donor).unwrap()[WAL_MAGIC.len()..].to_vec();

    let wal_path = dir.as_path().join("embedded.wal");
    let k2_offset = {
        let store = FileStore::open(&wal_path).unwrap();
        store.set("k1", b"v1").unwrap();
        let off = store.disk_size().unwrap();
        let mut value = b"prefix".to_vec();
        value.extend_from_slice(&frame);
        value.extend_from_slice(&[0xAB; 100]);
        store.set("k2", &value).unwrap();
        off
    };
    let bytes = fs::read(&wal_path).unwrap();
    // Tear the final record: the embedded frame survives intact, followed by partial garbage.
    fs::write(&wal_path, &bytes[..bytes.len() - 50]).unwrap();

    let store = FileStore::open(&wal_path).unwrap();
    assert_eq!(store.get("k1").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(store.get("k2").unwrap(), None);
    assert_eq!(store.get("zz").unwrap(), None);
    assert_eq!(store.disk_size().unwrap(), k2_offset);
}

#[test]
fn file_store_second_open_is_rejected_by_lock_file() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("locked.wal");
    let lock = akku::kv::file::lock_path(&wal_path);

    let store = FileStore::open(&wal_path).unwrap();
    assert!(lock.exists());
    let second = FileStore::open(&wal_path);
    assert!(
        matches!(second, Err(KvError::Io(ref e)) if e.kind() == std::io::ErrorKind::AlreadyExists
            && e.to_string().contains("break_lock")),
        "got {second:?}"
    );
    // The rejected open must not have removed the owner's lock.
    assert!(lock.exists());
    store.set("k", b"v").unwrap();
    drop(store);
    assert!(!lock.exists(), "lock released on drop");

    let reopened = FileStore::open(&wal_path).unwrap();
    assert_eq!(reopened.get("k").unwrap(), Some(b"v".to_vec()));
    drop(reopened);

    // A stale lock (crashed owner) blocks opening until explicitly broken.
    fs::write(&lock, b"999999\n").unwrap();
    assert!(matches!(
        FileStore::open(&wal_path),
        Err(KvError::Io(ref e)) if e.to_string().contains("999999")
    ));
    assert!(FileStore::break_lock(&wal_path).unwrap());
    assert!(!FileStore::break_lock(&wal_path).unwrap());
    assert!(FileStore::open(&wal_path).is_ok());
}

#[test]
fn file_store_failed_reopen_after_compaction_poisons_writes() {
    let dir = TempDir::new("akku-kv").unwrap();
    let wal_path = dir.as_path().join("poison.wal");
    let store = FileStore::open(&wal_path).unwrap();
    store.set("k", b"v").unwrap();

    // Replace the WAL with a non-empty directory: the snapshot rename and the reopen both fail.
    fs::remove_file(&wal_path).unwrap();
    fs::create_dir(&wal_path).unwrap();
    fs::write(wal_path.join("blocker"), b"x").unwrap();

    assert!(store.compact().is_err());
    assert!(matches!(
        store.set("k2", b"v2"),
        Err(KvError::Io(ref e)) if e.to_string().contains("poisoned")
    ));
    assert!(store.delete("k").is_err());
    assert!(store.batch(&[BatchOp::set("x", b"1".to_vec())]).is_err());
    assert!(store.sync().is_err());
    assert!(store.compact().is_err());
    // Reads keep serving the in-memory state.
    assert_eq!(store.get("k").unwrap(), Some(b"v".to_vec()));
    assert_eq!(store.get("k2").unwrap(), None);
}

#[test]
fn resp_simple_strings_and_errors_are_charged_to_reply_budget() {
    let limits = RespLimits {
        max_reply_bytes: 5,
        ..RespLimits::default()
    };
    let mut ok = Cursor::new(b"+hello\r\n".to_vec());
    assert!(RespValue::decode_with_limits(&mut ok, limits).is_ok());
    let mut long = Cursor::new(b"+hello!\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut long, limits),
        Err(KvError::Protocol(ref m)) if m.contains("max_reply_bytes")
    ));
    let mut errs = Cursor::new(b"*2\r\n-abc\r\n-def\r\n".to_vec());
    assert!(matches!(
        RespValue::decode_with_limits(&mut errs, limits),
        Err(KvError::Protocol(ref m)) if m.contains("max_reply_bytes")
    ));
}

#[test]
fn resp_unsupported_resp3_types_are_rejected() {
    for frame in [
        &b",1.5\r\n"[..],
        b"(123\r\n",
        b"!3\r\nerr\r\n",
        b"=7\r\ntxt:abc\r\n",
        b">1\r\n+x\r\n",
        b"|1\r\n+a\r\n+b\r\n",
    ] {
        let mut c = Cursor::new(frame.to_vec());
        assert!(
            matches!(RespValue::decode(&mut c), Err(KvError::Protocol(ref m)) if m.contains("RESP2 only")),
            "frame {:?}",
            String::from_utf8_lossy(frame)
        );
    }
}

type ConnLog = Arc<Mutex<Vec<Vec<Vec<String>>>>>;

/// A server connection that logs every command. `GET poison` gets a garbage reply (which
/// poisons the client connection), `AUTH ... bad` is rejected, `GET k` returns
/// `value-of-k`, everything else `+OK`.
fn logging_handler(log: ConnLog) -> ConnHandler {
    Box::new(move |mut reader, mut writer| {
        let idx = {
            let mut l = log.lock().unwrap();
            l.push(Vec::new());
            l.len() - 1
        };
        while let Some(args) = read_cmd(&mut reader) {
            log.lock().unwrap()[idx].push(args.clone());
            let reply = match args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["GET", "poison"] => "!garbage\r\n".to_string(),
                ["GET", k] => {
                    let v = format!("value-of-{k}");
                    format!("${}\r\n{v}\r\n", v.len())
                }
                ["AUTH", .., "bad"] => "-WRONGPASS invalid password\r\n".to_string(),
                _ => "+OK\r\n".to_string(),
            };
            if writer.write_all(reply.as_bytes()).is_err() {
                break;
            }
        }
    })
}

fn logging_server(
    conns: usize,
) -> (
    SocketAddr,
    Arc<std::sync::atomic::AtomicUsize>,
    thread::JoinHandle<()>,
    ConnLog,
) {
    let log: ConnLog = Arc::new(Mutex::new(Vec::new()));
    let handlers = (0..conns)
        .map(|_| logging_handler(Arc::clone(&log)))
        .collect();
    let (addr, accepted, handle) = spawn_scripted_server(handlers);
    (addr, accepted, handle, log)
}

fn strs(cmd: &[&str]) -> Vec<String> {
    cmd.iter().map(|s| s.to_string()).collect()
}

#[test]
fn redis_client_reconnect_replays_auth_and_select() {
    use akku::kv::redis::RedisOptions;
    let (addr, accepted, handle, log) = logging_server(3);

    let opts = RedisOptions::new()
        .timeout(Duration::from_secs(5))
        .auth("user", "secret")
        .db(3);
    assert!(!format!("{opts:?}").contains("secret"), "password redacted");
    let client = RedisClient::connect_with(addr, opts).unwrap();
    assert_eq!(client.get("a").unwrap(), Some(b"value-of-a".to_vec()));
    assert!(client.get("poison").is_err());
    assert_eq!(client.get("b").unwrap(), Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);

    // Raw SELECT / AUTH are tracked and replayed instead of the original options.
    client.command(["SELECT", "5"]).unwrap();
    client.command(["AUTH", "other"]).unwrap();
    assert!(client.get("poison").is_err());
    assert_eq!(client.get("c").unwrap(), Some(b"value-of-c".to_vec()));
    drop(client);
    handle.join().unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 3);
    for conn in &log[..2] {
        assert_eq!(conn[0], strs(&["AUTH", "user", "secret"]));
        assert_eq!(conn[1], strs(&["SELECT", "3"]));
    }
    assert_eq!(log[1][2], strs(&["GET", "b"]));
    assert_eq!(log[2][0], strs(&["AUTH", "other"]));
    assert_eq!(log[2][1], strs(&["SELECT", "5"]));
    assert_eq!(log[2][2], strs(&["GET", "c"]));
}

#[test]
fn redis_client_rejected_auth_fails_connect() {
    use akku::kv::redis::RedisOptions;
    let (addr, _, handle, _) = logging_server(1);
    let res = RedisClient::connect_with(
        addr,
        RedisOptions::new()
            .timeout(Duration::from_secs(5))
            .password("bad"),
    );
    assert!(matches!(res, Err(KvError::Server(ref m)) if m.contains("WRONGPASS")));
    drop(res);
    handle.join().unwrap();
}

#[test]
fn redis_client_lost_multi_errors_instead_of_silent_reconnect() {
    let (addr, accepted, handle, log) = logging_server(2);
    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();

    client.command(["MULTI"]).unwrap();
    assert!(client.get("poison").is_err());
    // Queued commands must not run on a fresh, non-transactional connection.
    assert!(matches!(
        client.command(["SET", "x", "1"]),
        Err(KvError::Protocol(ref m)) if m.contains("MULTI")
    ));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(matches!(
        client.command(["EXEC"]),
        Err(KvError::Protocol(ref m)) if m.contains("transaction aborted")
    ));
    // After the abort is reported, normal commands reconnect.
    assert_eq!(client.get("b").unwrap(), Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(client);
    handle.join().unwrap();
    let log = log.lock().unwrap();
    assert!(log
        .iter()
        .flatten()
        .all(|c| c[0] != "SET" && c[0] != "EXEC"));
}

#[test]
fn redis_client_pinned_session_requires_explicit_reconnect() {
    let (addr, accepted, handle, _) = logging_server(2);
    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();

    client.command(["client", "setname", "worker-1"]).unwrap();
    assert!(client.get("poison").is_err());
    assert!(matches!(
        client.get("b"),
        Err(KvError::Protocol(ref m)) if m.contains("CLIENT SETNAME")
    ));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    client.reconnect().unwrap();
    assert_eq!(client.get("b").unwrap(), Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    drop(client);
    handle.join().unwrap();
}

#[test]
fn redis_client_rejects_pubsub_monitor_tracking_and_resp3() {
    let (addr, accepted, handle, log) = logging_server(1);
    let client = RedisClient::connect_timeout(addr, Duration::from_secs(5)).unwrap();

    for cmd in [
        &["SUBSCRIBE", "ch"][..],
        &["psubscribe", "p*"],
        &["SSUBSCRIBE", "s"],
        &["MONITOR"],
        &["CLIENT", "TRACKING", "ON"],
        &["CLIENT", "REPLY", "OFF"],
        &["HELLO", "3"],
        &[],
    ] {
        assert!(
            matches!(client.command(cmd), Err(KvError::InvalidInput(_))),
            "{cmd:?} must be rejected"
        );
    }
    client.command(["HELLO", "2"]).unwrap();
    assert_eq!(client.get("b").unwrap(), Some(b"value-of-b".to_vec()));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(client);
    handle.join().unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log[0], vec![strs(&["HELLO", "2"]), strs(&["GET", "b"])]);
}
