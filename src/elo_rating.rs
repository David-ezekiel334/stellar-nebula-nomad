/// ELO-Based Ranking System — Issue #590
///
/// Implements a complete ELO rating algorithm with K-factor calculation,
/// 8 ranking tiers (Bronze → Challenger), promotion/demotion thresholds,
/// rating decay for inactivity, and seasonal adjustments.
///
/// The heavy-lifting math (ratings, decay) lives in `pvp_combat`; this
/// module re-exports those primitives and layers the tier/seasonal logic
/// on top so both can be used independently.
use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol, Vec};

use crate::pvp_combat::{
    get_elo_rating, update_elo_rating, ELO_DECAY_FLOOR, INITIAL_ELO, K_FACTOR,
};

// ── Tier Thresholds ───────────────────────────────────────────────────────────
// 8 tiers: Bronze → Silver → Gold → Platinum → Diamond → Master → GrandMaster → Challenger

pub const TIER_BRONZE_MIN: u32 = 0;
pub const TIER_SILVER_MIN: u32 = 1200;
pub const TIER_GOLD_MIN: u32 = 1400;
pub const TIER_PLATINUM_MIN: u32 = 1600;
pub const TIER_DIAMOND_MIN: u32 = 1800;
pub const TIER_MASTER_MIN: u32 = 2000;
pub const TIER_GRANDMASTER_MIN: u32 = 2200;
pub const TIER_CHALLENGER_MIN: u32 = 2500;

/// Demotion safety buffer — a player must fall this many points below a tier
/// floor before being demoted, preventing yo-yo at boundaries.
pub const DEMOTION_BUFFER: u32 = 50;

/// K-factor for new players (fewer than 30 games).
pub const K_FACTOR_NEW: u32 = 40;
/// K-factor for established players (30–99 games).
pub const K_FACTOR_MID: u32 = 32;
/// K-factor for high-ELO veterans (1800+).
pub const K_FACTOR_HIGH: u32 = 16;

/// Seasonal decay: percentage of rating above floor removed each season reset.
pub const SEASONAL_DECAY_BPS: u32 = 1000; // 10 %

/// Minimum rating after a seasonal reset.
pub const SEASONAL_FLOOR: u32 = ELO_DECAY_FLOOR;

// ── Data Types ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum RankTier {
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    Master,
    GrandMaster,
    Challenger,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct RatingEntry {
    pub player: Address,
    pub rating: u32,
    pub tier: RankTier,
    pub games_played: u32,
    pub season: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SeasonConfig {
    pub current_season: u32,
    pub decay_bps: u32,
    pub floor: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum EloKey {
    GamesPlayed(Address),
    RatingHistory(Address, u32),
    SeasonConfig,
    EloLeaderboard,
}

// ── Tier Logic ────────────────────────────────────────────────────────────────

/// Return the tier for a given ELO rating.
pub fn tier_for_rating(rating: u32) -> RankTier {
    if rating >= TIER_CHALLENGER_MIN {
        RankTier::Challenger
    } else if rating >= TIER_GRANDMASTER_MIN {
        RankTier::GrandMaster
    } else if rating >= TIER_MASTER_MIN {
        RankTier::Master
    } else if rating >= TIER_DIAMOND_MIN {
        RankTier::Diamond
    } else if rating >= TIER_PLATINUM_MIN {
        RankTier::Platinum
    } else if rating >= TIER_GOLD_MIN {
        RankTier::Gold
    } else if rating >= TIER_SILVER_MIN {
        RankTier::Silver
    } else {
        RankTier::Bronze
    }
}

/// Return the display Symbol for a tier (≤9 chars, Soroban-safe).
pub fn tier_symbol(tier: &RankTier) -> Symbol {
    match tier {
        RankTier::Bronze => symbol_short!("bronze"),
        RankTier::Silver => symbol_short!("silver"),
        RankTier::Gold => symbol_short!("gold"),
        RankTier::Platinum => symbol_short!("platinum"),
        RankTier::Diamond => symbol_short!("diamond"),
        RankTier::Master => symbol_short!("master"),
        RankTier::GrandMaster => symbol_short!("gm"),
        RankTier::Challenger => symbol_short!("chall"),
    }
}

/// Dynamic K-factor based on games played and current rating.
pub fn k_factor_for(games_played: u32, rating: u32) -> u32 {
    if games_played < 30 {
        K_FACTOR_NEW
    } else if rating >= TIER_MASTER_MIN {
        K_FACTOR_HIGH
    } else {
        K_FACTOR_MID
    }
}

// ── Games-Played Tracking ─────────────────────────────────────────────────────

pub fn increment_games_played(env: &Env, player: &Address) -> u32 {
    let key = EloKey::GamesPlayed(player.clone());
    let count: u32 = env.storage().persistent().get(&key).unwrap_or(0);
    let new_count = count.saturating_add(1);
    env.storage().persistent().set(&key, &new_count);
    new_count
}

pub fn get_games_played(env: &Env, player: &Address) -> u32 {
    let key = EloKey::GamesPlayed(player.clone());
    env.storage().persistent().get(&key).unwrap_or(0)
}

// ── ELO Calculation ───────────────────────────────────────────────────────────

/// Calculate updated ratings for winner and loser using dynamic K-factor.
/// Returns `(new_winner_rating, new_loser_rating)`.
pub fn calculate_updated_ratings(env: &Env, winner: &Address, loser: &Address) -> (u32, u32) {
    let winner_rating = get_elo_rating(env, winner);
    let loser_rating = get_elo_rating(env, loser);
    let winner_games = get_games_played(env, winner);
    let loser_games = get_games_played(env, loser);

    let k_winner = k_factor_for(winner_games, winner_rating) as f64;
    let k_loser = k_factor_for(loser_games, loser_rating) as f64;

    let expected_winner =
        1.0 / (1.0 + libm::pow(10.0, (loser_rating as f64 - winner_rating as f64) / 400.0));
    let expected_loser = 1.0 - expected_winner;

    let winner_gain = libm::round(k_winner * (1.0 - expected_winner)) as u32;
    let loser_loss = libm::round(k_loser * expected_loser) as u32;

    let new_winner = winner_rating.saturating_add(winner_gain);
    let new_loser = loser_rating.saturating_sub(loser_loss.min(loser_rating));

    (new_winner, new_loser)
}

/// Apply a match result: update ratings, increment games played, emit events.
pub fn apply_match_result(env: &Env, winner: &Address, loser: &Address) {
    let (new_winner_rating, new_loser_rating) = calculate_updated_ratings(env, winner, loser);

    update_elo_rating(env, winner, new_winner_rating);
    update_elo_rating(env, loser, new_loser_rating);

    increment_games_played(env, winner);
    increment_games_played(env, loser);

    let winner_tier = tier_for_rating(new_winner_rating);
    let loser_tier = tier_for_rating(new_loser_rating);

    env.events().publish(
        (symbol_short!("elo"), symbol_short!("match")),
        (
            winner.clone(),
            new_winner_rating,
            tier_symbol(&winner_tier),
            loser.clone(),
            new_loser_rating,
            tier_symbol(&loser_tier),
        ),
    );
}

// ── Tier Promotion / Demotion ─────────────────────────────────────────────────

/// Check if a player has promoted to a new tier this match.
/// Returns `Some(new_tier)` on promotion, `None` otherwise.
pub fn check_promotion(old_rating: u32, new_rating: u32) -> Option<RankTier> {
    let old_tier = tier_for_rating(old_rating);
    let new_tier = tier_for_rating(new_rating);
    if new_tier != old_tier && new_rating > old_rating {
        Some(new_tier)
    } else {
        None
    }
}

/// Check demotion with buffer: player must fall DEMOTION_BUFFER below tier
/// floor before actually demoting. Returns `Some(new_tier)` on demotion.
pub fn check_demotion(old_rating: u32, new_rating: u32) -> Option<RankTier> {
    let old_tier = tier_for_rating(old_rating);
    let floor = match old_tier {
        RankTier::Bronze => return None, // cannot demote from Bronze
        RankTier::Silver => TIER_SILVER_MIN,
        RankTier::Gold => TIER_GOLD_MIN,
        RankTier::Platinum => TIER_PLATINUM_MIN,
        RankTier::Diamond => TIER_DIAMOND_MIN,
        RankTier::Master => TIER_MASTER_MIN,
        RankTier::GrandMaster => TIER_GRANDMASTER_MIN,
        RankTier::Challenger => TIER_CHALLENGER_MIN,
    };
    if new_rating < floor.saturating_sub(DEMOTION_BUFFER) {
        Some(tier_for_rating(new_rating))
    } else {
        None
    }
}

// ── Seasonal Adjustments ──────────────────────────────────────────────────────

/// Get the current season config (defaults to season 1 if unset).
pub fn get_season_config(env: &Env) -> SeasonConfig {
    env.storage()
        .persistent()
        .get(&EloKey::SeasonConfig)
        .unwrap_or(SeasonConfig {
            current_season: 1,
            decay_bps: SEASONAL_DECAY_BPS,
            floor: SEASONAL_FLOOR,
        })
}

/// Apply seasonal soft-reset: compress ratings toward the floor by
/// `decay_bps` basis points, keeping them above `floor`.
/// Should be called by an admin/cron at the start of each new season.
pub fn apply_seasonal_reset(env: &Env, players: Vec<Address>) {
    let config = get_season_config(env);

    for player in players.iter() {
        let rating = get_elo_rating(env, &player);
        if rating <= config.floor {
            continue;
        }
        let above_floor = rating - config.floor;
        let decay = (above_floor as u64 * config.decay_bps as u64 / 10_000) as u32;
        let new_rating = rating.saturating_sub(decay).max(config.floor);
        if new_rating != rating {
            update_elo_rating(env, &player, new_rating);
        }
    }

    env.events().publish(
        (symbol_short!("elo"), symbol_short!("season")),
        (config.current_season,),
    );
}

/// Advance the season counter.
pub fn advance_season(env: &Env) -> u32 {
    let mut config = get_season_config(env);
    config.current_season = config.current_season.saturating_add(1);
    env.storage()
        .persistent()
        .set(&EloKey::SeasonConfig, &config);
    config.current_season
}

// ── Leaderboard Snapshot ──────────────────────────────────────────────────────

/// Build a leaderboard entry for a player.
pub fn get_rating_entry(env: &Env, player: &Address) -> RatingEntry {
    let rating = get_elo_rating(env, player);
    let games = get_games_played(env, player);
    let config = get_season_config(env);
    RatingEntry {
        player: player.clone(),
        rating,
        tier: tier_for_rating(rating),
        games_played: games,
        season: config.current_season,
    }
}
