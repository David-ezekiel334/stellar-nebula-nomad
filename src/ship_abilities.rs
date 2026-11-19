/// Ship Ability System — Issue #589
///
/// Defines 5 abilities per ship type (fighter, explorer, hauler + 2 bonus types),
/// handles cooldowns, energy costs, ability effects, and integrates with PvP combat.
use soroban_sdk::{contracttype, symbol_short, Address, Env, Symbol, Vec};

use crate::pvp_combat::{PvPError, PvPDataKey, CombatState};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const MAX_ABILITIES_PER_SHIP: u32 = 5;
/// Energy cap for combat (mirrors pvp_combat::MAX_ENERGY).
pub const ABILITY_MAX_ENERGY: u32 = 100;

// ── Data Types ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum AbilityEffect {
    /// Deal direct damage to opponent.
    Damage,
    /// Restore own HP.
    Heal,
    /// Restore own energy.
    EnergyRestore,
    /// Reduce opponent's energy.
    EnergySiphon,
    /// Boost own next attack damage (multiplicative).
    PowerBoost,
    /// Temporarily reduce damage taken.
    Shield,
    /// Deal damage that ignores partial defence.
    ArmorPierce,
    /// Stun: opponent loses next turn.
    Stun,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Ability {
    /// Short identifier — max 9 chars (Soroban Symbol constraint).
    pub id: Symbol,
    pub name: Symbol,
    pub effect: AbilityEffect,
    /// Base magnitude of the effect (damage/heal/restore amount).
    pub power: u32,
    /// Energy required to use.
    pub energy_cost: u32,
    /// Cooldown in turns before the ability can be used again.
    pub cooldown_turns: u32,
}

/// Per-player, per-combat ability state (cooldown tracking).
#[contracttype]
#[derive(Clone, Debug)]
pub struct AbilityState {
    pub ability_id: Symbol,
    /// Turns remaining on cooldown (0 = ready).
    pub cooldown_remaining: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ShipAbilityLoadout {
    pub ship_type: Symbol,
    pub abilities: Vec<Ability>,
}

#[contracttype]
#[derive(Clone)]
pub enum AbilityKey {
    /// Active ability states for a player in a combat.
    CombatAbilityState(u64, Address),
}

// ── Ability Definitions ───────────────────────────────────────────────────────

/// Return the 5 abilities defined for the "fighter" ship type.
pub fn fighter_abilities(env: &Env) -> Vec<Ability> {
    let mut v = Vec::new(env);
    v.push_back(Ability {
        id: symbol_short!("plasma"),
        name: symbol_short!("plasma"),
        effect: AbilityEffect::Damage,
        power: 25,
        energy_cost: 20,
        cooldown_turns: 2,
    });
    v.push_back(Ability {
        id: symbol_short!("missile"),
        name: symbol_short!("missile"),
        effect: AbilityEffect::ArmorPierce,
        power: 20,
        energy_cost: 25,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("overchrg"),
        name: symbol_short!("overchrg"),
        effect: AbilityEffect::PowerBoost,
        power: 15,
        energy_cost: 15,
        cooldown_turns: 4,
    });
    v.push_back(Ability {
        id: symbol_short!("evasion"),
        name: symbol_short!("evasion"),
        effect: AbilityEffect::Shield,
        power: 10,
        energy_cost: 10,
        cooldown_turns: 2,
    });
    v.push_back(Ability {
        id: symbol_short!("emp"),
        name: symbol_short!("emp"),
        effect: AbilityEffect::Stun,
        power: 5,
        energy_cost: 30,
        cooldown_turns: 5,
    });
    v
}

/// Return the 5 abilities defined for the "explorer" ship type.
pub fn explorer_abilities(env: &Env) -> Vec<Ability> {
    let mut v = Vec::new(env);
    v.push_back(Ability {
        id: symbol_short!("scan_pls"),
        name: symbol_short!("scan_pls"),
        effect: AbilityEffect::Damage,
        power: 15,
        energy_cost: 10,
        cooldown_turns: 1,
    });
    v.push_back(Ability {
        id: symbol_short!("nebula_s"),
        name: symbol_short!("nebula_s"),
        effect: AbilityEffect::Shield,
        power: 20,
        energy_cost: 15,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("essence"),
        name: symbol_short!("essence"),
        effect: AbilityEffect::EnergyRestore,
        power: 30,
        energy_cost: 5,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("phase"),
        name: symbol_short!("phase"),
        effect: AbilityEffect::Heal,
        power: 20,
        energy_cost: 20,
        cooldown_turns: 4,
    });
    v.push_back(Ability {
        id: symbol_short!("disrupt"),
        name: symbol_short!("disrupt"),
        effect: AbilityEffect::EnergySiphon,
        power: 20,
        energy_cost: 20,
        cooldown_turns: 4,
    });
    v
}

/// Return the 5 abilities defined for the "hauler" ship type.
pub fn hauler_abilities(env: &Env) -> Vec<Ability> {
    let mut v = Vec::new(env);
    v.push_back(Ability {
        id: symbol_short!("ram"),
        name: symbol_short!("ram"),
        effect: AbilityEffect::Damage,
        power: 30,
        energy_cost: 25,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("hull_rep"),
        name: symbol_short!("hull_rep"),
        effect: AbilityEffect::Heal,
        power: 35,
        energy_cost: 20,
        cooldown_turns: 4,
    });
    v.push_back(Ability {
        id: symbol_short!("cargo_dm"),
        name: symbol_short!("cargo_dm"),
        effect: AbilityEffect::ArmorPierce,
        power: 25,
        energy_cost: 30,
        cooldown_turns: 5,
    });
    v.push_back(Ability {
        id: symbol_short!("bulkhead"),
        name: symbol_short!("bulkhead"),
        effect: AbilityEffect::Shield,
        power: 30,
        energy_cost: 15,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("thruster"),
        name: symbol_short!("thruster"),
        effect: AbilityEffect::EnergyRestore,
        power: 20,
        energy_cost: 10,
        cooldown_turns: 2,
    });
    v
}

/// Return the 5 abilities defined for the "stealth" ship type.
pub fn stealth_abilities(env: &Env) -> Vec<Ability> {
    let mut v = Vec::new(env);
    v.push_back(Ability {
        id: symbol_short!("shadow"),
        name: symbol_short!("shadow"),
        effect: AbilityEffect::Shield,
        power: 25,
        energy_cost: 15,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("backstab"),
        name: symbol_short!("backstab"),
        effect: AbilityEffect::ArmorPierce,
        power: 35,
        energy_cost: 30,
        cooldown_turns: 4,
    });
    v.push_back(Ability {
        id: symbol_short!("smoke"),
        name: symbol_short!("smoke"),
        effect: AbilityEffect::EnergySiphon,
        power: 25,
        energy_cost: 20,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("cloakstk"),
        name: symbol_short!("cloakstk"),
        effect: AbilityEffect::Stun,
        power: 10,
        energy_cost: 25,
        cooldown_turns: 5,
    });
    v.push_back(Ability {
        id: symbol_short!("ghostpls"),
        name: symbol_short!("ghostpls"),
        effect: AbilityEffect::Damage,
        power: 20,
        energy_cost: 15,
        cooldown_turns: 2,
    });
    v
}

/// Return the 5 abilities defined for the "carrier" ship type.
pub fn carrier_abilities(env: &Env) -> Vec<Ability> {
    let mut v = Vec::new(env);
    v.push_back(Ability {
        id: symbol_short!("swarm"),
        name: symbol_short!("swarm"),
        effect: AbilityEffect::Damage,
        power: 20,
        energy_cost: 20,
        cooldown_turns: 2,
    });
    v.push_back(Ability {
        id: symbol_short!("med_bay"),
        name: symbol_short!("med_bay"),
        effect: AbilityEffect::Heal,
        power: 25,
        energy_cost: 15,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("hangar"),
        name: symbol_short!("hangar"),
        effect: AbilityEffect::EnergyRestore,
        power: 35,
        energy_cost: 10,
        cooldown_turns: 4,
    });
    v.push_back(Ability {
        id: symbol_short!("point_df"),
        name: symbol_short!("point_df"),
        effect: AbilityEffect::Shield,
        power: 20,
        energy_cost: 20,
        cooldown_turns: 3,
    });
    v.push_back(Ability {
        id: symbol_short!("broadsd"),
        name: symbol_short!("broadsd"),
        effect: AbilityEffect::ArmorPierce,
        power: 30,
        energy_cost: 35,
        cooldown_turns: 5,
    });
    v
}

/// Get the ability loadout for a given ship type symbol.
pub fn get_abilities_for_type(env: &Env, ship_type: &Symbol) -> Vec<Ability> {
    if *ship_type == symbol_short!("fighter") {
        fighter_abilities(env)
    } else if *ship_type == symbol_short!("explorer") {
        explorer_abilities(env)
    } else if *ship_type == symbol_short!("hauler") {
        hauler_abilities(env)
    } else if *ship_type == symbol_short!("stealth") {
        stealth_abilities(env)
    } else if *ship_type == symbol_short!("carrier") {
        carrier_abilities(env)
    } else {
        // Default: return fighter abilities for unknown types
        fighter_abilities(env)
    }
}

// ── Cooldown Management ───────────────────────────────────────────────────────

/// Initialise ability states for a player entering combat.
pub fn init_combat_abilities(env: &Env, combat_id: u64, player: &Address, ship_type: &Symbol) {
    let abilities = get_abilities_for_type(env, ship_type);
    let mut states = Vec::new(env);
    for i in 0..abilities.len() {
        if let Some(ab) = abilities.get(i) {
            states.push_back(AbilityState {
                ability_id: ab.id,
                cooldown_remaining: 0,
            });
        }
    }
    env.storage()
        .persistent()
        .set(&AbilityKey::CombatAbilityState(combat_id, player.clone()), &states);
}

/// Tick down all cooldowns by 1 at the start of a player's turn.
pub fn tick_cooldowns(env: &Env, combat_id: u64, player: &Address) {
    let key = AbilityKey::CombatAbilityState(combat_id, player.clone());
    let states: Vec<AbilityState> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));

    let mut updated = Vec::new(env);
    for i in 0..states.len() {
        if let Some(mut s) = states.get(i) {
            if s.cooldown_remaining > 0 {
                s.cooldown_remaining -= 1;
            }
            updated.push_back(s);
        }
    }
    env.storage().persistent().set(&key, &updated);
}

/// Check if an ability is off cooldown for a player in a combat.
pub fn is_ability_ready(env: &Env, combat_id: u64, player: &Address, ability_id: &Symbol) -> bool {
    let key = AbilityKey::CombatAbilityState(combat_id, player.clone());
    let states: Vec<AbilityState> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));

    for i in 0..states.len() {
        if let Some(s) = states.get(i) {
            if s.ability_id == *ability_id {
                return s.cooldown_remaining == 0;
            }
        }
    }
    false
}

/// Apply an ability in combat. Updates the CombatState (HP/energy) and sets
/// the ability on cooldown. Returns `Err(InvalidMove)` when ability is on
/// cooldown or player lacks energy.
pub fn use_ability(
    env: &Env,
    combat_id: u64,
    player: &Address,
    ship_type: &Symbol,
    ability_id: &Symbol,
) -> Result<(), PvPError> {
    // Load ability definition
    let abilities = get_abilities_for_type(env, ship_type);
    let mut ability_opt: Option<Ability> = None;
    for i in 0..abilities.len() {
        if let Some(ab) = abilities.get(i) {
            if ab.id == *ability_id {
                ability_opt = Some(ab);
                break;
            }
        }
    }
    let ability = ability_opt.ok_or(PvPError::InvalidMove)?;

    // Check cooldown
    if !is_ability_ready(env, combat_id, player, ability_id) {
        return Err(PvPError::InvalidMove);
    }

    // Load combat state
    let combat_key = PvPDataKey::Combat(combat_id);
    let mut combat: CombatState = env
        .storage()
        .persistent()
        .get(&combat_key)
        .ok_or(PvPError::CombatNotFound)?;

    let is_player1 = *player == combat.player1;

    // Check energy
    if is_player1 {
        if combat.player1_energy < ability.energy_cost {
            return Err(PvPError::InvalidMove);
        }
        combat.player1_energy -= ability.energy_cost;
    } else {
        if combat.player2_energy < ability.energy_cost {
            return Err(PvPError::InvalidMove);
        }
        combat.player2_energy -= ability.energy_cost;
    }

    // Apply effect
    match ability.effect {
        AbilityEffect::Damage | AbilityEffect::ArmorPierce => {
            if is_player1 {
                combat.player2_hp = combat.player2_hp.saturating_sub(ability.power);
            } else {
                combat.player1_hp = combat.player1_hp.saturating_sub(ability.power);
            }
        }
        AbilityEffect::Heal => {
            if is_player1 {
                combat.player1_hp = (combat.player1_hp + ability.power).min(100);
            } else {
                combat.player2_hp = (combat.player2_hp + ability.power).min(100);
            }
        }
        AbilityEffect::EnergyRestore => {
            if is_player1 {
                combat.player1_energy = (combat.player1_energy + ability.power).min(ABILITY_MAX_ENERGY);
            } else {
                combat.player2_energy = (combat.player2_energy + ability.power).min(ABILITY_MAX_ENERGY);
            }
        }
        AbilityEffect::EnergySiphon => {
            if is_player1 {
                combat.player2_energy = combat.player2_energy.saturating_sub(ability.power);
            } else {
                combat.player1_energy = combat.player1_energy.saturating_sub(ability.power);
            }
        }
        AbilityEffect::PowerBoost => {
            // Stored as a flag in history; damage resolution honours it
            // in the next attack move (tracked via event).
            let log = if is_player1 {
                Symbol::new(env, "p1_boost")
            } else {
                Symbol::new(env, "p2_boost")
            };
            combat.history.push_back(log);
        }
        AbilityEffect::Shield => {
            let log = if is_player1 {
                Symbol::new(env, "p1_shield")
            } else {
                Symbol::new(env, "p2_shield")
            };
            combat.history.push_back(log);
        }
        AbilityEffect::Stun => {
            // Stun: skip opponent turn — recorded in history for resolver
            let log = if is_player1 {
                Symbol::new(env, "p2_stun")
            } else {
                Symbol::new(env, "p1_stun")
            };
            combat.history.push_back(log);
        }
    }

    // Save combat state
    env.storage().persistent().set(&combat_key, &combat);

    // Set cooldown
    let state_key = AbilityKey::CombatAbilityState(combat_id, player.clone());
    let states: Vec<AbilityState> = env
        .storage()
        .persistent()
        .get(&state_key)
        .unwrap_or_else(|| Vec::new(env));

    let mut new_states = Vec::new(env);
    for i in 0..states.len() {
        if let Some(mut s) = states.get(i) {
            if s.ability_id == *ability_id {
                s.cooldown_remaining = ability.cooldown_turns;
            }
            new_states.push_back(s);
        }
    }
    env.storage().persistent().set(&state_key, &new_states);

    env.events().publish(
        (symbol_short!("ab"), symbol_short!("used")),
        (player.clone(), ability_id.clone(), combat_id),
    );

    Ok(())
}

/// Get current ability states (cooldowns) for a player in a combat.
pub fn get_ability_states(
    env: &Env,
    combat_id: u64,
    player: &Address,
) -> Vec<AbilityState> {
    let key = AbilityKey::CombatAbilityState(combat_id, player.clone());
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env))
}
