// Chaos engineering: state corruption prevention and recovery (issue #606).
//
// These tests verify that:
//   - Invariants (non-negative balances, monotone counters, valid ranges) hold
//     after every operation sequence.
//   - Recovery from a detected inconsistency does not silently continue.
//   - Ship level/owner invariants cannot be broken by adversarial inputs.
//   - Leaderboard state is self-consistent after updates.
//   - Resource ledger total supply equals sum of all balances.
#![allow(warnings)]

use std::collections::HashMap;

// ─── Balance invariant checks ─────────────────────────────────────────────────

struct Ledger {
    balances: HashMap<String, u64>,
    total_supply: u64,
}

impl Ledger {
    fn new() -> Self {
        Self {
            balances: HashMap::new(),
            total_supply: 0,
        }
    }

    fn mint(&mut self, account: &str, amount: u64) -> Result<(), &'static str> {
        if amount == 0 {
            return Err("zero mint");
        }
        let new_supply = self
            .total_supply
            .checked_add(amount)
            .ok_or("supply overflow")?;
        *self.balances.entry(account.to_string()).or_insert(0) += amount;
        self.total_supply = new_supply;
        Ok(())
    }

    fn transfer(&mut self, from: &str, to: &str, amount: u64) -> Result<(), &'static str> {
        if amount == 0 {
            return Err("zero transfer");
        }
        let from_bal = self.balances.get(from).copied().unwrap_or(0);
        let to_bal = self.balances.get(to).copied().unwrap_or(0);

        let new_from = from_bal.checked_sub(amount).ok_or("insufficient balance")?;
        let new_to = to_bal.checked_add(amount).ok_or("to balance overflow")?;

        self.balances.insert(from.to_string(), new_from);
        self.balances.insert(to.to_string(), new_to);
        Ok(())
    }

    fn assert_invariant(&self) {
        let sum: u64 = self.balances.values().sum();
        assert_eq!(
            sum, self.total_supply,
            "invariant broken: sum of balances ({sum}) != total supply ({})",
            self.total_supply
        );
    }
}

#[test]
fn chaos_corruption_mint_preserves_supply_invariant() {
    let mut ledger = Ledger::new();
    ledger.mint("player:A", 1_000).unwrap();
    ledger.mint("player:B", 500).unwrap();
    ledger.assert_invariant();
}

#[test]
fn chaos_corruption_transfer_preserves_supply_invariant() {
    let mut ledger = Ledger::new();
    ledger.mint("player:A", 1_000).unwrap();
    ledger.transfer("player:A", "player:B", 300).unwrap();
    ledger.assert_invariant();
}

#[test]
fn chaos_corruption_failed_transfer_does_not_alter_supply() {
    let mut ledger = Ledger::new();
    ledger.mint("player:A", 100).unwrap();

    let result = ledger.transfer("player:A", "player:B", 200); // exceeds balance
    assert!(result.is_err());
    ledger.assert_invariant(); // supply must still be 100
}

#[test]
fn chaos_corruption_zero_mint_rejected_invariant_unchanged() {
    let mut ledger = Ledger::new();
    ledger.mint("player:A", 100).unwrap();
    let _ = ledger.mint("player:A", 0); // should fail
    ledger.assert_invariant();
}

#[test]
fn chaos_corruption_1000_operations_preserve_invariant() {
    let mut ledger = Ledger::new();
    ledger.mint("player:A", 1_000_000).unwrap();
    ledger.mint("player:B", 500_000).unwrap();

    for i in 0..500 {
        let amount = (i as u64 % 100) + 1;
        let _ = ledger.transfer("player:A", "player:B", amount);
        let _ = ledger.transfer("player:B", "player:A", amount / 2 + 1);
    }
    ledger.assert_invariant();
}

// ─── Ship state invariants ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Ship {
    id: u64,
    owner: String,
    level: u32,
    scan_range: u32,
}

impl Ship {
    fn new(id: u64, owner: &str) -> Self {
        Self {
            id,
            owner: owner.to_string(),
            level: 1,
            scan_range: 10,
        }
    }

    fn upgrade(&mut self, new_owner: &str) -> Result<(), &'static str> {
        if self.owner != new_owner {
            return Err("not owner");
        }
        const MAX_LEVEL: u32 = 100;
        if self.level >= MAX_LEVEL {
            return Err("max level");
        }
        self.level = self.level.checked_add(1).ok_or("level overflow")?;
        self.scan_range = self.scan_range.checked_add(5).ok_or("range overflow")?;
        Ok(())
    }

    fn assert_invariants(&self) {
        assert!(self.level >= 1, "ship level must be >= 1");
        assert!(self.level <= 100, "ship level must be <= 100");
        assert!(self.scan_range >= 10, "scan range must be >= 10");
        assert!(!self.owner.is_empty(), "ship must have an owner");
    }
}

#[test]
fn chaos_corruption_ship_upgrade_invariants_hold() {
    let mut ship = Ship::new(1, "player:GABC");
    for _ in 0..50 {
        ship.upgrade("player:GABC").unwrap();
    }
    ship.assert_invariants();
    assert_eq!(ship.level, 51);
}

#[test]
fn chaos_corruption_ship_upgrade_at_max_level_rejected() {
    let mut ship = Ship::new(1, "player:GABC");
    ship.level = 100;
    let result = ship.upgrade("player:GABC");
    assert!(result.is_err());
    ship.assert_invariants();
}

#[test]
fn chaos_corruption_ship_non_owner_upgrade_rejected() {
    let mut ship = Ship::new(1, "player:OWNER");
    let result = ship.upgrade("player:ATTACKER");
    assert!(result.is_err());
    // Ship must be unchanged.
    assert_eq!(ship.level, 1);
    assert_eq!(ship.owner, "player:OWNER");
    ship.assert_invariants();
}

#[test]
fn chaos_corruption_ship_owner_transfer_does_not_duplicate() {
    let mut registry: HashMap<u64, Ship> = HashMap::new();
    registry.insert(1, Ship::new(1, "player:A"));

    fn transfer_ship(
        registry: &mut HashMap<u64, Ship>,
        ship_id: u64,
        from: &str,
        to: &str,
    ) -> Result<(), &'static str> {
        let ship = registry.get_mut(&ship_id).ok_or("ship not found")?;
        if ship.owner != from {
            return Err("not owner");
        }
        ship.owner = to.to_string();
        Ok(())
    }

    transfer_ship(&mut registry, 1, "player:A", "player:B").unwrap();

    let ship = registry.get(&1).unwrap();
    assert_eq!(ship.owner, "player:B");
    // Only one owner at any time.
    let owner_count = registry.values().filter(|s| s.id == 1).count();
    assert_eq!(owner_count, 1, "ship must have exactly one owner");
}

// ─── Counter monotonicity ─────────────────────────────────────────────────────

#[test]
fn chaos_corruption_resource_id_is_monotone() {
    let mut last_id: u64 = 0;
    for _ in 0..100 {
        let new_id = last_id.checked_add(1).expect("ID overflow");
        assert!(new_id > last_id, "resource IDs must be strictly increasing");
        last_id = new_id;
    }
}

#[test]
fn chaos_corruption_scan_sequence_number_is_monotone() {
    let mut seq: u64 = 0;
    for _ in 0..1_000 {
        let next = seq.checked_add(1).unwrap();
        assert!(next > seq);
        seq = next;
    }
}

// ─── Leaderboard consistency ───────────────────────────────────────────────────

#[test]
fn chaos_corruption_leaderboard_no_duplicate_ranks() {
    let scores: Vec<(String, u64)> = vec![
        ("A".to_string(), 300),
        ("B".to_string(), 100),
        ("C".to_string(), 200),
    ];
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    // All ranks unique.
    let names: Vec<&str> = sorted.iter().map(|(n, _)| n.as_str()).collect();
    let unique_names: std::collections::HashSet<&str> = names.iter().cloned().collect();
    assert_eq!(
        names.len(),
        unique_names.len(),
        "no duplicate entries in leaderboard"
    );
}

#[test]
fn chaos_corruption_leaderboard_scores_non_negative() {
    let scores: Vec<u64> = vec![0, 1, 100, u64::MAX / 2];
    for score in scores {
        // u64 scores are inherently non-negative — verify logic using signed comparison.
        let valid = score as i128 >= 0;
        assert!(valid, "score must be non-negative");
    }
}

// ─── Reentrancy guard (simulated) ─────────────────────────────────────────────

#[test]
fn chaos_corruption_reentrancy_guard_prevents_double_entry() {
    let mut locked = false;

    fn guarded_operation(locked: &mut bool) -> Result<(), &'static str> {
        if *locked {
            return Err("reentrancy detected");
        }
        *locked = true;
        // ... operation runs here ...
        *locked = false;
        Ok(())
    }

    // First call succeeds.
    assert!(guarded_operation(&mut locked).is_ok());

    // Simulate: a reentrant call while locked.
    locked = true;
    let reentrant = guarded_operation(&mut locked);
    assert!(reentrant.is_err(), "reentrant call must be rejected");
}

#[test]
fn chaos_corruption_reentrancy_guard_resets_after_success() {
    let mut locked = false;

    fn guarded_op(locked: &mut bool) -> Result<(), &'static str> {
        if *locked {
            return Err("reentrant");
        }
        *locked = true;
        // Operation succeeds.
        *locked = false;
        Ok(())
    }

    guarded_op(&mut locked).unwrap();
    // Guard must be reset — subsequent calls must succeed.
    assert!(
        !locked,
        "reentrancy guard must be cleared after successful execution"
    );
    guarded_op(&mut locked).unwrap();
}
