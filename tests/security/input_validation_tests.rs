//! Input validation tests (Issue #611).
//!
//! Feeds invalid, boundary and overflow-inducing inputs to the validation
//! helpers/macros and to the tournament, PvP, crafting and trading entry points.
//! Every case must be rejected with a typed error: none may panic.

// Test code: pedantic style lints (too_many_lines, semicolon_if_nothing_returned, ...)
// add noise without catching bugs here.
#![allow(clippy::pedantic)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    vec, Address, Env, String, Symbol, Vec,
};
use stellar_nebula_nomad::{
    crafting,
    input_validation::{
        check_bps_distribution, check_distinct_addresses, check_non_negative, check_positive,
        check_range, checked_deadline, safe_add, safe_mul, validate_name, validate_region_id,
        ValidationError, MAX_NAME_LENGTH,
    },
    resource_minter::ResourceType,
    tournament::{
        self, TournamentError, MAX_REGISTRATION_WINDOW_SECS, MIN_REGISTRATION_WINDOW_SECS,
    },
    trading::{self, AmmError, LimitOrder, OrderSide, TradingError, MAX_ORDER_VALUE},
    NebulaNomadContract, NebulaNomadContractClient, PvPError,
};

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let id = env.register(NebulaNomadContract, ());
    (env, id)
}

/// One contract frame per authenticated call (see rate_limiting_tests.rs).
fn at<T>(env: &Env, id: &Address, f: impl FnOnce() -> T) -> T {
    env.as_contract(id, f)
}

// ── Helpers ───────────────────────────────────────────────────────────────

#[test]
fn range_positive_and_non_negative_checks() {
    assert_eq!(check_range(5, 1, 10), Ok(()));
    assert_eq!(check_range(0, 1, 10), Err(ValidationError::OutOfRange));
    assert_eq!(check_range(11, 1, 10), Err(ValidationError::OutOfRange));
    assert_eq!(
        check_range(i128::MAX, 0, i128::MAX - 1),
        Err(ValidationError::OutOfRange)
    );
    assert_eq!(check_positive(1), Ok(()));
    assert_eq!(check_positive(0), Err(ValidationError::NotPositive));
    assert_eq!(check_positive(i128::MIN), Err(ValidationError::NotPositive));
    assert_eq!(check_non_negative(0), Ok(()));
    assert_eq!(check_non_negative(-1), Err(ValidationError::OutOfRange));
}

#[test]
fn distinct_addresses_rejects_self_reference() {
    let env = Env::default();
    let (a, b) = (Address::generate(&env), Address::generate(&env));
    assert_eq!(check_distinct_addresses(&a, &b), Ok(()));
    assert_eq!(
        check_distinct_addresses(&a, &a),
        Err(ValidationError::SelfReference)
    );
}

#[test]
fn checked_deadline_bounds_and_overflow() {
    assert_eq!(checked_deadline(100, 60, 60, 120), Ok(160));
    assert_eq!(
        checked_deadline(100, 59, 60, 120),
        Err(ValidationError::InvalidTimestamp)
    );
    assert_eq!(
        checked_deadline(100, 121, 60, 120),
        Err(ValidationError::InvalidTimestamp)
    );
    // now + window would overflow u64 even though the window is within bounds.
    assert_eq!(
        checked_deadline(u64::MAX, 60, 60, 120),
        Err(ValidationError::InvalidTimestamp)
    );
    assert_eq!(
        checked_deadline(0, u64::MAX, 0, u64::MAX),
        Ok(u64::MAX),
        "exactly representable deadline is fine"
    );
}

#[test]
fn bps_distribution_rejects_overflow_and_over_100_percent() {
    let env = Env::default();
    assert_eq!(
        check_bps_distribution(&vec![&env, 5_000u32, 5_000], 4),
        Ok(())
    );
    assert_eq!(check_bps_distribution(&Vec::<u32>::new(&env), 4), Ok(()));
    assert_eq!(
        check_bps_distribution(&vec![&env, 5_000u32, 5_001], 4),
        Err(ValidationError::InvalidBasisPoints)
    );
    assert_eq!(
        check_bps_distribution(&vec![&env, u32::MAX, 1u32], 4),
        Err(ValidationError::InvalidBasisPoints),
        "summing must not overflow"
    );
    assert_eq!(
        check_bps_distribution(&vec![&env, 1u32, 1, 1], 2),
        Err(ValidationError::InvalidBasisPoints),
        "more entries than allowed"
    );
}

#[test]
fn safe_math_reports_overflow_instead_of_panicking() {
    assert_eq!(safe_mul(3, 4), Ok(12));
    assert_eq!(
        safe_mul(i128::MAX, 2),
        Err(ValidationError::ArithmeticOverflow)
    );
    assert_eq!(
        safe_add(i128::MAX, 1),
        Err(ValidationError::ArithmeticOverflow)
    );
    assert_eq!(safe_add(1, 2), Ok(3));
}

#[test]
fn names_and_region_ids_reject_bad_input() {
    let env = Env::default();
    assert_eq!(
        validate_name(&env, &String::from_str(&env, "Voyager")),
        Ok(())
    );
    assert_eq!(
        validate_name(&env, &String::from_str(&env, "")),
        Err(ValidationError::EmptyString)
    );
    let long = "x".repeat(MAX_NAME_LENGTH as usize + 1);
    assert_eq!(
        validate_name(&env, &String::from_str(&env, &long)),
        Err(ValidationError::StringTooLong)
    );
    assert_eq!(
        validate_name(&env, &String::from_str(&env, "bad\u{0}name")),
        Err(ValidationError::InvalidCharacters)
    );
    assert_eq!(validate_region_id(0), Err(ValidationError::InvalidRegionId));
    assert_eq!(
        validate_region_id(u32::MAX),
        Err(ValidationError::InvalidRegionId)
    );
}

// ── Macros ────────────────────────────────────────────────────────────────

fn macro_checks(value: i128, pct: u32) -> Result<i128, ValidationError> {
    stellar_nebula_nomad::validate_non_negative!(value);
    stellar_nebula_nomad::validate_percentage!(pct);
    let doubled = stellar_nebula_nomad::safe_mul!(value, 2);
    stellar_nebula_nomad::ensure!(doubled != 42, ValidationError::OutOfRange);
    Ok(doubled)
}

#[test]
fn validation_macros_short_circuit_with_typed_errors() {
    assert_eq!(macro_checks(10, 50), Ok(20));
    assert_eq!(macro_checks(-1, 50), Err(ValidationError::OutOfRange));
    assert_eq!(
        macro_checks(10, 101),
        Err(ValidationError::InvalidPercentage)
    );
    assert_eq!(
        macro_checks(i128::MAX, 1),
        Err(ValidationError::ArithmeticOverflow)
    );
    assert_eq!(macro_checks(21, 1), Err(ValidationError::OutOfRange));
}

// ── Tournament ────────────────────────────────────────────────────────────

fn create(
    env: &Env,
    id: &Address,
    organizer: &Address,
    fee: u64,
    max_players: u32,
    window: u64,
    bps: Vec<u32>,
) -> Result<u64, TournamentError> {
    at(env, id, || {
        tournament::create_tournament(
            env,
            organizer,
            ResourceType::StellarDust,
            fee,
            max_players,
            window,
            bps,
        )
    })
}

#[test]
fn tournament_creation_rejects_invalid_inputs() {
    let (env, id) = setup();
    let org = Address::generate(&env);
    let ok_bps = || vec![&env, 6_000u32, 4_000];
    let ok_window = MIN_REGISTRATION_WINDOW_SECS;

    assert!(create(&env, &id, &org, 0, 4, ok_window, ok_bps()).is_ok());
    for bad_players in [0u32, 1, 3, 5, 63, 65, 128, u32::MAX] {
        assert_eq!(
            create(&env, &id, &org, 0, bad_players, ok_window, ok_bps()),
            Err(TournamentError::InvalidPlayerCount),
            "players={bad_players}"
        );
    }
    for bad_window in [
        0,
        MIN_REGISTRATION_WINDOW_SECS - 1,
        MAX_REGISTRATION_WINDOW_SECS + 1,
        u64::MAX,
    ] {
        assert_eq!(
            create(&env, &id, &org, 0, 4, bad_window, ok_bps()),
            Err(TournamentError::InvalidRegistrationWindow),
            "window={bad_window}"
        );
    }
    assert!(create(
        &env,
        &id,
        &org,
        0,
        4,
        MAX_REGISTRATION_WINDOW_SECS,
        ok_bps()
    )
    .is_ok());
    for bad_bps in [
        vec![&env, 10_001u32],
        vec![&env, 5_000u32, 5_001],
        vec![&env, u32::MAX, u32::MAX],
        vec![&env, 1u32, 1, 1, 1, 1], // more placements than players
    ] {
        assert_eq!(
            create(&env, &id, &org, 0, 4, ok_window, bad_bps),
            Err(TournamentError::InvalidPrizeDistribution)
        );
    }
}

#[test]
fn tournament_registration_rejects_unknown_ids_double_signup_and_no_funds() {
    let (env, id) = setup();
    let (org, player) = (Address::generate(&env), Address::generate(&env));
    let reg = |t: u64| {
        at(&env, &id, || {
            tournament::register_for_tournament(&env, &player, t)
        })
    };

    assert_eq!(reg(0), Err(TournamentError::TournamentNotFound));
    assert_eq!(reg(u64::MAX), Err(TournamentError::TournamentNotFound));

    let free = create(&env, &id, &org, 0, 4, 3_600, vec![&env, 10_000u32]).unwrap();
    assert_eq!(reg(free), Ok(0));
    assert_eq!(reg(free), Err(TournamentError::AlreadyRegistered));

    // Entry fee the player cannot pay: a funds error, not "registration closed".
    let paid = create(&env, &id, &org, u64::MAX, 4, 3_600, vec![&env, 10_000u32]).unwrap();
    assert_eq!(reg(paid), Err(TournamentError::InsufficientFunds));

    // After the deadline registration is closed.
    env.ledger().with_mut(|l| l.timestamp += 3_601);
    let late = Address::generate(&env);
    assert_eq!(
        at(&env, &id, || tournament::register_for_tournament(
            &env, &late, free
        )),
        Err(TournamentError::RegistrationClosed)
    );
}

// ── PvP ───────────────────────────────────────────────────────────────────

#[test]
fn pvp_challenge_rejects_self_challenge_and_bad_stakes() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b) = (Address::generate(&env), Address::generate(&env));

    assert_eq!(
        c.try_create_challenge(&a, &a, &0),
        Err(Ok(PvPError::InvalidCombatParams))
    );
    assert_eq!(
        c.try_create_challenge(&a, &b, &-1),
        Err(Ok(PvPError::InvalidCombatParams))
    );
    assert_eq!(
        c.try_create_challenge(&a, &b, &i128::MIN),
        Err(Ok(PvPError::InvalidCombatParams))
    );
    assert_eq!(
        c.try_create_challenge(&a, &b, &i128::MAX),
        Err(Ok(PvPError::InvalidCombatParams))
    );
    assert!(c.try_create_challenge(&a, &b, &1_000).is_ok());
}

#[test]
fn pvp_moves_reject_zero_and_oversized_power() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b) = (Address::generate(&env), Address::generate(&env));
    let challenge = c.create_challenge(&a, &b, &0);
    let combat = c.accept_challenge(&b, &challenge);
    let attack = Symbol::new(&env, "attack");

    for power in [0u32, 101, 1_000, u32::MAX / 2 + 1, u32::MAX] {
        for who in [&a, &b] {
            assert_eq!(
                c.try_execute_combat_move(who, &combat, &attack, &power),
                Err(Ok(PvPError::InvalidMove)),
                "power={power}"
            );
        }
    }
    // Unknown combat ids are reported, not trapped.
    assert_eq!(
        c.try_execute_combat_move(&a, &u64::MAX, &attack, &10),
        Err(Ok(PvPError::CombatNotFound))
    );
}

// ── Crafting ──────────────────────────────────────────────────────────────

#[test]
fn crafting_unknown_recipes_and_xp_overflow_do_not_panic() {
    let (env, id) = setup();
    let p = Address::generate(&env);
    for recipe in [0u32, 1_000_000, u32::MAX] {
        let r = at(&env, &id, || {
            crafting::craft(env.clone(), p.clone(), recipe)
        });
        assert!(r.is_err(), "recipe {recipe} should not exist");
    }
    env.as_contract(&id, || {
        crafting::add_xp(&env, p.clone(), u32::MAX);
        crafting::add_xp(&env, p.clone(), u32::MAX); // would overflow without saturation
        assert_eq!(crafting::get_xp(&env, p.clone()), u32::MAX);
    });
}

// ── Trading ───────────────────────────────────────────────────────────────

fn order(env: &Env, trader: &Address, qty: i128, price: i128) -> LimitOrder {
    LimitOrder {
        id: 0,
        trader: trader.clone(),
        side: OrderSide::Buy,
        resource: Symbol::new(env, "dust"),
        quantity: qty,
        limit_price: price,
        placed_at: 0,
        is_stop_loss: false,
    }
}

#[test]
fn limit_orders_reject_non_positive_and_oversized_values() {
    let (env, id) = setup();
    let t = Address::generate(&env);
    let place = |q: i128, p: i128| {
        at(&env, &id, || {
            trading::place_limit_order(&env, &t, order(&env, &t, q, p))
        })
    };
    assert_eq!(place(1, 0), Err(TradingError::InvalidPrice));
    assert_eq!(place(1, -5), Err(TradingError::InvalidPrice));
    assert_eq!(place(1, i128::MIN), Err(TradingError::InvalidPrice));
    assert_eq!(
        place(1, MAX_ORDER_VALUE + 1),
        Err(TradingError::InvalidPrice)
    );
    assert_eq!(place(1, i128::MAX), Err(TradingError::InvalidPrice));
    assert_eq!(place(0, 1), Err(TradingError::InvalidQuantity));
    assert_eq!(place(-1, 1), Err(TradingError::InvalidQuantity));
    assert_eq!(
        place(MAX_ORDER_VALUE + 1, 1),
        Err(TradingError::InvalidQuantity)
    );
    assert_eq!(place(i128::MAX, 1), Err(TradingError::InvalidQuantity));
    assert!(
        place(MAX_ORDER_VALUE, MAX_ORDER_VALUE).is_ok(),
        "boundary values are valid"
    );
}

#[test]
fn amm_rejects_overflowing_amounts_without_panicking() {
    let (env, id) = setup();
    let t = Address::generate(&env);
    let (a, b) = (Symbol::new(&env, "a"), Symbol::new(&env, "b"));
    let pool = at(&env, &id, || {
        trading::create_pool(&env, &t, a.clone(), b.clone())
    })
    .unwrap();

    let add = |x: i128, y: i128| at(&env, &id, || trading::add_liquidity(&env, &t, pool, x, y));
    assert_eq!(add(0, 10).err(), Some(AmmError::InvalidAmount));
    assert_eq!(add(10, -1).err(), Some(AmmError::InvalidAmount));
    // x * y overflows i128 in the first-deposit geometric mean.
    assert_eq!(
        add(i128::MAX, i128::MAX).err(),
        Some(AmmError::InvalidAmount)
    );
    assert_eq!(
        add(1i128 << 100, 1i128 << 100).err(),
        Some(AmmError::InvalidAmount)
    );
    assert!(add(1_000_000, 1_000_000).is_ok());
    // A follow-up deposit whose share maths would overflow.
    assert_eq!(
        add(i128::MAX, i128::MAX).err(),
        Some(AmmError::InvalidAmount)
    );

    let swap = |amt: i128| {
        at(&env, &id, || {
            trading::swap_exact_input(&env, &t, a.clone(), amt, 0, vec![&env, pool])
        })
    };
    assert_eq!(swap(0), Err(AmmError::InvalidAmount));
    assert_eq!(swap(-1), Err(AmmError::InvalidAmount));
    assert_eq!(
        swap(i128::MAX),
        Err(AmmError::InvalidAmount),
        "fee/output maths overflows"
    );
    assert_eq!(
        at(&env, &id, || trading::quote_swap(
            &env,
            pool,
            a.clone(),
            i128::MAX
        )),
        Err(AmmError::InvalidAmount)
    );

    let rm = |lp: i128| at(&env, &id, || trading::remove_liquidity(&env, &t, pool, lp));
    assert_eq!(rm(0), Err(AmmError::InvalidAmount));
    assert_eq!(rm(-5), Err(AmmError::InvalidAmount));
    assert_eq!(rm(i128::MAX), Err(AmmError::InsufficientLpTokens));
    // Unknown pools / empty or oversized routes.
    assert_eq!(
        rm_pool(&env, &id, &t, u64::MAX),
        Err(AmmError::PoolNotFound)
    );
    assert_eq!(
        at(&env, &id, || trading::swap_exact_input(
            &env,
            &t,
            a.clone(),
            1,
            0,
            Vec::new(&env)
        )),
        Err(AmmError::InvalidRoute)
    );
    assert_eq!(
        at(&env, &id, || trading::swap_exact_input(
            &env,
            &t,
            a.clone(),
            1,
            0,
            vec![&env, pool, pool, pool, pool]
        )),
        Err(AmmError::InvalidRoute)
    );
}

fn rm_pool(env: &Env, id: &Address, t: &Address, pool: u64) -> Result<(i128, i128), AmmError> {
    at(env, id, || trading::remove_liquidity(env, t, pool, 1))
}
