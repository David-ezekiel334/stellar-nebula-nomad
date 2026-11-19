//! Per-address, per-operation rate limiting (Issues #175, #610, #585).
//!
//! Every expensive or abusable entry point calls [`check_rate_limit`] right
//! after `require_auth()`. Each `(caller, Operation)` pair gets a fixed window
//! of `max_calls` per `window_seconds`; the counter lives in temporary storage
//! so it expires on its own. Exceeding the window returns
//! [`RateLimitError::RateLimitExceeded`] and emits a `("RateLimit", "hit")`
//! event so indexers can spot abuse.
//!
//! Admins (RBAC `admin` role) can override the built-in defaults per operation
//! with [`set_rate_limit_config`]; [`get_rate_limit_config`] returns the
//! effective limit. See `docs/RATE_LIMITING.md`.
//!
//! The module is declared `pub mod rate_limiter;` in `lib.rs` and imported as
//! `crate::rate_limiter` by every enforcing module (Issue #585).

use crate::access_control;
use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env};

// ── Operation kinds ───────────────────────────────────────────
/// Every expensive operation that requires rate limiting.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// `generate_nebula_layout` — CPU + storage intensive.
    NebulaGeneration,
    /// `mint_resource` — token minting on-ledger.
    ResourceMinting,
    /// `upgrade_ship` — ship NFT state mutation.
    ShipUpgrade,
    /// `scan_nebula` — nebula scan operation.
    NebulaScan,
    /// `batch_operation` — batch processing operations.
    BatchOperation,
    /// `privacy_commit` — privacy stat commitments.
    PrivacyCommit,
    /// `route_calculation` — navigation route calculation.
    RouteCalculation,
    /// `repair_ship` — paid hull repair, a resource sink (Issue #453).
    ShipRepair,
    /// PvP challenge creation / acceptance and matchmaking queue joins
    /// (Issue #610). Combat start is only reachable through these.
    CombatInitiation,
    /// `register_for_tournament` — entry-fee escrow plus registrant list growth.
    TournamentRegistration,
    /// `craft` / `craft_with_overcharge` — recipe execution (Issue #610).
    Crafting,
    /// Limit orders, AMM swaps and liquidity changes (Issue #610).
    Trading,
}

impl Operation {
    /// Every rate-limited operation, for enumeration in tests and tooling.
    pub const ALL: [Operation; 12] = [
        Operation::NebulaGeneration,
        Operation::ResourceMinting,
        Operation::ShipUpgrade,
        Operation::NebulaScan,
        Operation::BatchOperation,
        Operation::PrivacyCommit,
        Operation::RouteCalculation,
        Operation::ShipRepair,
        Operation::CombatInitiation,
        Operation::TournamentRegistration,
        Operation::Crafting,
        Operation::Trading,
    ];

    /// Built-in limit used until an admin overrides it with
    /// [`set_rate_limit_config`].
    pub fn default_config(&self) -> RateLimitConfig {
        match self {
            Operation::NebulaGeneration => RateLimitConfig::default_nebula_generation(),
            Operation::ResourceMinting => RateLimitConfig::default_resource_minting(),
            Operation::ShipUpgrade => RateLimitConfig::default_ship_upgrade(),
            Operation::NebulaScan => RateLimitConfig::default_nebula_scan(),
            Operation::BatchOperation => RateLimitConfig::default_batch_operation(),
            Operation::PrivacyCommit => RateLimitConfig::default_privacy_commit(),
            Operation::RouteCalculation => RateLimitConfig::default_route_calculation(),
            Operation::ShipRepair => RateLimitConfig::default_ship_repair(),
            Operation::CombatInitiation => RateLimitConfig::default_combat_initiation(),
            Operation::TournamentRegistration => RateLimitConfig::default_tournament_registration(),
            Operation::Crafting => RateLimitConfig::default_crafting(),
            Operation::Trading => RateLimitConfig::default_trading(),
        }
    }
}

// ── Config types ──────────────────────────────────────────────
/// Rate limit configuration for a single operation type.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    /// Maximum calls allowed within `window_seconds`.
    pub max_calls: u32,
    /// Rolling window length in seconds.
    pub window_seconds: u64,
}

impl RateLimitConfig {
    pub fn default_nebula_generation() -> Self {
        Self {
            max_calls: 5,
            window_seconds: 60,
        } // 5 scans / minute
    }
    pub fn default_resource_minting() -> Self {
        Self {
            max_calls: 10,
            window_seconds: 60,
        } // 10 mints / minute
    }
    pub fn default_ship_upgrade() -> Self {
        Self {
            max_calls: 3,
            window_seconds: 300,
        } // 3 upgrades / 5 min
    }
    pub fn default_nebula_scan() -> Self {
        Self {
            max_calls: 10,
            window_seconds: 3600,
        } // 10 scans / hour
    }
    pub fn default_batch_operation() -> Self {
        Self {
            max_calls: 5,
            window_seconds: 3600,
        } // 5 batch ops / hour
    }
    pub fn default_privacy_commit() -> Self {
        Self {
            max_calls: 20,
            window_seconds: 3600,
        } // 20 commits / hour
    }
    pub fn default_route_calculation() -> Self {
        Self {
            max_calls: 15,
            window_seconds: 3600,
        } // 15 route calcs / hour
    }
    /// Repairs are priced per durability point and are the game's main
    /// resource sink (Issue #453), so the budget is deliberately tight.
    pub fn default_ship_repair() -> Self {
        Self {
            max_calls: 3,
            window_seconds: 300,
        } // 3 repairs / 5 min
    }
    /// Challenges, accepts and queue joins: a duel is minutes long, so 10 per
    /// 5 minutes never blocks real play but stops challenge-spam.
    pub fn default_combat_initiation() -> Self {
        Self {
            max_calls: 10,
            window_seconds: 300,
        } // 10 initiations / 5 min
    }
    /// Players register for a handful of tournaments at a time.
    pub fn default_tournament_registration() -> Self {
        Self {
            max_calls: 5,
            window_seconds: 3600,
        } // 5 registrations / hour
    }
    /// Crafting is a core loop, so the budget is generous but bounded.
    pub fn default_crafting() -> Self {
        Self {
            max_calls: 20,
            window_seconds: 60,
        } // 20 crafts / minute
    }
    /// Active traders place several orders/swaps per minute.
    pub fn default_trading() -> Self {
        Self {
            max_calls: 30,
            window_seconds: 60,
        } // 30 trades / minute
    }
}

// ── Storage key ───────────────────────────────────────────────
#[contracttype]
pub enum RateLimitKey {
    /// Tracks (call_count, window_start) per address + operation.
    Entry(Address, Operation),
    /// Admin-configurable limits per operation.
    RateLimitConfigKey(Operation),
}

// ── Error ─────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RateLimitError {
    /// Caller has exceeded the allowed call rate for this operation.
    RateLimitExceeded = 100,
    /// Only the contract admin may update rate limit configuration.
    Unauthorized = 101,
    /// `max_calls` and `window_seconds` must both be non-zero.
    InvalidConfig = 102,
}

impl crate::error_standard::StandardContractError for RateLimitError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::RateLimitExceeded => (ErrorKind::ResourceLimit, true),
            Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::InvalidConfig => (ErrorKind::Validation, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "rate_limiter",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ── Per-address window state ──────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowState {
    /// Number of calls made within the current window.
    pub call_count: u32,
    /// Ledger timestamp when the current window started.
    pub window_start: u64,
}

// ── Core rate-limiter logic ───────────────────────────────────

/// Check and increment the rate-limit counter for `caller` + `op`.
///
/// Returns `Ok(())` if the call is within limits, or
/// `Err(RateLimitError::RateLimitExceeded)` and emits a
/// `RateLimitHit` event if not.
pub fn check_rate_limit(env: &Env, caller: &Address, op: Operation) -> Result<(), RateLimitError> {
    let config: RateLimitConfig = env
        .storage()
        .instance()
        .get(&RateLimitKey::RateLimitConfigKey(op.clone()))
        .unwrap_or_else(|| op.default_config());

    let now = env.ledger().timestamp();
    let entry_key = RateLimitKey::Entry(caller.clone(), op.clone());

    let mut state: WindowState = env
        .storage()
        .temporary()
        .get(&entry_key)
        .unwrap_or(WindowState {
            call_count: 0,
            window_start: now,
        });

    // Roll window forward if it has expired
    if now >= state.window_start.saturating_add(config.window_seconds) {
        state = WindowState {
            call_count: 0,
            window_start: now,
        };
    }

    if state.call_count >= config.max_calls {
        // Emit RateLimitHit event (Issue #175 acceptance criterion)
        env.events().publish(
            (symbol_short!("RateLimit"), symbol_short!("hit")),
            (caller.clone(), op, state.call_count, config.max_calls),
        );
        return Err(RateLimitError::RateLimitExceeded);
    }

    // Increment and persist — TTL = window_seconds + 1 ledger
    state.call_count = state.call_count.saturating_add(1);
    env.storage().temporary().set(&entry_key, &state);

    Ok(())
}

/// Effective limit for `op`: the admin override if set, else the built-in default.
pub fn get_rate_limit_config(env: &Env, op: Operation) -> RateLimitConfig {
    env.storage()
        .instance()
        .get(&RateLimitKey::RateLimitConfigKey(op.clone()))
        .unwrap_or_else(|| op.default_config())
}

/// Admin function: update rate limit config for an operation.
///
/// `admin` must authorize the call and hold the RBAC `admin` role
/// (`access_control::init_roles` must have been called).
pub fn set_rate_limit_config(
    env: &Env,
    admin: &Address,
    op: Operation,
    config: RateLimitConfig,
) -> Result<(), RateLimitError> {
    admin.require_auth();
    if !access_control::has_role(env, &access_control::admin_role(), admin) {
        return Err(RateLimitError::Unauthorized);
    }
    if config.max_calls == 0 || config.window_seconds == 0 {
        return Err(RateLimitError::InvalidConfig);
    }
    env.storage()
        .instance()
        .set(&RateLimitKey::RateLimitConfigKey(op), &config);
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// Tests — Issue #175
// ─────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::in_contract;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    fn make_env() -> Env {
        let env = crate::test_helpers::contract_env();
        env.mock_all_auths();
        env
    }

    #[test]
    fn test_calls_within_limit_succeed() {
        let env = make_env();
        let caller = Address::generate(&env);

        // Default: 5 calls / 60 s for NebulaGeneration
        for _ in 0..5 {
            assert!(in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::NebulaGeneration
            ))
            .is_ok());
        }
    }

    #[test]
    fn test_call_beyond_limit_fails() {
        let env = make_env();
        let caller = Address::generate(&env);

        for _ in 0..5 {
            in_contract(&env, || {
                check_rate_limit(&env, &caller, Operation::NebulaGeneration)
            })
            .ok();
        }
        let result = in_contract(&env, || {
            check_rate_limit(&env, &caller, Operation::NebulaGeneration)
        });
        assert_eq!(result, Err(RateLimitError::RateLimitExceeded));
    }

    #[test]
    fn test_different_addresses_have_independent_limits() {
        let env = make_env();
        let caller1 = Address::generate(&env);
        let caller2 = Address::generate(&env);

        // Exhaust caller1's limit
        for _ in 0..5 {
            in_contract(&env, || {
                check_rate_limit(&env, &caller1, Operation::NebulaGeneration)
            })
            .ok();
        }
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &caller1,
                Operation::NebulaGeneration
            )),
            Err(RateLimitError::RateLimitExceeded)
        );
        // caller2 is unaffected
        assert!(in_contract(&env, || check_rate_limit(
            &env,
            &caller2,
            Operation::NebulaGeneration
        ))
        .is_ok());
    }

    #[test]
    fn test_different_operations_have_independent_limits() {
        let env = make_env();
        let caller = Address::generate(&env);

        // Exhaust NebulaGeneration (5 calls)
        for _ in 0..5 {
            in_contract(&env, || {
                check_rate_limit(&env, &caller, Operation::NebulaGeneration)
            })
            .ok();
        }
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::NebulaGeneration
            )),
            Err(RateLimitError::RateLimitExceeded)
        );
        // ResourceMinting limit is independent (10 calls / 60 s)
        assert!(in_contract(&env, || check_rate_limit(
            &env,
            &caller,
            Operation::ResourceMinting
        ))
        .is_ok());
    }

    #[test]
    fn test_custom_config_respected() {
        let env = make_env();
        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        in_contract(&env, || access_control::init_roles(&env, admin.clone())).unwrap();

        // Set a very tight limit: 2 calls / 120 s
        in_contract(&env, || {
            set_rate_limit_config(
                &env,
                &admin,
                Operation::ResourceMinting,
                RateLimitConfig {
                    max_calls: 2,
                    window_seconds: 120,
                },
            )
        })
        .unwrap();

        assert!(in_contract(&env, || check_rate_limit(
            &env,
            &user,
            Operation::ResourceMinting
        ))
        .is_ok());
        assert!(in_contract(&env, || check_rate_limit(
            &env,
            &user,
            Operation::ResourceMinting
        ))
        .is_ok());
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &user,
                Operation::ResourceMinting
            )),
            Err(RateLimitError::RateLimitExceeded)
        );
    }

    #[test]
    fn test_set_config_rejects_non_admin() {
        let env = make_env();
        let admin = Address::generate(&env);
        let intruder = Address::generate(&env);
        in_contract(&env, || access_control::init_roles(&env, admin)).unwrap();

        assert_eq!(
            in_contract(&env, || set_rate_limit_config(
                &env,
                &intruder,
                Operation::ResourceMinting,
                RateLimitConfig {
                    max_calls: 1000,
                    window_seconds: 1
                },
            )),
            Err(RateLimitError::Unauthorized)
        );
    }

    #[test]
    fn test_set_config_rejects_zero_values() {
        let env = make_env();
        let admin = Address::generate(&env);
        in_contract(&env, || access_control::init_roles(&env, admin.clone())).unwrap();

        assert_eq!(
            in_contract(&env, || set_rate_limit_config(
                &env,
                &admin,
                Operation::ResourceMinting,
                RateLimitConfig {
                    max_calls: 0,
                    window_seconds: 60
                },
            )),
            Err(RateLimitError::InvalidConfig)
        );
        assert_eq!(
            in_contract(&env, || set_rate_limit_config(
                &env,
                &admin,
                Operation::ResourceMinting,
                RateLimitConfig {
                    max_calls: 5,
                    window_seconds: 0
                },
            )),
            Err(RateLimitError::InvalidConfig)
        );
    }

    #[test]
    fn test_ship_upgrade_default_limit() {
        let env = make_env();
        let caller = Address::generate(&env);

        for _ in 0..3 {
            assert!(in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::ShipUpgrade
            ))
            .is_ok());
        }
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::ShipUpgrade
            )),
            Err(RateLimitError::RateLimitExceeded)
        );
    }

    // ── Issue #585: module integration ──────────────────────────────────

    fn advance_time(env: &Env, seconds: u64) {
        use soroban_sdk::testutils::Ledger as _;
        let now = env.ledger().timestamp();
        env.ledger().set_timestamp(now + seconds);
    }

    #[test]
    fn test_window_resets_after_expiry() {
        let env = make_env();
        let caller = Address::generate(&env);
        let window = Operation::NebulaGeneration.default_config().window_seconds;

        for _ in 0..5 {
            in_contract(&env, || {
                check_rate_limit(&env, &caller, Operation::NebulaGeneration).unwrap();
            });
        }
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::NebulaGeneration
            )),
            Err(RateLimitError::RateLimitExceeded)
        );

        // One second short of the window: still blocked.
        advance_time(&env, window - 1);
        assert_eq!(
            in_contract(&env, || check_rate_limit(
                &env,
                &caller,
                Operation::NebulaGeneration
            )),
            Err(RateLimitError::RateLimitExceeded)
        );

        // Window elapsed: a fresh budget is available.
        advance_time(&env, 1);
        assert!(in_contract(&env, || check_rate_limit(
            &env,
            &caller,
            Operation::NebulaGeneration
        ))
        .is_ok());
    }

    #[test]
    fn test_get_config_returns_default_then_override() {
        let env = make_env();
        let admin = Address::generate(&env);
        in_contract(&env, || access_control::init_roles(&env, admin.clone())).unwrap();

        let default = Operation::Trading.default_config();
        assert_eq!(
            in_contract(&env, || get_rate_limit_config(&env, Operation::Trading)),
            default
        );

        let custom = RateLimitConfig {
            max_calls: 7,
            window_seconds: 90,
        };
        in_contract(&env, || {
            set_rate_limit_config(&env, &admin, Operation::Trading, custom.clone())
        })
        .unwrap();
        assert_eq!(
            in_contract(&env, || get_rate_limit_config(&env, Operation::Trading)),
            custom
        );
        // Other operations keep their defaults.
        assert_eq!(
            in_contract(&env, || get_rate_limit_config(&env, Operation::Crafting)),
            Operation::Crafting.default_config()
        );
    }

    #[test]
    fn test_every_operation_has_a_usable_default() {
        for op in Operation::ALL {
            let cfg = op.default_config();
            assert!(cfg.max_calls > 0, "{op:?} has max_calls == 0");
            assert!(cfg.window_seconds > 0, "{op:?} has window_seconds == 0");
        }
    }

    #[test]
    fn test_limit_hit_emits_event() {
        use soroban_sdk::testutils::Events as _;
        let env = make_env();
        let caller = Address::generate(&env);

        for _ in 0..3 {
            in_contract(&env, || {
                check_rate_limit(&env, &caller, Operation::ShipUpgrade).unwrap();
            });
        }
        let result = in_contract(&env, || {
            let r = check_rate_limit(&env, &caller, Operation::ShipUpgrade);
            // Events are scoped to the frame that emitted them.
            assert!(!env.events().all().events().is_empty());
            r
        });
        assert_eq!(result, Err(RateLimitError::RateLimitExceeded));
    }
}
