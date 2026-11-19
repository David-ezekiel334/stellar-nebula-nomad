//! Access control tests (Issue #608).
//!
//! Covers every authorization pattern used by the contract:
//!
//! 1. **Signature required** - state-changing entry points must call
//!    `require_auth()`. Verified with *no* mocked auths.
//! 2. **Admin-only** - RBAC (`access_control`), PvP, emergency controls,
//!    rate-limit configuration. Verified with a different, validly signed caller.
//! 3. **Ownership** - ships, limit orders, challenges, combat turns.
//! 4. **Guild membership** - alliances, guild quests, treasury proposals.
//! 5. **Tournament / PvP / matchmaking** - organizer-only actions and
//!    participant-only actions.
//!
//! Note on method: `mock_all_auths()` makes every `require_auth()` succeed, so
//! logic-level checks (admin == caller, owner == caller, member of alliance)
//! are exercised with it. The "signature required" tests clear the mocks with
//! `env.mock_auths(&[])` to prove `require_auth()` is actually reached.
//! See `docs/ACCESS_CONTROL_PATTERNS.md`.

// Test code: pedantic style lints (too_many_lines, semicolon_if_nothing_returned, ...)
// add noise without catching bugs here.
#![allow(clippy::pedantic)]

use soroban_sdk::{
    symbol_short, testutils::Address as _, vec, Address, Bytes, Env, String, Symbol, Vec,
};
use stellar_nebula_nomad::{
    access_control::{self, AccessControlError, ProposalOperation, BATCH_GRANT_LIMIT},
    alliance_manager::{self, AllianceError},
    emergency_controls::{self, EmergencyError},
    guild_quests::{self, GuildQuestError},
    rate_limiter::{self, Operation, RateLimitConfig, RateLimitError},
    resource_minter::ResourceType,
    tournament::{self, TournamentError},
    trading::{LimitOrder, OrderSide, TradingError},
    EloDecayConfig, NebulaNomadContract, NebulaNomadContractClient, PvPError, ShipError,
};

// ── Fixtures ──────────────────────────────────────────────────────────────

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(NebulaNomadContract, ());
    (env, id)
}

/// Run `f` in its own contract frame (one `require_auth` per frame per address).
fn at<T>(env: &Env, id: &Address, f: impl FnOnce() -> T) -> T {
    env.as_contract(id, f)
}

/// Drop all mocked authorizations: from now on only real signatures count.
fn require_real_signatures(env: &Env) {
    env.mock_auths(&[]);
}

fn rbac(env: &Env, id: &Address) -> Address {
    let admin = Address::generate(env);
    at(env, id, || {
        access_control::init_roles(env, admin.clone()).unwrap()
    });
    admin
}

// ══════════════════════════════════════════════════════════════════════════
// 1. Signature required (require_auth is reached)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn state_changing_entry_points_reject_unsigned_calls() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (alice, bob) = (Address::generate(&env), Address::generate(&env));
    let dust = Symbol::new(&env, "dust");

    // Prepare state while everything is mocked, then drop the mocks.
    let challenge = c.create_challenge(&alice, &bob, &0);
    let order_id = c.place_limit_order(
        &alice,
        &LimitOrder {
            id: 0,
            trader: alice.clone(),
            side: OrderSide::Buy,
            resource: dust.clone(),
            quantity: 5,
            limit_price: 5,
            placed_at: 0,
            is_stop_loss: false,
        },
    );
    let ship = c.mint_ship(&alice, &symbol_short!("explorer"), &Bytes::new(&env));
    require_real_signatures(&env);

    // PvP
    assert!(
        c.try_create_challenge(&alice, &bob, &0).is_err(),
        "create_challenge"
    );
    assert!(
        c.try_accept_challenge(&bob, &challenge).is_err(),
        "accept_challenge"
    );
    assert!(
        c.try_decline_challenge(&bob, &challenge).is_err(),
        "decline_challenge"
    );
    assert!(
        c.try_join_matchmaking(&alice, &0).is_err(),
        "join_matchmaking"
    );
    assert!(
        c.try_leave_matchmaking(&alice).is_err(),
        "leave_matchmaking"
    );
    assert!(
        c.try_execute_combat_move(&alice, &1, &symbol_short!("attack"), &10)
            .is_err(),
        "execute_combat_move"
    );
    assert!(c.try_set_pvp_admin(&alice).is_err(), "set_pvp_admin");
    // Trading
    assert!(
        c.try_cancel_limit_order(&alice, &order_id).is_err(),
        "cancel_limit_order"
    );
    assert!(
        c.try_create_pool(&alice, &Symbol::new(&env, "a"), &Symbol::new(&env, "b"))
            .is_err(),
        "create_pool"
    );
    // Ships
    assert!(
        c.try_transfer_ship(&ship.id, &alice, &bob).is_err(),
        "transfer_ship"
    );
    assert!(c
        .try_mint_ship(&alice, &symbol_short!("explorer"), &Bytes::new(&env))
        .is_err());
    // Guilds
    assert!(
        c.try_found_alliance(&alice, &String::from_str(&env, "Nomads"))
            .is_err(),
        "found_alliance"
    );
    assert!(c.try_join_alliance(&0, &alice).is_err(), "join_alliance");
    assert!(c.try_leave_alliance(&alice).is_err(), "leave_alliance");
    // Admin surface
    assert!(c.try_init_rbac(&alice).is_err(), "init_rbac");
    assert!(c.try_pause_contract(&alice).is_err(), "pause_contract");
    assert!(
        c.try_set_rate_limit_config(
            &alice,
            &Operation::Trading,
            &RateLimitConfig {
                max_calls: 1,
                window_seconds: 1
            }
        )
        .is_err(),
        "set_rate_limit_config"
    );
}

#[test]
fn signed_calls_succeed_when_arguments_are_valid() {
    // Positive control for the test above: with auths mocked the same calls work,
    // so the previous failures are attributable to the missing signature.
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (alice, bob) = (Address::generate(&env), Address::generate(&env));
    assert!(c.try_create_challenge(&alice, &bob, &0).is_ok());
    assert!(c.try_join_matchmaking(&alice, &0).is_ok());
    assert!(c
        .try_found_alliance(&alice, &String::from_str(&env, "Nomads"))
        .is_ok());
}

#[test]
fn module_level_functions_reject_unsigned_calls() {
    let (env, id) = setup();
    let (admin, user) = (rbac(&env, &id), Address::generate(&env));
    require_real_signatures(&env);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        at(&env, &id, || {
            access_control::grant_role(
                &env,
                admin.clone(),
                symbol_short!("nomad"),
                user.clone(),
                None,
            )
        })
    }));
    assert!(
        r.is_err(),
        "grant_role must panic without the admin's signature"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// 2. Admin-only functions and RBAC
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn rbac_can_only_be_initialised_once() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let attacker = Address::generate(&env);
    assert_eq!(
        at(&env, &id, || access_control::init_roles(
            &env,
            attacker.clone()
        )),
        Err(AccessControlError::InitializationFailed)
    );
    // The original admin is untouched.
    assert!(at(&env, &id, || access_control::has_role(
        &env,
        &symbol_short!("admin"),
        &admin
    )));
    assert!(!at(&env, &id, || access_control::has_role(
        &env,
        &symbol_short!("admin"),
        &attacker
    )));
}

#[test]
fn only_the_admin_can_grant_revoke_and_batch_grant_roles() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let (mallory, user) = (Address::generate(&env), Address::generate(&env));
    let role = symbol_short!("nomad");

    assert_eq!(
        at(&env, &id, || access_control::grant_role(
            &env,
            mallory.clone(),
            role.clone(),
            mallory.clone(),
            None
        )),
        Err(AccessControlError::AdminRequired),
        "self-grant"
    );
    assert_eq!(
        at(&env, &id, || access_control::grant_role(
            &env,
            mallory.clone(),
            role.clone(),
            user.clone(),
            None
        )),
        Err(AccessControlError::AdminRequired)
    );
    assert!(at(&env, &id, || access_control::grant_role(
        &env,
        admin.clone(),
        role.clone(),
        user.clone(),
        None
    ))
    .is_ok());
    assert!(at(&env, &id, || access_control::has_role(
        &env, &role, &user
    )));

    assert_eq!(
        at(&env, &id, || access_control::revoke_role(
            &env,
            mallory.clone(),
            role.clone(),
            user.clone()
        )),
        Err(AccessControlError::AdminRequired)
    );
    assert!(
        at(&env, &id, || access_control::has_role(&env, &role, &user)),
        "still granted"
    );

    let many: Vec<Address> = {
        let mut v = Vec::new(&env);
        for _ in 0..=BATCH_GRANT_LIMIT {
            v.push_back(Address::generate(&env));
        }
        v
    };
    assert!(at(&env, &id, || access_control::grant_role_batch(
        &env,
        mallory.clone(),
        role.clone(),
        many.clone(),
        None
    ))
    .is_err());
    assert_eq!(
        at(&env, &id, || access_control::grant_role_batch(
            &env,
            admin.clone(),
            role.clone(),
            many.clone(),
            None
        )),
        Err(AccessControlError::BatchLimitExceeded)
    );

    assert!(at(&env, &id, || access_control::revoke_role(
        &env,
        admin.clone(),
        role.clone(),
        user.clone()
    ))
    .is_ok());
    assert!(!at(&env, &id, || access_control::has_role(
        &env, &role, &user
    )));
}

#[test]
fn permissions_are_role_based_and_admin_managed() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let (holder, outsider, mallory) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let role = symbol_short!("indexer");
    let action = symbol_short!("index");

    at(&env, &id, || {
        access_control::grant_role(&env, admin.clone(), role.clone(), holder.clone(), None).unwrap()
    });
    assert_eq!(
        at(&env, &id, || access_control::grant_permission(
            &env,
            mallory.clone(),
            role.clone(),
            action.clone()
        )),
        Err(AccessControlError::AdminRequired)
    );
    at(&env, &id, || {
        access_control::grant_permission(&env, admin.clone(), role.clone(), action.clone()).unwrap()
    });

    assert!(at(&env, &id, || access_control::check_permission(
        &env, &holder, &action
    ))
    .is_ok());
    assert!(at(&env, &id, || access_control::check_permission(
        &env, &outsider, &action
    ))
    .is_err());
    // A permission is scoped to the action: the holder cannot do something else.
    assert!(at(&env, &id, || access_control::check_permission(
        &env,
        &holder,
        &symbol_short!("wipe")
    ))
    .is_err());

    assert_eq!(
        at(&env, &id, || access_control::revoke_permission(
            &env,
            mallory.clone(),
            role.clone(),
            action.clone()
        )),
        Err(AccessControlError::AdminRequired)
    );
    at(&env, &id, || {
        access_control::revoke_permission(&env, admin.clone(), role.clone(), action.clone())
            .unwrap()
    });
    assert!(at(&env, &id, || access_control::check_permission(
        &env, &holder, &action
    ))
    .is_err());
}

#[test]
fn admin_transfer_is_admin_only_and_removes_the_old_admin() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let (mallory, successor, user) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    assert_eq!(
        at(&env, &id, || access_control::transfer_admin(
            &env,
            mallory.clone(),
            user.clone()
        )),
        Err(AccessControlError::AdminRequired)
    );
    at(&env, &id, || {
        access_control::transfer_admin(&env, admin.clone(), successor.clone()).unwrap()
    });
    // Old admin is locked out; the new one can act.
    assert_eq!(
        at(&env, &id, || access_control::grant_role(
            &env,
            admin.clone(),
            symbol_short!("nomad"),
            user.clone(),
            None
        )),
        Err(AccessControlError::AdminRequired)
    );
    assert!(at(&env, &id, || access_control::grant_role(
        &env,
        successor.clone(),
        symbol_short!("nomad"),
        user.clone(),
        None
    ))
    .is_ok());
}

#[test]
fn emergency_mode_and_role_delegation_are_guarded() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let (mallory, holder, deputy) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    assert_eq!(
        at(&env, &id, || access_control::set_emergency_mode(
            &env,
            mallory.clone(),
            true
        )),
        Err(AccessControlError::AdminRequired)
    );
    assert!(!at(&env, &id, || access_control::is_emergency_mode(&env)));
    at(&env, &id, || {
        access_control::set_emergency_mode(&env, admin.clone(), true).unwrap()
    });
    assert!(at(&env, &id, || access_control::is_emergency_mode(&env)));

    // You cannot delegate a role you do not hold.
    let role = symbol_short!("nomad");
    let expiry = env.ledger().sequence() + 100;
    assert_eq!(
        at(&env, &id, || access_control::delegate_role(
            &env,
            mallory.clone(),
            deputy.clone(),
            role.clone(),
            expiry
        )),
        Err(AccessControlError::UnauthorizedRole)
    );
    at(&env, &id, || {
        access_control::grant_role(&env, admin.clone(), role.clone(), holder.clone(), None).unwrap()
    });
    assert!(at(&env, &id, || access_control::delegate_role(
        &env,
        holder.clone(),
        deputy.clone(),
        role.clone(),
        expiry
    ))
    .is_ok());
    assert!(at(&env, &id, || access_control::has_role(
        &env, &role, &deputy
    )));
}

#[test]
fn multisig_requires_signers_approvals_and_the_admin_to_configure() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let (s1, s2, s3, outsider, mallory) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let signers = vec![&env, s1.clone(), s2.clone(), s3.clone()];

    assert_eq!(
        at(&env, &id, || access_control::init_multisig(
            &env,
            mallory.clone(),
            signers.clone(),
            2,
            0
        )),
        Err(AccessControlError::AdminRequired)
    );
    assert_eq!(
        at(&env, &id, || access_control::init_multisig(
            &env,
            admin.clone(),
            signers.clone(),
            4,
            0
        )),
        Err(AccessControlError::InvalidSignerConfig),
        "threshold above signer count"
    );
    at(&env, &id, || {
        access_control::init_multisig(&env, admin.clone(), signers.clone(), 2, 0).unwrap()
    });

    let op = ProposalOperation::GrantRole(symbol_short!("nomad"), outsider.clone(), None);
    assert!(
        at(&env, &id, || access_control::create_proposal(
            &env,
            outsider.clone(),
            op.clone()
        ))
        .is_err(),
        "non-signer cannot propose"
    );
    let pid = at(&env, &id, || {
        access_control::create_proposal(&env, s1.clone(), op.clone())
    })
    .unwrap();
    assert!(
        at(&env, &id, || access_control::approve_proposal(
            &env,
            outsider.clone(),
            pid
        ))
        .is_err(),
        "non-signer cannot approve"
    );

    // The proposer auto-approves (1 of 2): executing must fail and grant nothing.
    assert!(at(&env, &id, || access_control::execute_proposal(
        &env,
        s1.clone(),
        pid
    ))
    .is_err());
    assert!(!at(&env, &id, || access_control::has_role(
        &env,
        &symbol_short!("nomad"),
        &outsider
    )));

    assert_eq!(
        at(&env, &id, || access_control::approve_proposal(
            &env,
            s1.clone(),
            pid
        )),
        Err(AccessControlError::AlreadyApproved),
        "a signer cannot approve twice"
    );
    at(&env, &id, || {
        access_control::approve_proposal(&env, s2.clone(), pid).unwrap()
    });
    assert!(at(&env, &id, || access_control::execute_proposal(
        &env,
        s3.clone(),
        pid
    ))
    .is_ok());
    assert!(at(&env, &id, || access_control::has_role(
        &env,
        &symbol_short!("nomad"),
        &outsider
    )));
    assert_eq!(
        at(&env, &id, || access_control::execute_proposal(
            &env,
            s3.clone(),
            pid
        )),
        Err(AccessControlError::ProposalAlreadyExecuted),
        "no replay"
    );
}

#[test]
fn rate_limit_configuration_is_admin_only() {
    let (env, id) = setup();
    let admin = rbac(&env, &id);
    let mallory = Address::generate(&env);
    let cfg = RateLimitConfig {
        max_calls: 1_000_000,
        window_seconds: 1,
    };
    assert_eq!(
        at(&env, &id, || rate_limiter::set_rate_limit_config(
            &env,
            &mallory,
            Operation::Trading,
            cfg.clone()
        )),
        Err(RateLimitError::Unauthorized)
    );
    assert!(at(&env, &id, || rate_limiter::set_rate_limit_config(
        &env,
        &admin,
        Operation::Trading,
        cfg.clone()
    ))
    .is_ok());
}

#[test]
fn emergency_controls_are_admin_only() {
    let (env, id) = setup();
    let (admin, mallory) = (Address::generate(&env), Address::generate(&env));
    at(&env, &id, || {
        emergency_controls::initialize_admins(&env, vec![&env, admin.clone()]).unwrap()
    });
    // Admin set cannot be re-initialised by an attacker.
    assert_eq!(
        at(&env, &id, || emergency_controls::initialize_admins(
            &env,
            vec![&env, mallory.clone()]
        )),
        Err(EmergencyError::AlreadyInitialized)
    );
    let c = NebulaNomadContractClient::new(&env, &id);
    assert_eq!(
        c.try_pause_contract(&mallory),
        Err(Ok(EmergencyError::NotAdmin))
    );
    assert!(c.try_pause_contract(&admin).is_ok());
    assert_eq!(
        c.try_emergency_withdraw(&mallory, &Symbol::new(&env, "dust")),
        Err(Ok(EmergencyError::NotAdmin))
    );
}

#[test]
fn pvp_admin_is_set_once_and_gates_configuration() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (admin, mallory) = (Address::generate(&env), Address::generate(&env));
    let cfg = EloDecayConfig {
        inactivity_secs: 1,
        decay_points: 1_000,
        floor: 0,
    };

    // Nobody is admin yet.
    assert_eq!(
        c.try_set_elo_decay_config(&mallory, &cfg),
        Err(Ok(PvPError::Unauthorized))
    );
    c.set_pvp_admin(&admin);
    // A second call must not re-appoint the admin (Issue #237 regression).
    c.set_pvp_admin(&mallory);
    assert_eq!(
        c.try_set_elo_decay_config(&mallory, &cfg),
        Err(Ok(PvPError::Unauthorized))
    );
    assert!(c.try_set_elo_decay_config(&admin, &cfg).is_ok());
}

// ══════════════════════════════════════════════════════════════════════════
// 3. User ownership verification
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn only_the_owner_can_transfer_a_ship() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (owner, thief, friend) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let ship = c.mint_ship(&owner, &symbol_short!("explorer"), &Bytes::new(&env));
    assert_eq!(
        c.try_transfer_ship(&ship.id, &thief, &friend),
        Err(Ok(ShipError::NotOwner)),
        "thief cannot move someone else's ship"
    );
    assert_eq!(c.get_ship(&ship.id).owner, owner);
    assert_eq!(
        c.try_transfer_ship(&ship.id, &owner, &owner),
        Err(Ok(ShipError::SameOwner))
    );
    assert!(c.try_transfer_ship(&ship.id, &owner, &friend).is_ok());
    assert_eq!(c.get_ship(&ship.id).owner, friend);
    // The previous owner no longer controls it.
    assert_eq!(
        c.try_transfer_ship(&ship.id, &owner, &thief),
        Err(Ok(ShipError::NotOwner))
    );
}

#[test]
fn only_the_trader_can_cancel_their_limit_order() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (alice, mallory) = (Address::generate(&env), Address::generate(&env));
    let order = LimitOrder {
        id: 0,
        trader: mallory.clone(), // forged: the contract must overwrite with the signer
        side: OrderSide::Sell,
        resource: Symbol::new(&env, "dust"),
        quantity: 10,
        limit_price: 3,
        placed_at: 0,
        is_stop_loss: false,
    };
    let oid = c.place_limit_order(&alice, &order);
    assert_eq!(
        c.get_limit_order(&oid).unwrap().trader,
        alice,
        "trader field cannot be forged"
    );
    assert_eq!(
        c.try_cancel_limit_order(&mallory, &oid),
        Err(Ok(TradingError::NotOrderOwner))
    );
    assert!(
        c.get_limit_order(&oid).is_some(),
        "order survives the attack"
    );
    assert!(c.try_cancel_limit_order(&alice, &oid).is_ok());
}

#[test]
fn only_the_challenged_player_can_accept_or_decline() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b, mallory) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let cid = c.create_challenge(&a, &b, &0);
    assert_eq!(
        c.try_accept_challenge(&mallory, &cid),
        Err(Ok(PvPError::Unauthorized))
    );
    assert_eq!(
        c.try_accept_challenge(&a, &cid),
        Err(Ok(PvPError::Unauthorized)),
        "challenger cannot accept own challenge"
    );
    assert_eq!(
        c.try_decline_challenge(&mallory, &cid),
        Err(Ok(PvPError::Unauthorized))
    );
    assert!(c.try_accept_challenge(&b, &cid).is_ok());
}

#[test]
fn only_combat_participants_on_their_turn_can_move() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b, mallory) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let cid = c.create_challenge(&a, &b, &0);
    let combat = c.accept_challenge(&b, &cid);
    let attack = symbol_short!("attack");

    assert_eq!(
        c.try_execute_combat_move(&mallory, &combat, &attack, &10),
        Err(Ok(PvPError::InvalidMove)),
        "outsiders cannot act in someone else's combat"
    );
    let state = c.get_combat(&combat);
    let (on_turn, off_turn) = if state.turn == a { (&a, &b) } else { (&b, &a) };
    assert_eq!(
        c.try_execute_combat_move(off_turn, &combat, &attack, &10),
        Err(Ok(PvPError::InvalidMove)),
        "cannot move out of turn"
    );
    assert!(c
        .try_execute_combat_move(on_turn, &combat, &attack, &10)
        .is_ok());
}

#[test]
fn ship_abilities_cannot_be_used_by_non_participants() {
    // Regression for a finding of this audit: `use_ability` used to accept any
    // address and treat strangers as player 2.
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b, mallory) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let cid = c.create_challenge(&a, &b, &0);
    let combat = c.accept_challenge(&b, &cid);
    let ship_type = symbol_short!("fighter");
    let abilities = c.get_ship_abilities(&ship_type);
    let ability = abilities.get(0).unwrap().id;
    let before = c.get_combat(&combat);

    assert_eq!(
        c.try_use_ability(&combat, &mallory, &ship_type, &ability),
        Err(Ok(PvPError::Unauthorized))
    );
    let after = c.get_combat(&combat);
    assert_eq!(before.player1_hp, after.player1_hp);
    assert_eq!(before.player2_hp, after.player2_hp);
    assert_eq!(before.player1_energy, after.player1_energy);
    assert_eq!(before.player2_energy, after.player2_energy);

    // Participants are fine, and require their own signature.
    c.init_combat_abilities(&combat, &a, &ship_type);
    assert!(c.try_use_ability(&combat, &a, &ship_type, &ability).is_ok());
    require_real_signatures(&env);
    assert!(
        c.try_use_ability(&combat, &a, &ship_type, &ability)
            .is_err(),
        "unsigned use_ability"
    );
    assert!(
        c.try_init_combat_abilities(&combat, &a, &ship_type)
            .is_err(),
        "unsigned init"
    );
    assert!(
        c.try_tick_ability_cooldowns(&combat, &a).is_err(),
        "unsigned tick"
    );
}

#[test]
fn per_user_state_endpoints_require_the_users_signature() {
    // Regression: these used to be callable by anyone for any address, letting
    // a stranger burn a victim's daily anti-whale cap or poison their forecast.
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let victim = Address::generate(&env);
    require_real_signatures(&env);
    assert!(
        c.try_process_anti_whale_action(&victim, &1_000).is_err(),
        "anti-whale cap grief"
    );
    assert!(
        c.try_generate_yield_forecast(&victim, &7).is_err(),
        "forecast read-for-victim"
    );
    assert!(
        c.try_create_player_reputation(&victim).is_err(),
        "reputation"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// 4. Guild membership
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn guild_membership_rules_are_enforced() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (founder, member, outsider) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let alliance = c.found_alliance(&founder, &String::from_str(&env, "Nomads"));

    assert_eq!(
        c.try_found_alliance(&founder, &String::from_str(&env, "Again")),
        Err(Ok(AllianceError::AlreadyInAlliance))
    );
    assert_eq!(
        c.try_leave_alliance(&outsider),
        Err(Ok(AllianceError::NotMember)),
        "non-members cannot leave"
    );
    assert!(c.try_join_alliance(&alliance, &member).is_ok());
    assert_eq!(
        c.try_join_alliance(&alliance, &member).unwrap_err(),
        Ok(AllianceError::AlreadyInAlliance)
    );
    assert_eq!(c.get_player_alliance(&outsider), None);
    assert_eq!(c.get_player_alliance(&member), Some(alliance));

    // Members-only actions reject outsiders.
    assert_eq!(
        at(&env, &id, || alliance_manager::contribute_to_treasury(
            &env,
            outsider.clone(),
            10
        )),
        Err(AllianceError::NotMember)
    );
    assert_eq!(
        at(&env, &id, || alliance_manager::spawn_guild_boss(
            &env,
            outsider.clone()
        )),
        Err(AllianceError::NotMember)
    );
    assert!(c.try_leave_alliance(&member).is_ok());
    assert_eq!(c.get_player_alliance(&member), None);
}

#[test]
fn only_the_alliance_founder_can_create_guild_quests() {
    // Regression for a finding of this audit: any signer could create a quest
    // for any alliance and mint `reward_essence` into its treasury.
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (founder, member, outsider) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let alliance = c.found_alliance(&founder, &String::from_str(&env, "Nomads"));
    c.join_alliance(&alliance, &member);
    let kind = symbol_short!("scan");
    let create = |who: &Address, alliance_id: u64, target: u64| {
        at(&env, &id, || {
            guild_quests::create_guild_quest(
                &env,
                who.clone(),
                alliance_id,
                kind.clone(),
                target,
                1_000_000,
                10,
                3_600,
            )
        })
    };
    assert_eq!(
        create(&outsider, alliance, 5),
        Err(GuildQuestError::Unauthorized)
    );
    assert_eq!(
        create(&member, alliance, 5),
        Err(GuildQuestError::Unauthorized),
        "members are not founders"
    );
    assert_eq!(
        create(&founder, 999, 5),
        Err(GuildQuestError::NotAllianceMember),
        "unknown alliance"
    );
    assert_eq!(
        create(&founder, alliance, 0),
        Err(GuildQuestError::InvalidParameters)
    );
    assert_eq!(create(&founder, alliance, 5), Ok(1));

    // Progress only counts for alliance members.
    at(&env, &id, || {
        assert_eq!(
            guild_quests::contribute_quest_progress(&env, outsider.clone(), kind.clone(), 5),
            Err(GuildQuestError::NotAllianceMember)
        );
        assert_eq!(
            guild_quests::contribute_quest_progress(&env, member.clone(), kind.clone(), 5),
            Ok(())
        );
    });
}

// ══════════════════════════════════════════════════════════════════════════
// 5. Tournament and matchmaking participation
// ══════════════════════════════════════════════════════════════════════════

fn tournament_of_four(env: &Env, id: &Address, organizer: &Address) -> u64 {
    at(env, id, || {
        tournament::create_tournament(
            env,
            organizer,
            ResourceType::StellarDust,
            0,
            4,
            3_600,
            vec![env, 10_000u32],
        )
    })
    .unwrap()
}

#[test]
fn only_the_organizer_can_start_a_tournament() {
    let (env, id) = setup();
    let (org, mallory) = (Address::generate(&env), Address::generate(&env));
    let t = tournament_of_four(&env, &id, &org);
    let players: std::vec::Vec<Address> = (0..4).map(|_| Address::generate(&env)).collect();
    for p in &players {
        at(&env, &id, || {
            tournament::register_for_tournament(&env, p, t)
        })
        .unwrap();
    }
    assert_eq!(
        at(&env, &id, || tournament::start_tournament(
            &env, &mallory, t
        )),
        Err(TournamentError::Unauthorized)
    );
    // A registrant is still not the organizer.
    assert_eq!(
        at(&env, &id, || tournament::start_tournament(
            &env,
            &players[0],
            t
        )),
        Err(TournamentError::Unauthorized)
    );
    assert_eq!(
        at(&env, &id, || tournament::get_tournament(&env, t))
            .unwrap()
            .status,
        symbol_short!("reg"),
        "still in registration after the failed attempts"
    );
    assert!(at(&env, &id, || tournament::start_tournament(&env, &org, t)).is_ok());
}

#[test]
fn tournament_participation_rules() {
    let (env, id) = setup();
    let (org, p1, p2) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let t = tournament_of_four(&env, &id, &org);
    // Cannot start with fewer than two registrants, even as organizer.
    assert_eq!(
        at(&env, &id, || tournament::start_tournament(&env, &org, t)),
        Err(TournamentError::NotEnoughRegistrants)
    );
    at(&env, &id, || {
        tournament::register_for_tournament(&env, &p1, t)
    })
    .unwrap();
    assert_eq!(
        at(&env, &id, || tournament::register_for_tournament(
            &env, &p1, t
        )),
        Err(TournamentError::AlreadyRegistered)
    );
    at(&env, &id, || {
        tournament::register_for_tournament(&env, &p2, t)
    })
    .unwrap();
    at(&env, &id, || tournament::start_tournament(&env, &org, t)).unwrap();
    // Registration is closed once the bracket exists; starting twice is refused.
    let late = Address::generate(&env);
    assert_eq!(
        at(&env, &id, || tournament::register_for_tournament(
            &env, &late, t
        )),
        Err(TournamentError::NotInRegistration)
    );
    assert_eq!(
        at(&env, &id, || tournament::start_tournament(&env, &org, t)),
        Err(TournamentError::NotInRegistration)
    );
    // Results cannot be reported for a bracket slot that has no real combat.
    assert!(at(&env, &id, || tournament::report_match_result(
        &env, t, 99, 0
    ))
    .is_err());
}

#[test]
fn matchmaking_queue_membership_is_per_player() {
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, b, outsider) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    c.join_matchmaking(&a, &0);
    c.join_matchmaking(&b, &0);
    assert_eq!(c.get_matchmaking_queue_size(), 2);
    // Leaving only removes the caller; an outsider is told they are not queued.
    assert_eq!(
        c.try_leave_matchmaking(&outsider),
        Err(Ok(PvPError::NotInQueue))
    );
    assert_eq!(c.get_matchmaking_queue_size(), 2);
    assert!(c.try_leave_matchmaking(&a).is_ok());
    assert_eq!(c.get_matchmaking_queue_size(), 1);
    assert_eq!(c.try_leave_matchmaking(&a), Err(Ok(PvPError::NotInQueue)));
}

#[test]
fn mmr_queue_only_dequeues_the_calling_player() {
    use stellar_nebula_nomad::GameMode;
    let (env, id) = setup();
    let c = NebulaNomadContractClient::new(&env, &id);
    let (a, outsider) = (Address::generate(&env), Address::generate(&env));
    c.mm_enqueue(&a, &GameMode::Casual);
    assert!(
        c.try_mm_dequeue(&outsider, &GameMode::Casual).is_err(),
        "outsider cannot dequeue"
    );
    assert!(
        c.mm_is_queued(&a, &GameMode::Casual),
        "player is still queued"
    );
    assert!(c.try_mm_dequeue(&a, &GameMode::Casual).is_ok());
    assert!(!c.mm_is_queued(&a, &GameMode::Casual));
}

// ══════════════════════════════════════════════════════════════════════════
// 6. Static audit of every contract entry point
// ══════════════════════════════════════════════════════════════════════════

/// Static audit of `src/lib.rs`: every contract function that takes an actor
/// address (`player`, `caller`, `admin`, ...) and is not a read-only getter must
/// authenticate it, either directly or in the module function it forwards to
/// (or that function's `_unguarded` body). This is what caught `use_ability`,
/// `process_anti_whale_action`, `update_forecast_model` and friends being
/// callable on behalf of any address.
#[test]
fn every_state_changing_entry_point_authenticates_its_actor() {
    use std::{collections::BTreeMap, fs, path::Path};

    const ACTORS: [&str; 31] = [
        "admin",
        "caller",
        "player",
        "owner",
        "operator",
        "organizer",
        "challenger",
        "trader",
        "provider",
        "creator",
        "sender",
        "user",
        "proposer",
        "voter",
        "guardian",
        "governor",
        "authority",
        "signer",
        "requester",
        "seller",
        "buyer",
        "founder",
        "member",
        "spectator",
        "referrer",
        "claimer",
        "depositor",
        "staker",
        "farmer",
        "holder",
        "leader",
    ];
    const READ_ONLY: [&str; 15] = [
        "get_",
        "is_",
        "has_",
        "query_",
        "view_",
        "calculate_",
        "quote_",
        "list_",
        "compute_",
        "preview_",
        "check_",
        "estimate_",
        "simulate_",
        "verify_",
        "can_",
    ];
    const AUTH: [&str; 9] = [
        "require_auth",
        "ensure_auth",
        "require_admin",
        "require_role",
        "check_permission",
        "has_role",
        "is_admin",
        "assert_admin",
        "ensure_admin",
    ];
    // Deliberately permissionless or read-only; each has a reason.
    let allowed: BTreeMap<&str, &str> = BTreeMap::from([
        (
            "apply_elo_decay",
            "keeper function: anyone may decay an inactive player's ELO",
        ),
        ("mm_is_queued", "read-only"),
        ("mm_get_status", "read-only"),
        ("batch_get_mobile_info", "read-only"),
        (
            "transfer_ownership",
            "authenticates the *current* owner (from storage) via transfer_ship",
        ),
        (
            "auto_snapshot",
            "authenticates via take_snapshot (owner check)",
        ),
        (
            "schedule_weekly_festival",
            "authenticates via schedule_recurring_event -> require_admin",
        ),
    ]);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    fn collect(dir: &Path, out: &mut std::vec::Vec<(std::string::String, std::string::String)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push((
                    p.file_stem().unwrap().to_string_lossy().into_owned(),
                    fs::read_to_string(&p).unwrap(),
                ));
            }
        }
    }
    let mut files: std::vec::Vec<(std::string::String, std::string::String)> = std::vec::Vec::new();
    collect(&root, &mut files);

    fn body_of<'a>(src: &'a str, name: &str) -> Option<&'a str> {
        let pat = format!("fn {name}");
        let mut from = 0;
        while let Some(pos) = src[from..].find(&pat) {
            let at = from + pos;
            from = at + pat.len();
            let next = src[from..].chars().next()?;
            if next.is_alphanumeric() || next == '_' {
                continue; // e.g. `fn name_unguarded`
            }
            let open = src[at..].find('{')? + at;
            if src[at..open].contains(';') {
                continue;
            }
            let (mut depth, mut end) = (0usize, open);
            for (i, c) in src[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Some(&src[open..=end]);
        }
        None
    }
    let authenticates = |body: &str| AUTH.iter().any(|m| body.contains(m));

    let lib = &files.iter().find(|(n, _)| n == "lib").unwrap().1;
    let impl_start = lib.find("#[contractimpl]").unwrap();
    let lib = &lib[impl_start..];

    let mut offenders: std::vec::Vec<std::string::String> = std::vec::Vec::new();
    let mut audited = 0;
    let mut cursor = 0;
    while let Some(pos) = lib[cursor..].find("\n    pub fn ") {
        let start = cursor + pos + "\n    pub fn ".len();
        cursor = start;
        let name: std::string::String = lib[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let params_end = lib[start..].find(')').unwrap() + start;
        let params = &lib[start + name.len()..params_end];
        let takes_actor = ACTORS.iter().any(|a| {
            params.contains(&format!("{a}: Address")) || params.contains(&format!("{a}: &Address"))
        });
        if !takes_actor || READ_ONLY.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        audited += 1;
        if allowed.contains_key(name.as_str()) {
            continue;
        }
        let Some(body) = body_of(lib, &name) else {
            continue;
        };
        if authenticates(body) {
            continue;
        }
        // Resolve `module::function(` callees one level deep.
        let mut ok = false;
        let bytes: std::vec::Vec<&str> = body
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
            .collect();
        for tok in bytes {
            let Some((m, f)) = tok.rsplit_once("::") else {
                continue;
            };
            let m = m.rsplit("::").next().unwrap_or(m);
            for (stem, src) in &files {
                if stem != m {
                    continue;
                }
                for cand in [f.to_string(), format!("{f}_unguarded")] {
                    if body_of(src, &cand).is_some_and(authenticates) {
                        ok = true;
                    }
                }
            }
        }
        if !ok {
            offenders.push(name);
        }
    }
    assert!(
        audited > 100,
        "audit scanned only {audited} entry points; parser broken?"
    );
    assert!(
        offenders.is_empty(),
        "entry points taking an actor address without authenticating it: {offenders:?}\n\
         (call `actor.require_auth()` or add a documented exception to `allowed`)"
    );
}
