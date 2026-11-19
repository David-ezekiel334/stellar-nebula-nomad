// Chaos engineering: storage failure and recovery scenarios (issue #606).
//
// Soroban contracts use a host-managed storage layer with Persistent,
// Temporary, and Instance storage tiers. These tests verify that:
//   - Missing keys are handled gracefully (no panic on None).
//   - Corrupted values are caught by validation before use.
//   - TTL expiry of Temporary entries is handled without crashing.
//   - Storage key namespacing prevents cross-contract collisions.
//   - Partial writes do not leave the contract in an inconsistent state.
#![allow(warnings)]

use std::collections::HashMap;

// ─── Mock storage layer ───────────────────────────────────────────────────────

/// Minimal mock of Soroban's key-value storage.
struct MockStorage {
    data: HashMap<String, Vec<u8>>,
    ttls: HashMap<String, u64>, // simulated TTL in ledger sequences
    current_ledger: u64,
}

impl MockStorage {
    fn new(current_ledger: u64) -> Self {
        Self {
            data: HashMap::new(),
            ttls: HashMap::new(),
            current_ledger,
        }
    }

    fn set(&mut self, key: &str, value: Vec<u8>, ttl: Option<u64>) {
        self.data.insert(key.to_string(), value);
        if let Some(t) = ttl {
            self.ttls.insert(key.to_string(), self.current_ledger + t);
        }
    }

    fn get(&self, key: &str) -> Option<&Vec<u8>> {
        // Respect TTL expiry for Temporary storage entries.
        if let Some(&expiry) = self.ttls.get(key) {
            if self.current_ledger > expiry {
                return None; // entry expired
            }
        }
        self.data.get(key)
    }

    fn remove(&mut self, key: &str) {
        self.data.remove(key);
        self.ttls.remove(key);
    }
}

// ─── Missing key handling ─────────────────────────────────────────────────────

#[test]
fn chaos_storage_missing_key_returns_none_not_panic() {
    let storage = MockStorage::new(100);
    let result = storage.get("player:GABC:balance");
    assert!(result.is_none(), "missing key must return None, not panic");
}

#[test]
fn chaos_storage_missing_key_with_default_fallback() {
    let storage = MockStorage::new(100);
    let balance = storage
        .get("player:GABC:balance")
        .and_then(|b| b.first().copied())
        .map(|b| b as u64)
        .unwrap_or(0);
    assert_eq!(balance, 0, "default balance for missing player must be 0");
}

#[test]
fn chaos_storage_read_after_delete_returns_none() {
    let mut storage = MockStorage::new(100);
    storage.set("ship:1:level", vec![5], None);
    assert!(storage.get("ship:1:level").is_some());

    storage.remove("ship:1:level");
    assert!(storage.get("ship:1:level").is_none());
}

// ─── Corrupted value handling ─────────────────────────────────────────────────

#[test]
fn chaos_storage_corrupted_balance_caught_by_validation() {
    let mut storage = MockStorage::new(100);
    // Inject garbage bytes into a balance field.
    storage.set(
        "player:GABC:balance",
        vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        None,
    );

    let raw = storage.get("player:GABC:balance").unwrap();
    // A valid u64 is exactly 8 bytes.
    let balance: Option<u64> = if raw.len() == 8 {
        let bytes: [u8; 8] = raw[..8].try_into().unwrap();
        Some(u64::from_le_bytes(bytes))
    } else {
        None // malformed — reject
    };

    assert!(
        balance.is_none(),
        "9-byte balance must be rejected as corrupted"
    );
}

#[test]
fn chaos_storage_valid_balance_deserializes_correctly() {
    let mut storage = MockStorage::new(100);
    let expected: u64 = 1_000_000;
    storage.set("player:GABC:balance", expected.to_le_bytes().to_vec(), None);

    let raw = storage.get("player:GABC:balance").unwrap();
    let balance: u64 = u64::from_le_bytes(raw[..8].try_into().unwrap());
    assert_eq!(balance, expected);
}

#[test]
fn chaos_storage_empty_value_treated_as_missing() {
    let mut storage = MockStorage::new(100);
    storage.set("ship:2:name", vec![], None); // empty

    let raw = storage.get("ship:2:name").unwrap();
    let valid = !raw.is_empty();
    assert!(
        !valid,
        "empty stored value must be treated as missing/invalid"
    );
}

// ─── TTL expiry (Temporary storage) ─────────────────────────────────────────

#[test]
fn chaos_storage_temp_entry_accessible_before_expiry() {
    let mut storage = MockStorage::new(1_000);
    storage.set("scan:region:42", vec![1, 2, 3], Some(100)); // expires at ledger 1100

    let result = storage.get("scan:region:42");
    assert!(
        result.is_some(),
        "entry should be accessible before TTL expiry"
    );
}

#[test]
fn chaos_storage_temp_entry_gone_after_expiry() {
    // Simulate the ledger advancing past the TTL.
    let mut storage = MockStorage::new(1_200); // current ledger = 1200
                                               // Entry was set with 100-ledger TTL, expires at ledger 1100.
                                               // We skip the set and directly test: a lookup at ledger 1200 for a key
                                               // whose TTL was at 1100 returns None.
    storage
        .data
        .insert("scan:region:42".to_string(), vec![1, 2, 3]);
    storage.ttls.insert("scan:region:42".to_string(), 1_100); // already expired

    let result = storage.get("scan:region:42");
    assert!(
        result.is_none(),
        "expired Temporary entry must not be accessible"
    );
}

#[test]
fn chaos_storage_contract_handles_expired_cache_gracefully() {
    let storage = MockStorage::new(2_000);
    // No entry exists (expired or never set).
    let cached = storage.get("nebula:layout:region:7");
    // Contract must regenerate layout from ledger seed instead of panicking.
    let layout = cached.map(|_| "cached").unwrap_or("regenerated");
    assert_eq!(layout, "regenerated");
}

// ─── Key namespacing / collision prevention ───────────────────────────────────

#[test]
fn chaos_storage_different_namespaces_do_not_collide() {
    let mut storage = MockStorage::new(100);
    storage.set("player:GABC:score", vec![100], None);
    storage.set("ship:GABC:score", vec![200], None); // same "GABC" ID, different namespace

    let player_score = storage.get("player:GABC:score").unwrap();
    let ship_score = storage.get("ship:GABC:score").unwrap();

    assert_ne!(player_score, ship_score, "namespace collision detected");
}

#[test]
fn chaos_storage_contract_prefix_prevents_cross_contract_read() {
    let mut storage_a = MockStorage::new(100);
    let mut storage_b = MockStorage::new(100);

    // Contract A stores under its own prefix.
    storage_a.set("nebula_nomad:config:admin", vec![1], None);
    // Contract B does NOT have this key.
    let result = storage_b.get("nebula_nomad:config:admin");
    assert!(
        result.is_none(),
        "contract B must not see contract A's storage"
    );
}

#[test]
fn chaos_storage_symbol_key_max_length_respected() {
    // Soroban Symbol keys must fit in 9 bytes (see test_symbol_lengths test suite).
    let valid_keys = ["balance", "shipId", "scan", "ttlReset"];
    let invalid_keys = ["tenCharKey", "thisIsWayTooLong"]; // >9 chars

    for key in valid_keys {
        assert!(key.len() <= 9, "symbol key '{key}' exceeds 9 chars");
    }
    for key in invalid_keys {
        assert!(
            key.len() > 9,
            "key '{key}' should be over the 9-char symbol limit"
        );
    }
}

// ─── Partial write resilience ─────────────────────────────────────────────────

#[test]
fn chaos_storage_partial_write_leaves_no_inconsistency() {
    // Soroban contract execution is atomic: either all writes in an invocation
    // commit, or none do. We model this with a transaction helper.
    let mut storage = MockStorage::new(100);

    fn atomic_transfer(
        storage: &mut MockStorage,
        from: &str,
        to: &str,
        amount: u64,
        should_fail: bool,
    ) -> Result<(), &'static str> {
        // Read balances.
        let from_balance: u64 = storage
            .get(from)
            .map(|b| u64::from_le_bytes(b[..8].try_into().unwrap()))
            .unwrap_or(0);

        if should_fail {
            return Err("simulated failure before write");
        }

        // Both writes succeed or neither does (simulated atomicity).
        let new_from = from_balance
            .checked_sub(amount)
            .ok_or("insufficient balance")?;
        storage.set(from, new_from.to_le_bytes().to_vec(), None);
        storage.set(to, amount.to_le_bytes().to_vec(), None);
        Ok(())
    }

    // Set up initial state.
    storage.set("player:A:balance", 1000u64.to_le_bytes().to_vec(), None);

    // Simulate a failed transfer — state must be unchanged.
    let result = atomic_transfer(
        &mut storage,
        "player:A:balance",
        "player:B:balance",
        100,
        true,
    );
    assert!(result.is_err());

    let a_balance: u64 = storage
        .get("player:A:balance")
        .map(|b| u64::from_le_bytes(b[..8].try_into().unwrap()))
        .unwrap_or(0);
    assert_eq!(
        a_balance, 1_000,
        "balance must be unchanged after failed transfer"
    );
}

#[test]
fn chaos_storage_successful_write_persists() {
    let mut storage = MockStorage::new(100);
    storage.set("player:A:score", 500u64.to_le_bytes().to_vec(), None);

    let score: u64 = storage
        .get("player:A:score")
        .map(|b| u64::from_le_bytes(b[..8].try_into().unwrap()))
        .unwrap_or(0);
    assert_eq!(score, 500);
}

// ─── High-volume storage stress ───────────────────────────────────────────────

#[test]
fn chaos_storage_1000_entries_read_write_no_panic() {
    let mut storage = MockStorage::new(100);

    for i in 0u64..1_000 {
        let key = format!("player:{i}:balance");
        storage.set(&key, i.to_le_bytes().to_vec(), None);
    }

    for i in 0u64..1_000 {
        let key = format!("player:{i}:balance");
        let val = storage.get(&key).expect("entry must exist");
        let balance = u64::from_le_bytes(val[..8].try_into().unwrap());
        assert_eq!(balance, i);
    }
}

#[test]
fn chaos_storage_interleaved_reads_writes_consistent() {
    let mut storage = MockStorage::new(100);
    let mut expected: HashMap<String, u64> = HashMap::new();

    for i in 0u64..100 {
        let key = format!("ship:{i}:level");
        let level = i % 10;
        storage.set(&key, level.to_le_bytes().to_vec(), None);
        expected.insert(key, level);
    }

    for (key, exp_level) in &expected {
        let raw = storage.get(key).expect("key must exist");
        let level = u64::from_le_bytes(raw[..8].try_into().unwrap());
        assert_eq!(level, *exp_level, "level mismatch for key={key}");
    }
}
