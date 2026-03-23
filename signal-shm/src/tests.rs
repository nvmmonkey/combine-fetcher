use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::layout::{AssetSlot, ShmHeader, SignalSnapshot, SHM_MAGIC, SHM_VERSION};
use crate::reader::SignalReader;
use crate::writer::SignalWriter;

/// Helper: create a temp file path for testing.
fn tmp_shm_path(name: &str) -> String {
    let dir = std::env::temp_dir();
    dir.join(format!("signal-shm-test-{}-{}.shm", name, std::process::id()))
        .to_string_lossy()
        .into_owned()
}

/// Helper: clean up a temp file after test.
fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
}

#[test]
fn test_create_and_read_back() {
    let path = tmp_shm_path("create-read");

    // Writer creates the SHM file.
    let writer = SignalWriter::create(&path, 4).expect("create writer");

    // Verify header through writer.
    let hdr = writer.header();
    assert_eq!(hdr.magic.load(Ordering::Relaxed), SHM_MAGIC);
    assert_eq!(hdr.version.load(Ordering::Relaxed), SHM_VERSION as u64);
    assert_eq!(hdr.slot_count.load(Ordering::Relaxed), 4);
    assert_eq!(
        hdr.engine_pid.load(Ordering::Relaxed),
        std::process::id() as u64
    );

    // Write data into slot 0.
    let slot = writer.slot(0);
    slot.begin_write();
    AssetSlot::write_string(&slot.asset, "btc");
    slot.timeframe_secs.store(900, Ordering::Relaxed);
    AssetSlot::write_f64(&slot.binance_mid, 68000.50);
    AssetSlot::write_f64(&slot.yes_bid, 0.55);
    AssetSlot::write_f64(&slot.no_ask, 0.48);
    slot.ttx_seconds.store(-120, Ordering::Relaxed);
    slot.market_epoch.store(42, Ordering::Relaxed);
    AssetSlot::write_string(&slot.yes_token_id, "tok-yes-abc123");
    AssetSlot::write_string(&slot.no_token_id, "tok-no-def456");
    slot.end_write();

    // Reader opens and reads back.
    let reader = SignalReader::open(&path).expect("open reader");
    assert_eq!(reader.slot_count(), 4);

    let snap = reader.read_slot(0).expect("read slot 0");
    assert_eq!(snap.asset, "btc");
    assert_eq!(snap.timeframe_secs, 900);
    assert!((snap.binance_mid - 68000.50).abs() < f64::EPSILON);
    assert!((snap.yes_bid - 0.55).abs() < f64::EPSILON);
    assert!((snap.no_ask - 0.48).abs() < f64::EPSILON);
    assert_eq!(snap.ttx_seconds, -120);
    assert_eq!(snap.market_epoch, 42);
    assert_eq!(snap.yes_token_id, "tok-yes-abc123");
    assert_eq!(snap.no_token_id, "tok-no-def456");

    cleanup(&path);
}

#[test]
fn test_f64_roundtrip() {
    let path = tmp_shm_path("f64-roundtrip");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    let slot = writer.slot(0);

    let test_values: &[f64] = &[0.0, -1.0, 68000.50, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e-300];

    for (i, &val) in test_values.iter().enumerate() {
        // Reuse different fields for each value to keep it simple.
        let field = match i {
            0 => &slot.binance_mid,
            1 => &slot.binance_bid,
            2 => &slot.binance_ask,
            3 => &slot.binance_volume_1m,
            4 => &slot.yes_bid,
            5 => &slot.yes_ask,
            6 => &slot.yes_bid_size,
            _ => unreachable!(),
        };
        AssetSlot::write_f64(field, val);
        let got = AssetSlot::read_f64(field);

        if val.is_nan() {
            assert!(got.is_nan(), "expected NaN, got {}", got);
        } else {
            assert_eq!(
                val.to_bits(),
                got.to_bits(),
                "f64 roundtrip failed for value {}",
                val
            );
        }
    }

    cleanup(&path);
}

#[test]
fn test_string_roundtrip() {
    let path = tmp_shm_path("string-roundtrip");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    let slot = writer.slot(0);

    // Normal string.
    AssetSlot::write_string(&slot.yes_token_id, "0xabcdef1234567890abcdef1234567890abcdef12");
    let got = AssetSlot::read_string(&slot.yes_token_id);
    assert_eq!(got, "0xabcdef1234567890abcdef1234567890abcdef12");

    // Empty string.
    AssetSlot::write_string(&slot.no_token_id, "");
    let got = AssetSlot::read_string(&slot.no_token_id);
    assert_eq!(got, "");

    // Max-length string (fill entire field).
    let long = "a".repeat(crate::layout::TOKEN_ID_LEN);
    AssetSlot::write_string(&slot.condition_id, &long);
    let got = AssetSlot::read_string(&slot.condition_id);
    assert_eq!(got, long);

    // Overwrite with shorter string — old data should be cleared.
    AssetSlot::write_string(&slot.condition_id, "short");
    let got = AssetSlot::read_string(&slot.condition_id);
    assert_eq!(got, "short");

    // Asset name (8-byte field).
    AssetSlot::write_string(&slot.asset, "eth");
    let got = AssetSlot::read_string(&slot.asset);
    assert_eq!(got, "eth");

    cleanup(&path);
}

#[test]
fn test_seqlock_consistency() {
    let path = tmp_shm_path("seqlock");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    let slot = writer.slot(0);

    // Initially seq is 0 (even) — reads should succeed.
    let reader = SignalReader::open(&path).expect("open reader");
    let snap = reader.read_slot(0);
    assert!(snap.is_some(), "should read with seq=0");

    // Simulate a write-in-progress: set seq to odd.
    slot.seq.store(1, Ordering::Release);

    // Reader should fail (all 3 retries see odd seq).
    let snap = reader.read_slot(0);
    assert!(snap.is_none(), "should fail with odd seq (write in progress)");

    // Complete the write: set seq to 2 (even again).
    slot.seq.store(2, Ordering::Release);

    let snap = reader.read_slot(0);
    assert!(snap.is_some(), "should succeed with seq=2");
    assert_eq!(snap.unwrap().seq, 2);

    cleanup(&path);
}

#[test]
fn test_multiple_slots() {
    let path = tmp_shm_path("multi-slot");
    let writer = SignalWriter::create(&path, 8).expect("create writer");

    let assets = ["btc", "eth", "sol", "matic", "arb", "op", "avax", "doge"];
    let timeframes: [u64; 8] = [900, 900, 300, 300, 900, 300, 900, 300];

    for i in 0..8 {
        let slot = writer.slot(i);
        slot.begin_write();
        AssetSlot::write_string(&slot.asset, assets[i]);
        slot.timeframe_secs.store(timeframes[i], Ordering::Relaxed);
        AssetSlot::write_f64(&slot.strike, 60000.0 + i as f64 * 1000.0);
        slot.market_epoch.store(i as u64 + 100, Ordering::Relaxed);
        slot.end_write();
    }

    let reader = SignalReader::open(&path).expect("open reader");
    assert_eq!(reader.slot_count(), 8);

    for i in 0..8 {
        let snap = reader.read_slot(i).expect(&format!("read slot {}", i));
        assert_eq!(snap.asset, assets[i], "slot {} asset mismatch", i);
        assert_eq!(snap.timeframe_secs, timeframes[i], "slot {} tf mismatch", i);
        assert!(
            (snap.strike - (60000.0 + i as f64 * 1000.0)).abs() < f64::EPSILON,
            "slot {} strike mismatch",
            i
        );
        assert_eq!(snap.market_epoch, i as u64 + 100, "slot {} epoch mismatch", i);
    }

    // Test find-by-name.
    let snap = reader.read("sol", 300).expect("find sol/300");
    assert_eq!(snap.asset, "sol");
    assert_eq!(snap.timeframe_secs, 300);

    // Non-existent.
    assert!(reader.read("xrp", 900).is_none());
    // Wrong timeframe.
    assert!(reader.read("btc", 300).is_none());

    cleanup(&path);
}

#[test]
fn test_engine_alive() {
    let path = tmp_shm_path("engine-alive");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    // Heartbeat was just set in create().
    let reader = SignalReader::open(&path).expect("open reader");
    assert!(reader.engine_alive(), "engine should be alive right after create");

    // Simulate stale heartbeat (6 seconds ago).
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    writer
        .header()
        .engine_heartbeat_us
        .store(now_us - 6_000_000, Ordering::Relaxed);

    assert!(
        !reader.engine_alive(),
        "engine should be dead with 6s-old heartbeat"
    );

    // Refresh heartbeat.
    writer.heartbeat();
    assert!(reader.engine_alive(), "engine should be alive after heartbeat");

    cleanup(&path);
}

#[test]
fn test_staleness_detection() {
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;

    let threshold = 2_000_000; // 2 seconds

    // Fresh data: both timestamps recent.
    let mut snap = SignalSnapshot::default();
    snap.binance_update_us = now_us;
    snap.poly_yes_update_us = now_us;
    assert!(
        !SignalReader::is_stale(&snap, threshold),
        "fresh data should not be stale"
    );

    // Only Binance is stale, poly is fresh.
    snap.binance_update_us = now_us - 3_000_000;
    snap.poly_yes_update_us = now_us;
    assert!(
        !SignalReader::is_stale(&snap, threshold),
        "should not be stale if poly is fresh"
    );

    // Only poly is stale, Binance is fresh.
    snap.binance_update_us = now_us;
    snap.poly_yes_update_us = now_us - 3_000_000;
    assert!(
        !SignalReader::is_stale(&snap, threshold),
        "should not be stale if binance is fresh"
    );

    // Both stale.
    snap.binance_update_us = now_us - 3_000_000;
    snap.poly_yes_update_us = now_us - 3_000_000;
    assert!(
        SignalReader::is_stale(&snap, threshold),
        "both old -> stale"
    );

    // Zero timestamps (slot not yet populated).
    snap.binance_update_us = 0;
    snap.poly_yes_update_us = 0;
    assert!(
        SignalReader::is_stale(&snap, threshold),
        "zero timestamps -> stale"
    );
}

#[test]
fn test_default_snapshot() {
    let snap = SignalSnapshot::default();
    assert_eq!(snap.asset, "");
    assert_eq!(snap.timeframe_secs, 0);
    assert_eq!(snap.seq, 0);
    assert_eq!(snap.binance_mid, 0.0);
    assert_eq!(snap.ttx_seconds, 0);
    assert!(!snap.market_resolved);
    assert_eq!(snap.yes_token_id, "");
    assert_eq!(snap.outcome, "");
}

#[test]
fn test_header_validation() {
    let path = tmp_shm_path("header-validate");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    let hdr = writer.header();
    assert!(hdr.validate());

    // Corrupt magic.
    hdr.magic.store(0xDEADBEEF, Ordering::Relaxed);
    assert!(!hdr.validate());

    // Restore magic, corrupt version.
    hdr.magic.store(SHM_MAGIC, Ordering::Relaxed);
    hdr.version.store(999, Ordering::Relaxed);
    assert!(!hdr.validate());

    // Reader should fail to open a corrupted file.
    let result = SignalReader::open(&path);
    assert!(result.is_err());

    cleanup(&path);
}

#[test]
fn test_slot_out_of_range() {
    let path = tmp_shm_path("slot-range");
    let _writer = SignalWriter::create(&path, 2).expect("create writer");

    let reader = SignalReader::open(&path).expect("open reader");
    assert!(reader.read_slot(0).is_some());
    assert!(reader.read_slot(1).is_some());
    assert!(reader.read_slot(2).is_none());
    assert!(reader.read_slot(100).is_none());

    cleanup(&path);
}

#[test]
fn test_resolution_state() {
    let path = tmp_shm_path("resolution");
    let writer = SignalWriter::create(&path, 1).expect("create writer");

    let slot = writer.slot(0);
    slot.begin_write();
    AssetSlot::write_string(&slot.asset, "btc");
    slot.timeframe_secs.store(900, Ordering::Relaxed);
    slot.market_resolved.store(1, Ordering::Relaxed);
    AssetSlot::write_string(&slot.outcome, "YES");
    slot.end_write();

    let reader = SignalReader::open(&path).expect("open reader");
    let snap = reader.read_slot(0).expect("read slot");
    assert!(snap.market_resolved);
    assert_eq!(snap.outcome, "YES");

    cleanup(&path);
}

#[test]
fn test_layout_sizes_are_deterministic() {
    // Ensure repr(C) gives us predictable sizes.
    let header_size = std::mem::size_of::<ShmHeader>();
    let slot_size = std::mem::size_of::<AssetSlot>();

    // Header: 6 x AtomicU64 (48) + 16 pad = 64, aligned to 64.
    assert_eq!(header_size, 64, "ShmHeader size changed unexpectedly");

    // Slot size should be a multiple of 64 (align(64)).
    assert_eq!(
        slot_size % 64,
        0,
        "AssetSlot size ({}) is not 64-byte aligned",
        slot_size
    );

    // Sanity: slot should be large enough for all fields.
    assert!(slot_size >= 512, "AssetSlot unexpectedly small: {}", slot_size);
}
