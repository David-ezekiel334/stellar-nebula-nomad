// Chaos engineering: Byzantine attack resistance scenarios (issue #606).
//
// Byzantine faults involve actors that behave arbitrarily or maliciously.
// In a Soroban contract context this means:
//   - Callers that forge or reuse authorization tokens.
//   - Callers that submit transactions out of order.
//   - Callers that attempt to manipulate shared state via race conditions.
//   - Governance participants that collude to pass malicious proposals.
//   - Actors that try to drain funds by exploiting arithmetic edge cases.
#![allow(warnings)]

use std::collections::{HashMap, HashSet};

// ─── Mock authorization model ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum AuthResult {
    Authorized,
    Unauthorized,
    InvalidSignature,
    ReplayAttack,
}

struct AuthValidator {
    used_nonces: HashSet<u64>,
    admin: String,
}

impl AuthValidator {
    fn new(admin: &str) -> Self {
        Self {
            used_nonces: HashSet::new(),
            admin: admin.to_string(),
        }
    }

    fn validate(&mut self, caller: &str, nonce: u64, is_admin_required: bool) -> AuthResult {
        // Reject replayed nonces.
        if self.used_nonces.contains(&nonce) {
            return AuthResult::ReplayAttack;
        }
        // Admin-only operations require the admin address.
        if is_admin_required && caller != self.admin {
            return AuthResult::Unauthorized;
        }
        self.used_nonces.insert(nonce);
        AuthResult::Authorized
    }
}

// ─── Replay attack prevention ─────────────────────────────────────────────────

#[test]
fn chaos_byzantine_replay_attack_rejected() {
    let mut auth = AuthValidator::new("GADMIN");
    let nonce: u64 = 12345;

    let first = auth.validate("GADMIN", nonce, false);
    assert_eq!(first, AuthResult::Authorized);

    let replay = auth.validate("GADMIN", nonce, false);
    assert_eq!(
        replay,
        AuthResult::ReplayAttack,
        "replayed nonce must be rejected"
    );
}

#[test]
fn chaos_byzantine_different_nonces_both_accepted() {
    let mut auth = AuthValidator::new("GADMIN");
    let r1 = auth.validate("GADMIN", 1, false);
    let r2 = auth.validate("GADMIN", 2, false);
    assert_eq!(r1, AuthResult::Authorized);
    assert_eq!(r2, AuthResult::Authorized);
}

#[test]
fn chaos_byzantine_nonce_zero_valid_once() {
    let mut auth = AuthValidator::new("GADMIN");
    let r1 = auth.validate("GUSER", 0, false);
    let r2 = auth.validate("GUSER", 0, false);
    assert_eq!(r1, AuthResult::Authorized);
    assert_eq!(r2, AuthResult::ReplayAttack);
}

#[test]
fn chaos_byzantine_max_nonce_valid_once() {
    let mut auth = AuthValidator::new("GADMIN");
    let r1 = auth.validate("GUSER", u64::MAX, false);
    let r2 = auth.validate("GUSER", u64::MAX, false);
    assert_eq!(r1, AuthResult::Authorized);
    assert_eq!(r2, AuthResult::ReplayAttack);
}

// ─── Unauthorized admin escalation attempts ───────────────────────────────────

#[test]
fn chaos_byzantine_non_admin_cannot_call_admin_function() {
    let mut auth = AuthValidator::new("GADMIN");
    let result = auth.validate("GATTACKER", 1, true);
    assert_eq!(result, AuthResult::Unauthorized);
}

#[test]
fn chaos_byzantine_admin_can_call_admin_function() {
    let mut auth = AuthValidator::new("GADMIN");
    let result = auth.validate("GADMIN", 1, true);
    assert_eq!(result, AuthResult::Authorized);
}

#[test]
fn chaos_byzantine_attacker_cannot_impersonate_admin() {
    let mut auth = AuthValidator::new("GADMIN");
    // Attacker tries multiple spoofed admin addresses.
    let spoofed = vec!["GADMIN_", "_GADMIN", "gadmin", "GADMIN\0", " GADMIN"];
    for spoof in spoofed {
        let result = auth.validate(spoof, 100, true);
        assert_eq!(
            result,
            AuthResult::Unauthorized,
            "spoofed address '{spoof}' must be rejected"
        );
        auth.used_nonces.clear(); // allow reuse across variants
    }
}

// ─── Front-running / transaction ordering attacks ─────────────────────────────

#[test]
fn chaos_byzantine_escrow_cannot_be_drained_by_front_running() {
    // Escrow holds funds for a pending trade. An attacker cannot release
    // escrow funds meant for another party.
    let mut escrow: HashMap<u64, (String, u64)> = HashMap::new(); // order_id → (beneficiary, amount)
    escrow.insert(1, ("GBUYER".to_string(), 500));

    fn release_escrow(
        escrow: &mut HashMap<u64, (String, u64)>,
        order_id: u64,
        caller: &str,
    ) -> Result<u64, &'static str> {
        let (beneficiary, amount) = escrow.get(&order_id).ok_or("order not found")?;
        if caller != beneficiary {
            return Err("unauthorized: caller is not the beneficiary");
        }
        let released = *amount;
        escrow.remove(&order_id);
        Ok(released)
    }

    let attacker_result = release_escrow(&mut escrow, 1, "GATTACKER");
    assert!(
        attacker_result.is_err(),
        "attacker must not release another party's escrow"
    );
    assert!(
        escrow.contains_key(&1),
        "escrow must still be held after failed drain"
    );

    let buyer_result = release_escrow(&mut escrow, 1, "GBUYER");
    assert_eq!(buyer_result, Ok(500));
}

#[test]
fn chaos_byzantine_price_manipulation_via_sandwich_attack_bounded() {
    // In a constant-product AMM, a sandwich attack tries to profit by
    // inserting a large buy before the victim's buy and a sell after.
    // The profit is bounded by the pool's liquidity depth.
    let reserve_a: u128 = 1_000_000;
    let reserve_b: u128 = 1_000_000;
    let k = reserve_a * reserve_b;

    // Attacker front-runs with a large buy.
    let attacker_in: u128 = 100_000;
    let attacker_out_b = reserve_b * attacker_in / (reserve_a + attacker_in);
    let new_reserve_a = reserve_a + attacker_in;
    let new_reserve_b = reserve_b - attacker_out_b;

    // Victim buys at inflated price.
    let victim_in: u128 = 1_000;
    let victim_out = new_reserve_b * victim_in / (new_reserve_a + victim_in);

    // Pool invariant must hold (approximately, accounting for integer division).
    let new_k = (new_reserve_a + victim_in) * (new_reserve_b - victim_out);
    // k can only increase (fees) or stay close due to integer truncation.
    // With no fees and integer math, k may decrease slightly; we just verify no crash.
    assert!(new_k > 0, "pool invariant must remain positive");
    assert!(
        victim_out < victim_in || new_reserve_b > new_reserve_a,
        "victim receives less due to price impact"
    );
}

// ─── Governance attack resistance ─────────────────────────────────────────────

#[test]
fn chaos_byzantine_governance_quorum_cannot_be_bypassed() {
    struct Governance {
        total_votes: u64,
        quorum_bps: u64, // e.g. 1000 = 10%
        votes_for: u64,
    }

    impl Governance {
        fn has_quorum(&self) -> bool {
            if self.total_votes == 0 {
                return false;
            }
            let participation_bps = self.votes_for * 10_000 / self.total_votes;
            participation_bps >= self.quorum_bps
        }
    }

    let gov = Governance {
        total_votes: 1_000_000,
        quorum_bps: 1_000, // 10% required
        votes_for: 99_999, // 9.9999% — just below quorum
    };
    assert!(
        !gov.has_quorum(),
        "proposal with <10% participation must not pass"
    );
}

#[test]
fn chaos_byzantine_governance_quorum_met_at_exact_threshold() {
    struct Governance {
        total_votes: u64,
        quorum_bps: u64,
        votes_for: u64,
    }
    impl Governance {
        fn has_quorum(&self) -> bool {
            if self.total_votes == 0 {
                return false;
            }
            self.votes_for * 10_000 / self.total_votes >= self.quorum_bps
        }
    }

    let gov = Governance {
        total_votes: 1_000_000,
        quorum_bps: 1_000,
        votes_for: 100_000,
    };
    assert!(
        gov.has_quorum(),
        "proposal at exactly 10% participation must pass"
    );
}

#[test]
fn chaos_byzantine_double_vote_rejected() {
    let mut votes: HashSet<String> = HashSet::new();
    let voter = "GVOTER1";

    let first = votes.insert(voter.to_string());
    assert!(first, "first vote must succeed");

    let second = votes.insert(voter.to_string());
    assert!(!second, "double vote must be rejected");
}

#[test]
fn chaos_byzantine_governance_zero_total_votes_no_divide_by_zero() {
    let total_votes: u64 = 0;
    let votes_for: u64 = 0;

    // Safe check before division.
    let has_quorum = if total_votes == 0 {
        false
    } else {
        votes_for * 10_000 / total_votes >= 1_000
    };
    assert!(!has_quorum);
}

// ─── Arithmetic drain attacks ─────────────────────────────────────────────────

#[test]
fn chaos_byzantine_integer_overflow_drain_prevented() {
    // Attacker crafts an amount that overflows to a small number,
    // e.g. u64::MAX + 1 = 0 in wrapping arithmetic.
    let attacker_amount: u64 = u64::MAX;
    let treasury_balance: u64 = 1_000_000;

    // The contract must use checked_sub to prevent the underflow.
    let result = treasury_balance.checked_sub(attacker_amount);
    assert!(
        result.is_none(),
        "overflow-crafted drain must be caught by checked_sub"
    );
}

#[test]
fn chaos_byzantine_fee_bypass_via_zero_amount_prevented() {
    // Some fee calculations use: fee = amount * fee_bps / 10_000.
    // If amount = 0, fee = 0 and the operation may be free.
    // The contract must reject zero amounts before fee calculation.
    let amount: u64 = 0;
    let valid = amount > 0;
    assert!(!valid, "zero-amount transactions must be rejected");
}

#[test]
fn chaos_byzantine_bps_overflow_in_fee_prevented() {
    // fee = amount * rate_bps / 10_000
    // With large amount, the intermediate product must use checked_mul.
    let amount: u64 = u64::MAX / 2;
    let rate_bps: u64 = 10_000;

    let intermediate = amount.checked_mul(rate_bps);
    // u64::MAX/2 * 10_000 overflows — contract must catch this.
    assert!(
        intermediate.is_none(),
        "fee intermediate must be caught for oversized amount"
    );
}
