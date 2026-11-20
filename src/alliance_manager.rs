//! Alliance membership, treasury, and progression management.
//!
use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, String, Symbol, Vec};

use crate::input_validation;

// Alliance configuration constants
/// Base member cap at level 1; grows with level (see [`get_alliance_benefits`]).
pub const MAX_MEMBERS_PER_ALLIANCE: u32 = 50;
/// Hard ceiling on members regardless of level.
pub const MAX_MEMBER_CAP: u32 = 100;
/// Base cost of claiming a territory before level discounts.
pub const TERRITORY_CLAIM_COST: i128 = 500;
pub const MIN_VOTING_THRESHOLD: u32 = 51; // 51% for decisions
pub const INITIAL_TREASURY: i128 = 0;

#[derive(Clone)]
#[contracttype]
pub enum AllianceKey {
    Alliance(u64),                    // alliance_id -> Alliance
    AllianceCount,                    // -> u64 (next alliance ID)
    MemberAlliance(Address),          // player -> alliance_id
    AllianceTreasury(u64),            // alliance_id -> i128
    MemberContribution(u64, Address), // (alliance_id, member) -> i128
    AllianceLevelInfo(u64),           // alliance_id -> GuildLevelInfo
    TerritoryClaim(u64),              // coordinate_hash -> alliance_id
    GuildBoss(u64),                   // alliance_id -> GuildBossState
    /// (alliance_id, member) -> AllianceRole; absent means Member.
    AllianceMemberRole(u64, Address),
    /// (lower id, higher id) -> DiplomaticStatus; absent means Neutral.
    AllianceRelation(u64, u64),
    /// (from, to) -> proposed DiplomaticStatus.
    AllianceRelationProposal(u64, u64),
    /// (alliance_id, objective_id) -> AllianceObjective.
    AllianceObjective(u64, u32),
    /// alliance_id -> next objective id.
    AllianceObjectiveCount(u64),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct GuildLevelInfo {
    pub level: u32,
    pub xp: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct GuildBossState {
    pub boss_id: u32,
    pub max_health: u64,
    pub current_health: u64,
    pub attack_power: u32,
    pub active_until: u64,
    pub defeated: bool,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AllianceError {
    AllianceFull = 1,
    AllianceNotFound = 2,
    AlreadyInAlliance = 3,
    NotMember = 4,
    Unauthorized = 5,
    InvalidName = 6,
    InsufficientVotes = 7,
    /// Member hit today's guild contribution cap (anti-whale).
    ContributionCapExceeded = 8,
    /// The leader must hand over leadership before leaving a non-empty alliance.
    LeaderMustTransfer = 9,
    /// Target alliance or member is invalid for this action.
    InvalidTarget = 10,
    /// Objective does not exist.
    ObjectiveNotFound = 11,
    /// Objective is completed or past its deadline.
    ObjectiveClosed = 12,
    /// Objective parameters or contribution amount are invalid.
    InvalidObjective = 13,
}

impl crate::error_standard::StandardContractError for AllianceError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::AllianceFull | Self::InsufficientVotes => (ErrorKind::ResourceLimit, false),
            Self::AllianceNotFound | Self::ObjectiveNotFound => (ErrorKind::NotFound, false),
            Self::AlreadyInAlliance | Self::LeaderMustTransfer | Self::ObjectiveClosed => {
                (ErrorKind::Conflict, false)
            }
            Self::NotMember | Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::InvalidName | Self::InvalidTarget | Self::InvalidObjective => {
                (ErrorKind::Validation, false)
            }
            Self::ContributionCapExceeded => (ErrorKind::ResourceLimit, true),
        };
        crate::error_standard::ErrorDescriptor {
            module: "alliance_manager",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct Alliance {
    pub alliance_id: u64,
    pub name: String,
    /// Current leader: the founder until `transfer_leadership` hands it on.
    /// Other modules (guild_economy, guild_quests) authorize against it.
    pub founder: Address,
    pub created_at: u64,
    pub members: Vec<Address>,
    pub voting_threshold: u32,
    pub is_active: bool,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct MembershipRecord {
    pub alliance_id: u64,
    pub member: Address,
    pub joined_at: u64,
    pub contribution: i128,
}

/// Found a new alliance with initial treasury
pub fn found_alliance(env: &Env, founder: Address, name: String) -> Result<u64, AllianceError> {
    founder.require_auth();

    input_validation::validate_name(env, &name).map_err(|_| AllianceError::InvalidName)?;

    // Check if founder is already in an alliance
    if env
        .storage()
        .persistent()
        .has(&AllianceKey::MemberAlliance(founder.clone()))
    {
        return Err(AllianceError::AlreadyInAlliance);
    }

    if name.len() == 0 {
        return Err(AllianceError::InvalidName);
    }

    // Generate alliance ID
    let alliance_id = env
        .storage()
        .persistent()
        .get::<AllianceKey, u64>(&AllianceKey::AllianceCount)
        .unwrap_or(0);

    let current_time = env.ledger().timestamp();
    let mut members = Vec::new(env);
    members.push_back(founder.clone());

    let alliance = Alliance {
        alliance_id,
        name: name.clone(),
        founder: founder.clone(),
        created_at: current_time,
        members,
        voting_threshold: MIN_VOTING_THRESHOLD,
        is_active: true,
    };

    // Store alliance
    env.storage()
        .persistent()
        .set(&AllianceKey::Alliance(alliance_id), &alliance);

    // Update alliance count
    env.storage()
        .persistent()
        .set(&AllianceKey::AllianceCount, &(alliance_id + 1));

    // Set founder's alliance membership
    env.storage()
        .persistent()
        .set(&AllianceKey::MemberAlliance(founder.clone()), &alliance_id);

    // Initialize treasury
    env.storage().persistent().set(
        &AllianceKey::AllianceTreasury(alliance_id),
        &INITIAL_TREASURY,
    );

    // Initialize founder contribution
    env.storage().persistent().set(
        &AllianceKey::MemberContribution(alliance_id, founder.clone()),
        &0i128,
    );

    // Emit event
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("founded")),
        (alliance_id, name, founder),
    );

    Ok(alliance_id)
}

/// Join an existing alliance
pub fn join_alliance(
    env: &Env,
    alliance_id: u64,
    player: Address,
) -> Result<MembershipRecord, AllianceError> {
    player.require_auth();

    // Check if player is already in an alliance
    if env
        .storage()
        .persistent()
        .has(&AllianceKey::MemberAlliance(player.clone()))
    {
        return Err(AllianceError::AlreadyInAlliance);
    }

    // Get alliance
    let mut alliance = env
        .storage()
        .persistent()
        .get::<AllianceKey, Alliance>(&AllianceKey::Alliance(alliance_id))
        .ok_or(AllianceError::AllianceNotFound)?;

    // Check member limit (grows with alliance level)
    if alliance.members.len() >= get_alliance_benefits(env, alliance_id).member_cap {
        return Err(AllianceError::AllianceFull);
    }

    // Add member
    alliance.members.push_back(player.clone());

    // Update alliance
    env.storage()
        .persistent()
        .set(&AllianceKey::Alliance(alliance_id), &alliance);

    // Set player's alliance membership
    env.storage()
        .persistent()
        .set(&AllianceKey::MemberAlliance(player.clone()), &alliance_id);

    // Initialize member contribution
    env.storage().persistent().set(
        &AllianceKey::MemberContribution(alliance_id, player.clone()),
        &0i128,
    );

    let current_time = env.ledger().timestamp();
    let membership = MembershipRecord {
        alliance_id,
        member: player.clone(),
        joined_at: current_time,
        contribution: 0,
    };

    // Emit event
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("joined")),
        (alliance_id, player),
    );

    Ok(membership)
}

/// Leave an alliance (revocable membership)
pub fn leave_alliance(env: &Env, player: Address) -> Result<(), AllianceError> {
    player.require_auth();

    // Get player's alliance
    let alliance_id = env
        .storage()
        .persistent()
        .get::<AllianceKey, u64>(&AllianceKey::MemberAlliance(player.clone()))
        .ok_or(AllianceError::NotMember)?;

    // Get alliance
    let mut alliance = env
        .storage()
        .persistent()
        .get::<AllianceKey, Alliance>(&AllianceKey::Alliance(alliance_id))
        .ok_or(AllianceError::AllianceNotFound)?;

    if alliance.founder == player && alliance.members.len() > 1 {
        return Err(AllianceError::LeaderMustTransfer);
    }

    remove_member(env, &mut alliance, &player);

    // Emit event
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("left")),
        (alliance_id, player),
    );

    Ok(())
}

/// Contribute resources to alliance treasury
pub fn contribute_to_treasury(
    env: &Env,
    player: Address,
    amount: i128,
) -> Result<i128, AllianceError> {
    player.require_auth();

    if amount <= 0 {
        return Err(AllianceError::Unauthorized);
    }

    // Per-member daily cap so one whale cannot own the treasury.
    crate::economics::anti_whale::check_guild_contribution(env, &player, amount)
        .map_err(|_| AllianceError::ContributionCapExceeded)?;

    // Get player's alliance
    let alliance_id = env
        .storage()
        .persistent()
        .get::<AllianceKey, u64>(&AllianceKey::MemberAlliance(player.clone()))
        .ok_or(AllianceError::NotMember)?;

    // Update treasury
    let current_treasury = env
        .storage()
        .persistent()
        .get::<AllianceKey, i128>(&AllianceKey::AllianceTreasury(alliance_id))
        .unwrap_or(INITIAL_TREASURY);

    let new_treasury = current_treasury.saturating_add(amount);

    env.storage()
        .persistent()
        .set(&AllianceKey::AllianceTreasury(alliance_id), &new_treasury);

    // Update member contribution
    let current_contribution = env
        .storage()
        .persistent()
        .get::<AllianceKey, i128>(&AllianceKey::MemberContribution(
            alliance_id,
            player.clone(),
        ))
        .unwrap_or(0);

    let new_contribution = current_contribution.saturating_add(amount);

    env.storage().persistent().set(
        &AllianceKey::MemberContribution(alliance_id, player.clone()),
        &new_contribution,
    );

    // Emit event
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("contrib")),
        (alliance_id, player, amount, new_treasury),
    );

    Ok(new_treasury)
}

/// Get alliance details
pub fn get_alliance(env: &Env, alliance_id: u64) -> Result<Alliance, AllianceError> {
    env.storage()
        .persistent()
        .get::<AllianceKey, Alliance>(&AllianceKey::Alliance(alliance_id))
        .ok_or(AllianceError::AllianceNotFound)
}

/// Get alliance treasury balance
pub fn get_alliance_treasury(env: &Env, alliance_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get::<AllianceKey, i128>(&AllianceKey::AllianceTreasury(alliance_id))
        .unwrap_or(INITIAL_TREASURY)
}

/// Get member's contribution to alliance
pub fn get_member_contribution(env: &Env, alliance_id: u64, member: Address) -> i128 {
    env.storage()
        .persistent()
        .get::<AllianceKey, i128>(&AllianceKey::MemberContribution(alliance_id, member))
        .unwrap_or(0)
}

/// Get player's current alliance ID
pub fn get_player_alliance(env: &Env, player: Address) -> Option<u64> {
    env.storage()
        .persistent()
        .get::<AllianceKey, u64>(&AllianceKey::MemberAlliance(player))
}

// ── Guild Leveling & Perks ──────────────────────────────────────────────────

pub fn get_alliance_level(env: &Env, alliance_id: u64) -> GuildLevelInfo {
    let key = AllianceKey::AllianceLevelInfo(alliance_id);
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or(GuildLevelInfo { level: 1, xp: 0 })
}

pub fn add_alliance_xp(env: &Env, alliance_id: u64, amount: u64) -> Result<u32, AllianceError> {
    let key = AllianceKey::AllianceLevelInfo(alliance_id);
    let mut info = get_alliance_level(env, alliance_id);

    info.xp = info.xp.saturating_add(amount);

    let mut next_lvl_xp = (info.level as u64) * 1000;
    let mut leveled_up = false;
    while info.xp >= next_lvl_xp {
        info.xp = info.xp.saturating_sub(next_lvl_xp);
        info.level += 1;
        next_lvl_xp = (info.level as u64) * 1000;
        leveled_up = true;
    }

    env.storage().persistent().set(&key, &info);

    if leveled_up {
        env.events().publish(
            (symbol_short!("guild"), symbol_short!("level_up")),
            (alliance_id, info.level),
        );
    }

    Ok(info.level)
}

pub fn credit_alliance_treasury(
    env: &Env,
    alliance_id: u64,
    amount: i128,
) -> Result<i128, AllianceError> {
    if amount <= 0 {
        return Ok(get_alliance_treasury(env, alliance_id));
    }

    let treasury_key = AllianceKey::AllianceTreasury(alliance_id);
    let current_treasury = get_alliance_treasury(env, alliance_id);

    let new_treasury = current_treasury.saturating_add(amount);
    env.storage().persistent().set(&treasury_key, &new_treasury);

    env.events().publish(
        (symbol_short!("guild"), symbol_short!("credited")),
        (alliance_id, amount, new_treasury),
    );

    Ok(new_treasury)
}

// ── Territory Staking ────────────────────────────────────────────────────────

pub fn claim_guild_territory(
    env: &Env,
    player: Address,
    coordinate_hash: u64,
) -> Result<(), AllianceError> {
    player.require_auth();

    let alliance_id = get_player_alliance(env, player.clone()).ok_or(AllianceError::NotMember)?;

    let discount = i128::from(get_alliance_benefits(env, alliance_id).territory_discount_bps);
    let cost = TERRITORY_CLAIM_COST * (10_000 - discount) / 10_000;
    let treasury_key = AllianceKey::AllianceTreasury(alliance_id);
    let current_treasury = get_alliance_treasury(env, alliance_id);

    if current_treasury < cost {
        return Err(AllianceError::Unauthorized); // Insufficient funds in treasury
    }

    let territory_key = AllianceKey::TerritoryClaim(coordinate_hash);
    if env.storage().persistent().has(&territory_key) {
        return Err(AllianceError::AlreadyInAlliance); // Already claimed
    }

    let new_treasury = current_treasury.saturating_sub(cost);
    env.storage().persistent().set(&treasury_key, &new_treasury);

    env.storage().persistent().set(&territory_key, &alliance_id);

    env.events().publish(
        (symbol_short!("guild"), symbol_short!("claim")),
        (alliance_id, coordinate_hash, player),
    );

    Ok(())
}

pub fn get_territory_owner(env: &Env, coordinate_hash: u64) -> Option<u64> {
    let territory_key = AllianceKey::TerritoryClaim(coordinate_hash);
    env.storage().persistent().get(&territory_key)
}

// ── Boss Encounters ──────────────────────────────────────────────────────────

pub fn spawn_guild_boss(env: &Env, player: Address) -> Result<(), AllianceError> {
    player.require_auth();

    let alliance_id = get_player_alliance(env, player.clone()).ok_or(AllianceError::NotMember)?;

    let cost = 1000i128;
    let treasury_key = AllianceKey::AllianceTreasury(alliance_id);
    let current_treasury = get_alliance_treasury(env, alliance_id);

    if current_treasury < cost {
        return Err(AllianceError::Unauthorized);
    }

    let new_treasury = current_treasury.saturating_sub(cost);
    env.storage().persistent().set(&treasury_key, &new_treasury);

    let boss_key = AllianceKey::GuildBoss(alliance_id);
    let active_until = env.ledger().timestamp() + 86400;

    let boss_state = GuildBossState {
        boss_id: 1,
        max_health: 10_000,
        current_health: 10_000,
        attack_power: 100,
        active_until,
        defeated: false,
    };

    env.storage().persistent().set(&boss_key, &boss_state);

    env.events().publish(
        (symbol_short!("guild"), symbol_short!("boss_spn")),
        (alliance_id, active_until),
    );

    Ok(())
}

pub fn attack_guild_boss(
    env: &Env,
    player: Address,
    damage: u64,
) -> Result<GuildBossState, AllianceError> {
    player.require_auth();

    let alliance_id = get_player_alliance(env, player.clone()).ok_or(AllianceError::NotMember)?;

    let boss_key = AllianceKey::GuildBoss(alliance_id);
    let mut boss: GuildBossState = env
        .storage()
        .persistent()
        .get(&boss_key)
        .ok_or(AllianceError::AllianceNotFound)?;

    if boss.defeated || env.ledger().timestamp() > boss.active_until {
        return Err(AllianceError::Unauthorized);
    }

    boss.current_health = boss.current_health.saturating_sub(damage);

    if boss.current_health == 0 {
        boss.defeated = true;
        // Payout and Guild XP rewards
        let _ = credit_alliance_treasury(env, alliance_id, 2000i128);
        let _ = add_alliance_xp(env, alliance_id, 1500u64);

        // Contribute quest progress
        let _ = crate::guild_quests::contribute_quest_progress(
            env,
            player.clone(),
            symbol_short!("boss_kill"),
            1,
        );
    }

    env.storage().persistent().set(&boss_key, &boss);

    env.events().publish(
        (symbol_short!("guild"), symbol_short!("boss_dmg")),
        (alliance_id, player, damage, boss.current_health),
    );

    Ok(boss)
}

pub fn get_guild_boss(env: &Env, alliance_id: u64) -> Option<GuildBossState> {
    let boss_key = AllianceKey::GuildBoss(alliance_id);
    env.storage().persistent().get(&boss_key)
}

// ── Member management, diplomacy, objectives, benefits (#712) ───────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[contracttype]
pub enum AllianceRole {
    Member = 0,
    Officer = 1,
    Leader = 2,
}

/// Relation between two alliances. `Neutral` is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum DiplomaticStatus {
    Allied = 0,
    Neutral = 1,
    AtWar = 2,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AllianceObjective {
    pub objective_id: u32,
    pub kind: Symbol,
    pub target: u64,
    pub progress: u64,
    pub reward_xp: u64,
    pub deadline: u64,
    pub completed: bool,
}

/// Perks unlocked by alliance level.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AllianceBenefits {
    pub level: u32,
    pub member_cap: u32,
    /// Discount on territory claims, in basis points.
    pub territory_discount_bps: u32,
}

/// Drop `player` from the alliance's member list and membership keys.
fn remove_member(env: &Env, alliance: &mut Alliance, player: &Address) {
    let mut kept = Vec::new(env);
    for member in alliance.members.iter() {
        if member != *player {
            kept.push_back(member);
        }
    }
    alliance.members = kept;
    env.storage()
        .persistent()
        .set(&AllianceKey::Alliance(alliance.alliance_id), &*alliance);
    env.storage()
        .persistent()
        .remove(&AllianceKey::MemberAlliance(player.clone()));
    env.storage()
        .persistent()
        .remove(&AllianceKey::AllianceMemberRole(
            alliance.alliance_id,
            player.clone(),
        ));
}

/// Role of `member` in `alliance_id`, or `None` if they are not a member.
pub fn get_member_role(env: &Env, alliance_id: u64, member: &Address) -> Option<AllianceRole> {
    if get_player_alliance(env, member.clone()) != Some(alliance_id) {
        return None;
    }
    let alliance = get_alliance(env, alliance_id).ok()?;
    if alliance.founder == *member {
        return Some(AllianceRole::Leader);
    }
    Some(
        env.storage()
            .persistent()
            .get(&AllianceKey::AllianceMemberRole(
                alliance_id,
                member.clone(),
            ))
            .unwrap_or(AllianceRole::Member),
    )
}

/// Authenticate `actor` and require at least `min` in their alliance.
fn require_role(
    env: &Env,
    actor: &Address,
    min: AllianceRole,
) -> Result<(Alliance, AllianceRole), AllianceError> {
    actor.require_auth();
    let alliance_id = get_player_alliance(env, actor.clone()).ok_or(AllianceError::NotMember)?;
    let alliance = get_alliance(env, alliance_id)?;
    let role = get_member_role(env, alliance_id, actor).ok_or(AllianceError::NotMember)?;
    if role < min {
        return Err(AllianceError::Unauthorized);
    }
    Ok((alliance, role))
}

/// Promote or demote a member to `Member` or `Officer`. Leader only.
pub fn set_member_role(
    env: &Env,
    leader: Address,
    member: Address,
    role: AllianceRole,
) -> Result<(), AllianceError> {
    let (alliance, _) = require_role(env, &leader, AllianceRole::Leader)?;
    if role == AllianceRole::Leader || member == leader {
        return Err(AllianceError::InvalidTarget); // use transfer_leadership
    }
    get_member_role(env, alliance.alliance_id, &member).ok_or(AllianceError::NotMember)?;
    env.storage().persistent().set(
        &AllianceKey::AllianceMemberRole(alliance.alliance_id, member.clone()),
        &role,
    );
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("role")),
        (alliance.alliance_id, member, role),
    );
    Ok(())
}

/// Remove a member of strictly lower rank. Officers and the leader only.
pub fn kick_member(env: &Env, actor: Address, member: Address) -> Result<(), AllianceError> {
    let (mut alliance, actor_role) = require_role(env, &actor, AllianceRole::Officer)?;
    let target_role =
        get_member_role(env, alliance.alliance_id, &member).ok_or(AllianceError::NotMember)?;
    if target_role >= actor_role {
        return Err(AllianceError::Unauthorized);
    }
    remove_member(env, &mut alliance, &member);
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("kicked")),
        (alliance.alliance_id, member, actor),
    );
    Ok(())
}

/// Hand leadership to another member; the old leader becomes an officer.
pub fn transfer_leadership(
    env: &Env,
    leader: Address,
    new_leader: Address,
) -> Result<(), AllianceError> {
    let (mut alliance, _) = require_role(env, &leader, AllianceRole::Leader)?;
    let id = alliance.alliance_id;
    if new_leader == leader {
        return Err(AllianceError::InvalidTarget);
    }
    get_member_role(env, id, &new_leader).ok_or(AllianceError::NotMember)?;
    alliance.founder = new_leader.clone();
    env.storage()
        .persistent()
        .set(&AllianceKey::Alliance(id), &alliance);
    env.storage()
        .persistent()
        .remove(&AllianceKey::AllianceMemberRole(id, new_leader.clone()));
    env.storage().persistent().set(
        &AllianceKey::AllianceMemberRole(id, leader.clone()),
        &AllianceRole::Officer,
    );
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("leader")),
        (id, leader, new_leader),
    );
    Ok(())
}

fn relation_key(a: u64, b: u64) -> AllianceKey {
    AllianceKey::AllianceRelation(a.min(b), a.max(b))
}

fn hostility(status: DiplomaticStatus) -> u32 {
    status as u32
}

pub fn get_relation(env: &Env, a: u64, b: u64) -> DiplomaticStatus {
    env.storage()
        .persistent()
        .get(&relation_key(a, b))
        .unwrap_or(DiplomaticStatus::Neutral)
}

pub fn get_relation_proposal(env: &Env, from: u64, to: u64) -> Option<DiplomaticStatus> {
    env.storage()
        .persistent()
        .get(&AllianceKey::AllianceRelationProposal(from, to))
}

/// Change the relation with `target_alliance`. Leader only.
///
/// Becoming more hostile (`Allied` → `Neutral`, anything → `AtWar`) takes
/// effect immediately. Becoming friendlier needs both leaders: the first call
/// records a proposal, the matching call from the other side applies it.
/// Returns the relation after the call.
pub fn propose_relation(
    env: &Env,
    leader: Address,
    target_alliance: u64,
    status: DiplomaticStatus,
) -> Result<DiplomaticStatus, AllianceError> {
    let (alliance, _) = require_role(env, &leader, AllianceRole::Leader)?;
    let me = alliance.alliance_id;
    if target_alliance == me {
        return Err(AllianceError::InvalidTarget);
    }
    get_alliance(env, target_alliance)?;
    let current = get_relation(env, me, target_alliance);
    if status == current {
        return Err(AllianceError::InvalidTarget);
    }

    let theirs = AllianceKey::AllianceRelationProposal(target_alliance, me);
    let mine = AllianceKey::AllianceRelationProposal(me, target_alliance);
    let agreed = env
        .storage()
        .persistent()
        .get::<_, DiplomaticStatus>(&theirs)
        == Some(status);
    if hostility(status) > hostility(current) || agreed {
        env.storage()
            .persistent()
            .set(&relation_key(me, target_alliance), &status);
        env.storage().persistent().remove(&theirs);
        env.storage().persistent().remove(&mine);
        env.events().publish(
            (symbol_short!("alliance"), symbol_short!("relation")),
            (me, target_alliance, status),
        );
        return Ok(status);
    }
    env.storage().persistent().set(&mine, &status);
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("propose")),
        (me, target_alliance, status),
    );
    Ok(current)
}

/// Create a shared objective. Officers and the leader only.
pub fn create_objective(
    env: &Env,
    actor: Address,
    kind: Symbol,
    target: u64,
    reward_xp: u64,
    deadline: u64,
) -> Result<u32, AllianceError> {
    let (alliance, _) = require_role(env, &actor, AllianceRole::Officer)?;
    if target == 0 || deadline <= env.ledger().timestamp() {
        return Err(AllianceError::InvalidObjective);
    }
    let id = alliance.alliance_id;
    let count_key = AllianceKey::AllianceObjectiveCount(id);
    let objective_id: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
    let objective = AllianceObjective {
        objective_id,
        kind: kind.clone(),
        target,
        progress: 0,
        reward_xp,
        deadline,
        completed: false,
    };
    env.storage().persistent().set(
        &AllianceKey::AllianceObjective(id, objective_id),
        &objective,
    );
    env.storage()
        .persistent()
        .set(&count_key, &objective_id.saturating_add(1));
    env.events().publish(
        (symbol_short!("alliance"), symbol_short!("obj_new")),
        (id, objective_id, kind, target),
    );
    Ok(objective_id)
}

/// Add progress to an objective of the member's alliance. Completing it
/// (progress reaches target) awards `reward_xp` to the alliance once.
pub fn contribute_objective(
    env: &Env,
    member: Address,
    objective_id: u32,
    amount: u64,
) -> Result<AllianceObjective, AllianceError> {
    let (alliance, _) = require_role(env, &member, AllianceRole::Member)?;
    let id = alliance.alliance_id;
    let mut objective = get_objective(env, id, objective_id)?;
    if objective.completed || env.ledger().timestamp() > objective.deadline {
        return Err(AllianceError::ObjectiveClosed);
    }
    if amount == 0 {
        return Err(AllianceError::InvalidObjective);
    }
    objective.progress = objective
        .progress
        .saturating_add(amount)
        .min(objective.target);
    if objective.progress == objective.target {
        objective.completed = true;
        add_alliance_xp(env, id, objective.reward_xp)?;
        env.events().publish(
            (symbol_short!("alliance"), symbol_short!("obj_done")),
            (id, objective_id),
        );
    }
    env.storage().persistent().set(
        &AllianceKey::AllianceObjective(id, objective_id),
        &objective,
    );
    Ok(objective)
}

pub fn get_objective(
    env: &Env,
    alliance_id: u64,
    objective_id: u32,
) -> Result<AllianceObjective, AllianceError> {
    env.storage()
        .persistent()
        .get(&AllianceKey::AllianceObjective(alliance_id, objective_id))
        .ok_or(AllianceError::ObjectiveNotFound)
}

pub fn get_objective_count(env: &Env, alliance_id: u64) -> u32 {
    env.storage()
        .persistent()
        .get(&AllianceKey::AllianceObjectiveCount(alliance_id))
        .unwrap_or(0)
}

/// Perks for the alliance's current level: +5 member slots and +2.5 %
/// territory discount per level above 1, capped at 100 members / 25 %.
pub fn get_alliance_benefits(env: &Env, alliance_id: u64) -> AllianceBenefits {
    let level = get_alliance_level(env, alliance_id).level.max(1);
    let bonus_levels = level - 1;
    AllianceBenefits {
        level,
        member_cap: MAX_MEMBERS_PER_ALLIANCE
            .saturating_add(bonus_levels.saturating_mul(5))
            .min(MAX_MEMBER_CAP),
        territory_discount_bps: bonus_levels.saturating_mul(250).min(2_500),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _};
    use soroban_sdk::{contract, contractimpl};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    struct Setup {
        env: Env,
        id: Address,
    }

    impl Setup {
        /// One auth-checked call per frame (the test host rejects repeats).
        fn run<T>(&self, f: impl FnOnce() -> T) -> T {
            self.env.as_contract(&self.id, f)
        }
        fn player(&self) -> Address {
            Address::generate(&self.env)
        }
        fn found(&self, founder: &Address, name: &str) -> u64 {
            self.run(|| {
                found_alliance(
                    &self.env,
                    founder.clone(),
                    String::from_str(&self.env, name),
                )
                .unwrap()
            })
        }
        fn join(&self, id: u64, p: &Address) {
            self.run(|| join_alliance(&self.env, id, p.clone()).unwrap());
        }
    }

    fn setup() -> Setup {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register(Stub, ());
        Setup { env, id }
    }

    #[test]
    fn roles_and_kicks_follow_rank() {
        let s = setup();
        let (leader, officer, member) = (s.player(), s.player(), s.player());
        let id = s.found(&leader, "Orion");
        s.join(id, &officer);
        s.join(id, &member);

        // Only the leader assigns roles.
        s.run(|| {
            assert_eq!(
                set_member_role(
                    &s.env,
                    officer.clone(),
                    member.clone(),
                    AllianceRole::Officer
                ),
                Err(AllianceError::Unauthorized)
            );
        });
        s.run(|| {
            set_member_role(
                &s.env,
                leader.clone(),
                officer.clone(),
                AllianceRole::Officer,
            )
            .unwrap();
        });
        s.run(|| {
            assert_eq!(
                get_member_role(&s.env, id, &leader),
                Some(AllianceRole::Leader)
            );
            assert_eq!(
                get_member_role(&s.env, id, &officer),
                Some(AllianceRole::Officer)
            );
            assert_eq!(
                get_member_role(&s.env, id, &member),
                Some(AllianceRole::Member)
            );
        });

        // Officers cannot kick the leader; they can kick members.
        s.run(|| {
            assert_eq!(
                kick_member(&s.env, officer.clone(), leader.clone()),
                Err(AllianceError::Unauthorized)
            );
        });
        s.run(|| kick_member(&s.env, officer.clone(), member.clone()).unwrap());
        s.run(|| {
            assert_eq!(get_player_alliance(&s.env, member.clone()), None);
            assert_eq!(get_alliance(&s.env, id).unwrap().members.len(), 2);
            assert_eq!(get_member_role(&s.env, id, &member), None);
        });
    }

    #[test]
    fn leadership_transfer_moves_powers() {
        let s = setup();
        let (leader, heir) = (s.player(), s.player());
        let id = s.found(&leader, "Orion");
        s.join(id, &heir);

        s.run(|| {
            assert_eq!(
                leave_alliance(&s.env, leader.clone()),
                Err(AllianceError::LeaderMustTransfer)
            );
        });
        s.run(|| transfer_leadership(&s.env, leader.clone(), heir.clone()).unwrap());
        s.run(|| {
            assert_eq!(get_alliance(&s.env, id).unwrap().founder, heir);
            assert_eq!(
                get_member_role(&s.env, id, &leader),
                Some(AllianceRole::Officer)
            );
        });
        // The old leader can now leave.
        s.run(|| leave_alliance(&s.env, leader.clone()).unwrap());
        s.run(|| assert_eq!(get_member_role(&s.env, id, &leader), None));
    }

    #[test]
    fn friendlier_relations_need_both_leaders() {
        let s = setup();
        let (la, lb) = (s.player(), s.player());
        let a = s.found(&la, "Orion");
        let b = s.found(&lb, "Lyra");
        let propose = |leader: &Address, target, status| {
            s.run(|| propose_relation(&s.env, leader.clone(), target, status))
        };

        // One-sided pact proposal changes nothing yet.
        assert_eq!(
            propose(&la, b, DiplomaticStatus::Allied),
            Ok(DiplomaticStatus::Neutral)
        );
        s.run(|| {
            assert_eq!(
                get_relation_proposal(&s.env, a, b),
                Some(DiplomaticStatus::Allied)
            );
        });
        // Matching proposal from the other leader applies it.
        assert_eq!(
            propose(&lb, a, DiplomaticStatus::Allied),
            Ok(DiplomaticStatus::Allied)
        );
        s.run(|| {
            assert_eq!(get_relation(&s.env, b, a), DiplomaticStatus::Allied);
            assert_eq!(get_relation_proposal(&s.env, a, b), None);
        });

        // War is unilateral; peace needs agreement again.
        assert_eq!(
            propose(&lb, a, DiplomaticStatus::AtWar),
            Ok(DiplomaticStatus::AtWar)
        );
        assert_eq!(
            propose(&la, b, DiplomaticStatus::Neutral),
            Ok(DiplomaticStatus::AtWar)
        );
        assert_eq!(
            propose(&lb, a, DiplomaticStatus::Neutral),
            Ok(DiplomaticStatus::Neutral)
        );

        // Self, unknown and no-op targets are rejected.
        assert_eq!(
            propose(&la, a, DiplomaticStatus::AtWar),
            Err(AllianceError::InvalidTarget)
        );
        assert_eq!(
            propose(&la, 99, DiplomaticStatus::AtWar),
            Err(AllianceError::AllianceNotFound)
        );
        assert_eq!(
            propose(&la, b, DiplomaticStatus::Neutral),
            Err(AllianceError::InvalidTarget)
        );
    }

    #[test]
    fn objectives_complete_once_and_award_xp() {
        let s = setup();
        let (leader, member, outsider) = (s.player(), s.player(), s.player());
        let id = s.found(&leader, "Orion");
        s.join(id, &member);

        s.run(|| {
            assert_eq!(
                create_objective(
                    &s.env,
                    member.clone(),
                    symbol_short!("mine"),
                    10,
                    1_000,
                    100
                ),
                Err(AllianceError::Unauthorized)
            );
        });
        let obj = s.run(|| {
            create_objective(
                &s.env,
                leader.clone(),
                symbol_short!("mine"),
                10,
                1_000,
                100,
            )
            .unwrap()
        });
        let contribute = |who: &Address, amount| {
            s.run(|| contribute_objective(&s.env, who.clone(), obj, amount))
        };
        assert_eq!(contribute(&outsider, 1), Err(AllianceError::NotMember));
        assert_eq!(contribute(&member, 4).unwrap().progress, 4);
        let done = contribute(&member, 50).unwrap();
        assert_eq!((done.progress, done.completed), (10, true));
        assert_eq!(contribute(&member, 1), Err(AllianceError::ObjectiveClosed));
        s.run(|| {
            assert_eq!(get_alliance_level(&s.env, id).level, 2);
            assert_eq!(get_objective_count(&s.env, id), 1);
        });
    }

    #[test]
    fn objectives_close_at_deadline() {
        let s = setup();
        let leader = s.player();
        s.found(&leader, "Orion");
        s.run(|| {
            assert_eq!(
                create_objective(&s.env, leader.clone(), symbol_short!("x"), 0, 1, 100),
                Err(AllianceError::InvalidObjective)
            );
        });
        let obj = s.run(|| {
            create_objective(&s.env, leader.clone(), symbol_short!("x"), 5, 1, 100).unwrap()
        });
        s.env.ledger().set_timestamp(101);
        s.run(|| {
            assert_eq!(
                contribute_objective(&s.env, leader.clone(), obj, 1),
                Err(AllianceError::ObjectiveClosed)
            );
        });
    }

    #[test]
    fn benefits_scale_with_level_and_apply() {
        let s = setup();
        let leader = s.player();
        let id = s.found(&leader, "Orion");
        s.run(|| {
            assert_eq!(
                get_alliance_benefits(&s.env, id),
                AllianceBenefits {
                    level: 1,
                    member_cap: 50,
                    territory_discount_bps: 0
                }
            );
            add_alliance_xp(&s.env, id, 1_000 + 2_000).unwrap(); // level 3
            let b = get_alliance_benefits(&s.env, id);
            assert_eq!(
                (b.level, b.member_cap, b.territory_discount_bps),
                (3, 60, 500)
            );
            add_alliance_xp(&s.env, id, 1_000_000).unwrap();
            let b = get_alliance_benefits(&s.env, id);
            assert_eq!(
                (b.member_cap, b.territory_discount_bps),
                (MAX_MEMBER_CAP, 2_500)
            );
            credit_alliance_treasury(&s.env, id, 1_000).unwrap();
        });
        s.run(|| claim_guild_territory(&s.env, leader.clone(), 7).unwrap());
        // 25 % discount: 500 -> 375.
        s.run(|| assert_eq!(get_alliance_treasury(&s.env, id), 1_000 - 375));
    }

    #[test]
    fn member_cap_grows_with_level() {
        let s = setup();
        let leader = s.player();
        let id = s.found(&leader, "Orion");
        for _ in 1..MAX_MEMBERS_PER_ALLIANCE {
            s.join(id, &s.player());
        }
        let extra = s.player();
        s.run(|| {
            assert_eq!(
                join_alliance(&s.env, id, extra.clone()).unwrap_err(),
                AllianceError::AllianceFull
            );
        });
        s.run(|| add_alliance_xp(&s.env, id, 1_000).unwrap()); // level 2: cap 55
        s.join(id, &extra);
    }
}
