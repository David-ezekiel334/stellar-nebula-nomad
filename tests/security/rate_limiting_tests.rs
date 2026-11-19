//! Rate limiting effectiveness tests (Issue #610).
//!
//! Verifies the shared `rate_limiter`: per-address and per-operation isolation,
//! window rollover, admin-configurable limits, sane defaults for every
//! operation, and that each DoS vector (combat, tournament registration,
//! crafting, trading, minting, nebula generation) is actually wired to it.

// Test code: pedantic style lints (too_many_lines, semicolon_if_nothing_returned, ...)
// add noise without catching bugs here.
#![allow(clippy::pedantic)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    vec, Address, Env, Symbol,
};
use stellar_nebula_nomad::{
    access_control,
    crafting::{self, CraftingError},
    rate_limiter::{
        check_rate_limit, get_rate_limit_config, set_rate_limit_config, Operation, RateLimitConfig,
        RateLimitError,
    },
    resource_minter::ResourceType,
    tournament::{self, TournamentError},
    trading::{self, AmmError, LimitOrder, OrderSide, TradingError},
    NebulaNomadContract, NebulaNomadContractClient, PvPError,
};

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let id = env.register(NebulaNomadContract, ());
    (env, id)
}

/// Run `f` in its own contract frame. `require_auth` may only be satisfied once per
/// frame for identical arguments, so every authenticated call gets a fresh frame.
fn at<T>(env: &Env, id: &Address, f: impl FnOnce() -> T) -> T {
    env.as_contract(id, f)
}

fn advance(env: &Env, secs: u64) {
    env.ledger().with_mut(|l| l.timestamp += secs);
}

// ── Core limiter behaviour ────────────────────────────────────────────────

#[test]
fn every_operation_has_a_sane_default_limit() {
    for op in Operation::ALL {
        let cfg = op.default_config();
        assert!(cfg.max_calls >= 1, "{op:?} must allow at least one call");
        assert!(cfg.window_seconds >= 1, "{op:?} needs a non-zero window");
        // Legitimate users must not be starved: at least 1 call per hour.
        assert!(
            u64::from(cfg.max_calls) * 3600 / cfg.window_seconds >= 1,
            "{op:?} default is too restrictive"
        );
        // ...and the limit must actually bound abuse: no more than 1 call/sec sustained.
        assert!(
            u64::from(cfg.max_calls) <= cfg.window_seconds,
            "{op:?} default permits more than 1 call/second"
        );
    }
}

#[test]
fn each_operation_is_blocked_after_its_limit_and_recovers_after_the_window() {
    let (env, id) = setup();
    let user = Address::generate(&env);
    env.as_contract(&id, || {
        for op in Operation::ALL {
            let cfg = get_rate_limit_config(&env, op.clone());
            for n in 0..cfg.max_calls {
                assert_eq!(
                    check_rate_limit(&env, &user, op.clone()),
                    Ok(()),
                    "{op:?} call {n}"
                );
            }
            assert_eq!(
                check_rate_limit(&env, &user, op.clone()),
                Err(RateLimitError::RateLimitExceeded),
                "{op:?} must be blocked after {} calls",
                cfg.max_calls
            );
        }
    });
    // Window rolls over: every operation works again.
    advance(&env, 3601);
    env.as_contract(&id, || {
        for op in Operation::ALL {
            assert_eq!(
                check_rate_limit(&env, &user, op.clone()),
                Ok(()),
                "{op:?} after window"
            );
        }
    });
}

#[test]
fn limits_are_per_address_and_per_operation() {
    let (env, id) = setup();
    let (a, b) = (Address::generate(&env), Address::generate(&env));
    env.as_contract(&id, || {
        for _ in 0..Operation::Crafting.default_config().max_calls {
            check_rate_limit(&env, &a, Operation::Crafting).unwrap();
        }
        assert!(check_rate_limit(&env, &a, Operation::Crafting).is_err());
        assert!(
            check_rate_limit(&env, &b, Operation::Crafting).is_ok(),
            "other address"
        );
        assert!(
            check_rate_limit(&env, &a, Operation::Trading).is_ok(),
            "other operation"
        );
    });
}

#[test]
fn admin_can_tune_limits_and_non_admin_cannot() {
    let (env, id) = setup();
    let (admin, intruder, user) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    at(&env, &id, || {
        access_control::init_roles(&env, admin.clone()).unwrap()
    });
    let tight = RateLimitConfig {
        max_calls: 2,
        window_seconds: 120,
    };
    assert_eq!(
        at(&env, &id, || set_rate_limit_config(
            &env,
            &intruder,
            Operation::Trading,
            tight.clone()
        )),
        Err(RateLimitError::Unauthorized)
    );
    assert_eq!(
        at(&env, &id, || set_rate_limit_config(
            &env,
            &admin,
            Operation::Trading,
            RateLimitConfig {
                max_calls: 0,
                window_seconds: 60
            }
        )),
        Err(RateLimitError::InvalidConfig)
    );
    at(&env, &id, || {
        set_rate_limit_config(&env, &admin, Operation::Trading, tight.clone()).unwrap()
    });
    env.as_contract(&id, || {
        assert_eq!(get_rate_limit_config(&env, Operation::Trading), tight);
        // Untouched operations keep their default.
        assert_eq!(
            get_rate_limit_config(&env, Operation::Crafting),
            Operation::Crafting.default_config()
        );
        assert!(check_rate_limit(&env, &user, Operation::Trading).is_ok());
        assert!(check_rate_limit(&env, &user, Operation::Trading).is_ok());
        assert_eq!(
            check_rate_limit(&env, &user, Operation::Trading),
            Err(RateLimitError::RateLimitExceeded)
        );
    });
}

#[test]
fn huge_window_does_not_overflow() {
    let (env, id) = setup();
    let (admin, user) = (Address::generate(&env), Address::generate(&env));
    at(&env, &id, || {
        access_control::init_roles(&env, admin.clone()).unwrap()
    });
    at(&env, &id, || {
        set_rate_limit_config(
            &env,
            &admin,
            Operation::Crafting,
            RateLimitConfig {
                max_calls: 1,
                window_seconds: u64::MAX,
            },
        )
        .unwrap()
    });
    env.as_contract(&id, || {
        assert!(check_rate_limit(&env, &user, Operation::Crafting).is_ok());
        assert!(check_rate_limit(&env, &user, Operation::Crafting).is_err());
    });
}

// ── Each DoS vector is wired to the limiter ───────────────────────────────

#[test]
fn crafting_is_rate_limited() {
    let (env, id) = setup();
    let player = Address::generate(&env);
    let max = Operation::Crafting.default_config().max_calls;
    let craft = || {
        at(&env, &id, || {
            crafting::craft(env.clone(), player.clone(), 9_999)
        })
    };
    for _ in 0..max {
        // Unknown recipe: fails *after* the limiter, proving the call was counted.
        assert_eq!(craft(), Err(CraftingError::RecipeNotFound));
    }
    assert_eq!(craft(), Err(CraftingError::RateLimitExceeded));
    assert_eq!(
        at(&env, &id, || crafting::craft_with_overcharge(
            env.clone(),
            player.clone(),
            9_999
        )),
        Err(CraftingError::RateLimitExceeded),
        "overcharge variant shares the crafting budget"
    );
    // Budget refills once the window passes.
    advance(&env, 61);
    assert_eq!(craft(), Err(CraftingError::RecipeNotFound));
}

#[test]
fn tournament_registration_is_rate_limited() {
    let (env, id) = setup();
    let (organizer, player, other) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let max = Operation::TournamentRegistration.default_config().max_calls;
    let mut ids = std::vec::Vec::new();
    for _ in 0..=max {
        ids.push(at(&env, &id, || {
            tournament::create_tournament(
                &env,
                &organizer,
                ResourceType::StellarDust,
                0,
                4,
                10_000,
                vec![&env, 10_000u32],
            )
            .unwrap()
        }));
    }
    let reg = |who: &Address, t: u64| {
        at(&env, &id, || {
            tournament::register_for_tournament(&env, who, t)
        })
    };
    for t in ids.iter().take(max as usize) {
        assert!(reg(&player, *t).is_ok());
    }
    assert_eq!(
        reg(&player, ids[max as usize]),
        Err(TournamentError::RateLimitExceeded)
    );
    // A different player is unaffected.
    assert!(reg(&other, ids[max as usize]).is_ok());
}

#[test]
fn trading_is_rate_limited() {
    let (env, id) = setup();
    let trader = Address::generate(&env);
    let max = Operation::Trading.default_config().max_calls;
    let dust = Symbol::new(&env, "dust");
    let order = || LimitOrder {
        id: 0,
        trader: trader.clone(),
        side: OrderSide::Buy,
        resource: dust.clone(),
        quantity: 0, // invalid on purpose: rejected after the limiter ran
        limit_price: 1,
        placed_at: 0,
        is_stop_loss: false,
    };
    for _ in 0..max {
        let r = at(&env, &id, || {
            trading::place_limit_order(&env, &trader, order())
        });
        assert_ne!(r, Err(TradingError::RateLimitExceeded));
    }
    assert_eq!(
        at(&env, &id, || trading::place_limit_order(
            &env,
            &trader,
            order()
        )),
        Err(TradingError::RateLimitExceeded)
    );
    // AMM entry points draw from the same Trading budget.
    assert_eq!(
        at(&env, &id, || trading::swap_exact_input(
            &env,
            &trader,
            dust.clone(),
            1,
            0,
            vec![&env, 1u64]
        )),
        Err(AmmError::RateLimitExceeded)
    );
    assert_eq!(
        at(&env, &id, || trading::create_pool(
            &env,
            &trader,
            Symbol::new(&env, "a"),
            Symbol::new(&env, "b")
        )),
        Err(AmmError::RateLimitExceeded)
    );
    assert_eq!(
        at(&env, &id, || trading::add_liquidity(&env, &trader, 1, 1, 1)).err(),
        Some(AmmError::RateLimitExceeded)
    );
}

#[test]
fn pvp_challenges_are_rate_limited() {
    let (env, id) = setup();
    let client = NebulaNomadContractClient::new(&env, &id);
    let challenger = Address::generate(&env);
    let max = Operation::CombatInitiation.default_config().max_calls;
    for _ in 0..max {
        let opponent = Address::generate(&env);
        // Each distinct challenge succeeds or fails on its own merits, never on the limiter.
        let r = client.try_create_challenge(&challenger, &opponent, &0);
        assert!(!matches!(r, Err(Ok(PvPError::RateLimitExceeded))), "{r:?}");
    }
    let opponent = Address::generate(&env);
    assert_eq!(
        client.try_create_challenge(&challenger, &opponent, &0),
        Err(Ok(PvPError::RateLimitExceeded))
    );
    // Accepting is also budgeted, and other players are unaffected.
    assert_eq!(
        client.try_accept_challenge(&challenger, &1),
        Err(Ok(PvPError::RateLimitExceeded))
    );
    let fresh = Address::generate(&env);
    let r = client.try_create_challenge(&fresh, &opponent, &0);
    assert!(!matches!(r, Err(Ok(PvPError::RateLimitExceeded))));
}

// ── Coverage guard ────────────────────────────────────────────────────────

/// Every rate-limited operation must be enforced somewhere outside
/// `rate_limiter.rs`; otherwise a new variant could be declared but never used.
#[test]
fn every_operation_is_enforced_by_some_module() {
    use std::{fs, path::Path};
    fn collect(dir: &Path, out: &mut String) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs")
                && p.file_name().unwrap() != "rate_limiter.rs"
            {
                out.push_str(&fs::read_to_string(&p).unwrap());
            }
        }
    }
    let mut src = String::new();
    collect(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut src);
    // `NebulaScan` is a reserved budget: scans currently share `NebulaGeneration`.
    let reserved = ["NebulaScan"];
    for op in Operation::ALL {
        let name = format!("{op:?}");
        if reserved.contains(&name.as_str()) {
            continue;
        }
        assert!(
            src.contains(&format!("Operation::{name}")),
            "Operation::{name} is declared but never enforced"
        );
    }
}
