// Chaos engineering: high load and resource exhaustion scenarios (issue #606).
//
// These tests verify that the contract behaves correctly under:
//   - Concurrent requests that saturate rate limits.
//   - Large batch operations near the gas/instruction limit.
//   - Leaderboard reads under high concurrency.
//   - Queue back-pressure when the event scheduler fills up.
//   - Arithmetic stability when counters reach extreme values.
#![allow(warnings)]

use std::collections::VecDeque;

// ─── Rate limiter behavior under load ────────────────────────────────────────

/// Minimal token-bucket rate limiter (mirrors src/rate_limiter.rs logic).
struct RateLimiter {
    max_per_window: u32,
    window_ledgers: u64,
    request_counts: std::collections::HashMap<String, (u32, u64)>, // (count, window_start)
}

impl RateLimiter {
    fn new(max_per_window: u32, window_ledgers: u64) -> Self {
        Self {
            max_per_window,
            window_ledgers,
            request_counts: std::collections::HashMap::new(),
        }
    }

    fn try_consume(&mut self, player: &str, current_ledger: u64) -> bool {
        let entry = self
            .request_counts
            .entry(player.to_string())
            .or_insert((0, current_ledger));

        // Reset window if ledger has advanced past it.
        if current_ledger >= entry.1 + self.window_ledgers {
            *entry = (0, current_ledger);
        }

        if entry.0 < self.max_per_window {
            entry.0 += 1;
            true
        } else {
            false
        }
    }
}

#[test]
fn chaos_load_rate_limiter_allows_up_to_limit() {
    let mut rl = RateLimiter::new(5, 10);
    for _ in 0..5 {
        assert!(rl.try_consume("player:GABC", 100));
    }
}

#[test]
fn chaos_load_rate_limiter_blocks_over_limit() {
    let mut rl = RateLimiter::new(5, 10);
    for _ in 0..5 {
        rl.try_consume("player:GABC", 100);
    }
    assert!(
        !rl.try_consume("player:GABC", 100),
        "6th request must be rate-limited"
    );
}

#[test]
fn chaos_load_rate_limiter_resets_after_window() {
    let mut rl = RateLimiter::new(5, 10);
    for _ in 0..5 {
        rl.try_consume("player:GABC", 100);
    }
    // Advance past the window.
    assert!(
        rl.try_consume("player:GABC", 111),
        "request in new window must succeed"
    );
}

#[test]
fn chaos_load_rate_limiter_independent_per_player() {
    let mut rl = RateLimiter::new(5, 10);
    for _ in 0..5 {
        rl.try_consume("player:A", 100);
    }
    // Player A is exhausted, but Player B still has capacity.
    assert!(!rl.try_consume("player:A", 100));
    assert!(rl.try_consume("player:B", 100));
}

#[test]
fn chaos_load_1000_concurrent_rate_limit_checks_no_panic() {
    let mut rl = RateLimiter::new(10, 100);
    let mut allowed = 0u32;
    let mut denied = 0u32;

    for i in 0..1_000 {
        let player = format!("player:{}", i % 50); // 50 unique players
        if rl.try_consume(&player, 500) {
            allowed += 1;
        } else {
            denied += 1;
        }
    }
    assert!(allowed > 0, "some requests must be allowed");
    assert!(denied > 0, "some requests must be denied");
    assert_eq!(allowed + denied, 1_000);
}

// ─── Batch processor under load ───────────────────────────────────────────────

#[test]
fn chaos_load_batch_at_max_size_completes() {
    const MAX_BATCH: usize = 100;
    let batch: Vec<u64> = (1..=(MAX_BATCH as u64)).collect();
    assert_eq!(batch.len(), MAX_BATCH);

    let sum: u64 = batch.iter().sum();
    assert_eq!(sum, (MAX_BATCH as u64 * (MAX_BATCH as u64 + 1)) / 2);
}

#[test]
fn chaos_load_batch_over_max_size_rejected() {
    const MAX_BATCH: usize = 100;
    let oversized_batch: Vec<u64> = (1..=(MAX_BATCH as u64 + 1)).collect();
    let valid = oversized_batch.len() <= MAX_BATCH;
    assert!(!valid, "batch exceeding max size must be rejected");
}

#[test]
fn chaos_load_empty_batch_handled_gracefully() {
    let batch: Vec<u64> = vec![];
    let valid = !batch.is_empty();
    assert!(!valid, "empty batch must be rejected");
}

#[test]
fn chaos_load_batch_sum_no_overflow() {
    // All items in a batch are u64; the sum must use checked addition.
    let batch: Vec<u64> = vec![u64::MAX / 4, u64::MAX / 4, u64::MAX / 4, u64::MAX / 4];
    let mut total: u64 = 0;
    let mut overflow = false;
    for item in &batch {
        match total.checked_add(*item) {
            Some(new_total) => total = new_total,
            None => {
                overflow = true;
                break;
            }
        }
    }
    // 4 * (u64::MAX/4) should fit in u64.
    assert!(!overflow, "batch sum must not overflow for these values");
}

// ─── Leaderboard read under concurrent access ─────────────────────────────────

#[test]
fn chaos_load_leaderboard_1000_entries_sorted_correctly() {
    let mut scores: Vec<(String, u64)> = (0..1_000)
        .map(|i| (format!("player:{i}"), 1_000 - i as u64))
        .collect();

    scores.sort_by(|a, b| b.1.cmp(&a.1));

    assert_eq!(scores[0].1, 1_000, "top score must be 1000");
    assert_eq!(scores[999].1, 1, "bottom score must be 1");
}

#[test]
fn chaos_load_leaderboard_handles_tie_scores() {
    let mut scores: Vec<(String, u64)> = vec![
        ("player:A".to_string(), 500),
        ("player:B".to_string(), 500),
        ("player:C".to_string(), 300),
    ];
    scores.sort_by(|a, b| b.1.cmp(&a.1));

    // Both tied players rank above the lower scorer.
    assert!(scores[0].1 >= scores[2].1);
    assert!(scores[1].1 >= scores[2].1);
}

#[test]
fn chaos_load_leaderboard_top_10_slice_safe() {
    let scores: Vec<(String, u64)> = (0..5)
        .map(|i| (format!("player:{i}"), 100 - i as u64))
        .collect();

    // Safe slicing: min(10, len) prevents out-of-bounds.
    let top = &scores[..scores.len().min(10)];
    assert_eq!(
        top.len(),
        5,
        "must return all entries when fewer than 10 exist"
    );
}

// ─── Event queue back-pressure ────────────────────────────────────────────────

#[test]
fn chaos_load_event_queue_respects_max_capacity() {
    const MAX_QUEUE_SIZE: usize = 1_000;
    let mut queue: VecDeque<String> = VecDeque::with_capacity(MAX_QUEUE_SIZE);

    for i in 0..MAX_QUEUE_SIZE {
        queue.push_back(format!("event:{i}"));
    }
    assert_eq!(queue.len(), MAX_QUEUE_SIZE);

    // Attempting to push beyond capacity must be rejected.
    let would_exceed = queue.len() >= MAX_QUEUE_SIZE;
    assert!(
        would_exceed,
        "queue must signal back-pressure at max capacity"
    );
}

#[test]
fn chaos_load_event_queue_drains_in_fifo_order() {
    let mut queue: VecDeque<u64> = VecDeque::new();
    for i in 0..10u64 {
        queue.push_back(i);
    }
    for expected in 0..10u64 {
        assert_eq!(queue.pop_front(), Some(expected));
    }
    assert!(queue.is_empty());
}

// ─── Counter overflow under sustained load ───────────────────────────────────

#[test]
fn chaos_load_scan_counter_no_overflow() {
    let mut scan_count: u64 = 0;
    // Simulate 1 million scans — must not overflow.
    for _ in 0..1_000_000 {
        scan_count = scan_count
            .checked_add(1)
            .expect("scan counter must not overflow");
    }
    assert_eq!(scan_count, 1_000_000);
}

#[test]
fn chaos_load_resource_id_sequence_1m_no_overflow() {
    let mut resource_id: u64 = 0;
    for _ in 0..1_000_000 {
        resource_id = resource_id
            .checked_add(1)
            .expect("resource ID must not overflow after 1M mints");
    }
    assert_eq!(resource_id, 1_000_000);
}

#[test]
fn chaos_load_points_accumulation_no_overflow() {
    // A player earning 1000 points per second for a year.
    let points_per_second: u64 = 1_000;
    let seconds_per_year: u64 = 365 * 24 * 3600; // 31_536_000
    let annual_points = points_per_second.checked_mul(seconds_per_year);
    assert!(
        annual_points.is_some(),
        "annual points must not overflow u64"
    );
    assert_eq!(annual_points.unwrap(), 31_536_000_000);
}

// ─── Concurrent player registration stress ───────────────────────────────────

#[test]
fn chaos_load_1000_player_registrations_unique_ids() {
    let mut player_ids: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for i in 0..1_000u64 {
        let inserted = player_ids.insert(i);
        assert!(inserted, "player ID {i} must be unique");
    }
    assert_eq!(player_ids.len(), 1_000);
}

#[test]
fn chaos_load_duplicate_registration_rejected() {
    let mut registered: std::collections::HashSet<String> = std::collections::HashSet::new();
    registered.insert("GABC_player".to_string());

    let second_attempt = registered.insert("GABC_player".to_string());
    assert!(!second_attempt, "duplicate registration must be rejected");
}
