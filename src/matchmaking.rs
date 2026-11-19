/// MMR-Based Matchmaking System — Issue #591
///
/// Pairs players by ELO/MMR within ±100 initial range, expanding by 50 every
/// 30 seconds up to a 2-minute max wait. Handles queue abandonment (auto-
/// remove on timeout), abuse prevention (rate-limit re-queue), and
/// ranked/casual/team game modes.
use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol, Vec};

use crate::pvp_combat::{get_elo_rating, PvPDataKey, PvPError, MAX_QUEUE_SIZE};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Initial MMR search range (±points).
pub const MMR_INITIAL_RANGE: u32 = 100;
/// Range expansion per tick (every EXPAND_INTERVAL_SECS seconds).
pub const MMR_EXPAND_STEP: u32 = 50;
/// How often (seconds) the range expands.
pub const EXPAND_INTERVAL_SECS: u64 = 30;
/// Maximum wait before the queue entry is abandoned (2 minutes).
pub const MAX_QUEUE_WAIT_SECS: u64 = 120;
/// Minimum seconds between re-queue attempts (abuse prevention).
pub const REQUEUE_COOLDOWN_SECS: u64 = 10;
/// Maximum team size for team mode.
pub const MAX_TEAM_SIZE: u32 = 3;

// ── Game Mode ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum GameMode {
    /// Ranked 1v1 — ELO changes apply.
    Ranked,
    /// Casual 1v1 — no ELO changes.
    Casual,
    /// Team battle 3v3 — ELO changes apply to team average.
    Team,
}

// ── Data Types ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub player: Address,
    pub mmr: u32,
    pub queued_at: u64,
    pub mode: GameMode,
    /// Current search range at the time of last expansion check.
    pub current_range: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct MatchResult {
    pub player1: Address,
    pub player2: Address,
    pub mode: GameMode,
    pub mmr_diff: u32,
    pub wait_secs: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum MatchmakingKey {
    /// Per-mode queue.
    Queue(Symbol),
    /// Last leave timestamp for abuse prevention.
    LastLeave(Address),
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mode_symbol(mode: &GameMode) -> Symbol {
    match mode {
        GameMode::Ranked => symbol_short!("ranked"),
        GameMode::Casual => symbol_short!("casual"),
        GameMode::Team => symbol_short!("team"),
    }
}

fn queue_key(mode: &GameMode) -> MatchmakingKey {
    MatchmakingKey::Queue(mode_symbol(mode))
}

fn load_queue(env: &Env, mode: &GameMode) -> Vec<QueueEntry> {
    env.storage()
        .persistent()
        .get(&queue_key(mode))
        .unwrap_or_else(|| Vec::new(env))
}

fn save_queue(env: &Env, mode: &GameMode, queue: &Vec<QueueEntry>) {
    env.storage().persistent().set(&queue_key(mode), queue);
}

/// Compute effective MMR search range given how long the player has waited.
fn effective_range(queued_at: u64, now: u64) -> u32 {
    let waited = now.saturating_sub(queued_at);
    let ticks = (waited / EXPAND_INTERVAL_SECS) as u32;
    MMR_INITIAL_RANGE.saturating_add(ticks.saturating_mul(MMR_EXPAND_STEP))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Add a player to the matchmaking queue for the given mode.
///
/// Returns `QueueFull` if the mode queue is full, `AlreadyInCombat` if
/// already queued, or `InvalidCombatParams` if on requeue cooldown.
pub fn enqueue(env: &Env, player: &Address, mode: GameMode) -> Result<(), PvPError> {
    player.require_auth();

    let now = env.ledger().timestamp();

    // Abuse prevention: enforce requeue cooldown
    let last_leave_key = MatchmakingKey::LastLeave(player.clone());
    if let Some(last_leave) = env.storage().persistent().get::<_, u64>(&last_leave_key) {
        if now.saturating_sub(last_leave) < REQUEUE_COOLDOWN_SECS {
            return Err(PvPError::InvalidCombatParams);
        }
    }

    let mut queue = load_queue(env, &mode);

    if queue.len() >= MAX_QUEUE_SIZE {
        return Err(PvPError::QueueFull);
    }

    // Duplicate check
    for i in 0..queue.len() {
        if let Some(e) = queue.get(i) {
            if e.player == *player {
                return Err(PvPError::AlreadyInCombat);
            }
        }
    }

    let mmr = get_elo_rating(env, player);
    queue.push_back(QueueEntry {
        player: player.clone(),
        mmr,
        queued_at: now,
        mode: mode.clone(),
        current_range: MMR_INITIAL_RANGE,
    });

    save_queue(env, &mode, &queue);

    env.events().publish(
        (symbol_short!("mm"), symbol_short!("enqueue")),
        (player.clone(), mmr, mode_symbol(&mode)),
    );

    Ok(())
}

/// Remove a player from the queue voluntarily (records leave time for
/// abuse-prevention cooldown).
pub fn dequeue(env: &Env, player: &Address, mode: GameMode) -> Result<(), PvPError> {
    player.require_auth();

    let mut queue = load_queue(env, &mode);
    let mut found = false;
    let mut new_queue = Vec::new(env);

    for i in 0..queue.len() {
        if let Some(e) = queue.get(i) {
            if e.player == *player {
                found = true;
            } else {
                new_queue.push_back(e);
            }
        }
    }

    if !found {
        return Err(PvPError::NotInQueue);
    }

    save_queue(env, &mode, &new_queue);

    // Record leave time for requeue cooldown
    let now = env.ledger().timestamp();
    env.storage()
        .persistent()
        .set(&MatchmakingKey::LastLeave(player.clone()), &now);

    env.events().publish(
        (symbol_short!("mm"), symbol_short!("dequeue")),
        (player.clone(), mode_symbol(&mode)),
    );

    Ok(())
}

/// Scan the queue for the best match for the front-of-queue player,
/// respecting MMR range (which expands over time). Removes timed-out entries
/// first. Returns `Some(MatchResult)` when a pair is found.
///
/// This is designed to be called by a cron/admin transaction each ledger
/// close. On-chain, it matches at most one pair per call to stay within gas.
pub fn process_queue(env: &Env, mode: GameMode) -> Result<Option<MatchResult>, PvPError> {
    let now = env.ledger().timestamp();
    let mut queue = load_queue(env, &mode);

    // ── 1. Prune timed-out entries ────────────────────────────────────────
    let mut active = Vec::new(env);
    for i in 0..queue.len() {
        if let Some(e) = queue.get(i) {
            let waited = now.saturating_sub(e.queued_at);
            if waited <= MAX_QUEUE_WAIT_SECS {
                active.push_back(e.clone());
            } else {
                // Emit abandonment event
                env.events().publish(
                    (symbol_short!("mm"), symbol_short!("abandon")),
                    (e.player.clone(), waited),
                );
                // Record leave time so they can re-queue after cooldown
                env.storage()
                    .persistent()
                    .set(&MatchmakingKey::LastLeave(e.player.clone()), &now);
            }
        }
    }

    if active.len() < 2 {
        save_queue(env, &mode, &active);
        return Ok(None);
    }

    // ── 2. Find best match for the oldest queued player ───────────────────
    let seeker = active.get(0).ok_or(PvPError::NotInQueue)?.clone();
    let range = effective_range(seeker.queued_at, now);
    let seeker_mmr = seeker.mmr;

    let mut best_idx: Option<u32> = None;
    let mut best_diff = u32::MAX;

    for i in 1..active.len() {
        if let Some(candidate) = active.get(i) {
            let diff = if candidate.mmr >= seeker_mmr {
                candidate.mmr - seeker_mmr
            } else {
                seeker_mmr - candidate.mmr
            };

            if diff <= range && diff < best_diff {
                best_diff = diff;
                best_idx = Some(i);
            }
        }
    }

    // ── 3. Pair them and remove from queue ────────────────────────────────
    if let Some(idx) = best_idx {
        let opponent = active.get(idx).ok_or(PvPError::NotInQueue)?.clone();
        let wait_secs = now.saturating_sub(seeker.queued_at);

        // Rebuild queue without both entries
        let mut remaining = Vec::new(env);
        for i in 0..active.len() {
            if i != 0 && i != idx {
                if let Some(e) = active.get(i) {
                    remaining.push_back(e);
                }
            }
        }
        save_queue(env, &mode, &remaining);

        let result = MatchResult {
            player1: seeker.player.clone(),
            player2: opponent.player.clone(),
            mode: mode.clone(),
            mmr_diff: best_diff,
            wait_secs,
        };

        env.events().publish(
            (symbol_short!("mm"), symbol_short!("matched")),
            (
                seeker.player.clone(),
                opponent.player.clone(),
                best_diff,
                mode_symbol(&mode),
            ),
        );

        Ok(Some(result))
    } else {
        // No match yet — save pruned queue and wait
        save_queue(env, &mode, &active);
        Ok(None)
    }
}

/// Return current queue depth for a game mode.
pub fn queue_size(env: &Env, mode: GameMode) -> u32 {
    load_queue(env, &mode).len()
}

/// Check if a player is currently queued in a given mode.
pub fn is_queued(env: &Env, player: &Address, mode: GameMode) -> bool {
    let queue = load_queue(env, &mode);
    for i in 0..queue.len() {
        if let Some(e) = queue.get(i) {
            if e.player == *player {
                return true;
            }
        }
    }
    false
}

/// Return a player's current queue entry if present.
pub fn get_queue_status(env: &Env, player: &Address, mode: GameMode) -> Option<QueueEntry> {
    let queue = load_queue(env, &mode);
    for i in 0..queue.len() {
        if let Some(e) = queue.get(i) {
            if e.player == *player {
                return Some(e);
            }
        }
    }
    None
}
