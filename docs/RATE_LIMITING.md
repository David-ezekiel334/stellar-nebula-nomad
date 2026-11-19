# Rate limiting strategy

Issue #610. `src/rate_limiter.rs` enforces a per-address, per-operation
fixed window (`max_calls` per `window_seconds`). Exceeding it returns
`RateLimitError::RateLimitExceeded` (surfaced as each module's own
`RateLimitExceeded` error) and emits a `("RateLimit", "hit")` event so indexers
can spot abuse.

## Where it is enforced

| Operation | Entry points | Default |
| --- | --- | --- |
| `NebulaGeneration` | `nebula_gen::generate_*`, `nebula_explorer` scans | 5 / 60 s |
| `ResourceMinting` | `resource_minter::mint_resource`, batch mint | 10 / 60 s |
| `ShipUpgrade` | `ship_upgrade` upgrade paths | 3 / 5 min |
| `ShipRepair` | `ship_repair::repair_ship` | 3 / 5 min |
| `BatchOperation` | `batch_processor` | 5 / 1 h |
| `PrivacyCommit` | `privacy_stats` commits | 20 / 1 h |
| `RouteCalculation` | `navigation_planner` | 15 / 1 h |
| `CombatInitiation` | `pvp_combat::create_challenge`, `accept_challenge`, `join_matchmaking` | 10 / 5 min |
| `TournamentRegistration` | `tournament::register_for_tournament` | 5 / 1 h |
| `Crafting` | `crafting::craft`, `craft_with_overcharge` | 20 / 60 s |
| `Trading` | `trading::place_limit_order`, `create_pool`, `add_liquidity`, `swap_exact_input` | 30 / 60 s |

`NebulaScan` is a declared-but-reserved budget: scans currently share
`NebulaGeneration`. `start_combat` has no caller of its own; it is only reachable
through `accept_challenge`, so it is covered by `CombatInitiation`.

## Rules for adding a limit

1. Call `check_rate_limit(env, &caller, Operation::X)?` **after** the caller's
   `require_auth()` (so nobody can burn another address's budget) and **before**
   any storage-heavy or CPU-heavy work.
2. Add a `RateLimitExceeded` variant plus `impl From<RateLimitError>` to the
   module's error enum so `?` works.
3. A failed contract call reverts, so a rejected call never consumes budget
   that a later legitimate call needs.
4. Add the new `Operation` to `Operation::ALL` and give it a
   `default_*` config. `tests/security/rate_limiting_tests.rs` fails if an
   operation has no sane default or is declared but never enforced.

## Choosing limits

Defaults are sized so a normal player never notices them (a duel lasts minutes,
tournament signups are rare) while sustained abuse is capped at well under one
call per second per address. The test suite asserts both properties for every
operation.

## Configuration

Admins (RBAC `admin` role) can override any limit at runtime:

```rust
rate_limiter::set_rate_limit_config(&env, &admin, Operation::Trading,
    RateLimitConfig { max_calls: 60, window_seconds: 60 })?;
rate_limiter::get_rate_limit_config(&env, Operation::Trading); // effective limit
```

Zero values are rejected (`InvalidConfig`) and non-admins get `Unauthorized`.

## Limits of this approach

Limits are per address, so an attacker with many funded accounts is bounded by
network fees, not by this module; sybil resistance lives in `bot_detection` and
`anti_whale`. Windows are fixed (not sliding), so an address can burst up to
`2 * max_calls` across a window boundary.

## Module integration (Issue #585)

After the upstream merge the limiter lives in `src/rate_limiter.rs` and is
declared publicly in `src/lib.rs`:

```rust
/// Per-address rate limiting for sensitive contract operations.
pub mod rate_limiter;
```

Every enforcing module imports it through the crate path, e.g.
`use crate::rate_limiter::{check_rate_limit, Operation, RateLimitError};`
(`resource_minter`, `nebula_gen`, `pvp_combat`, `tournament`, `crafting`,
`trading`, ...). External integration tests reach it as
`stellar_nebula_nomad::rate_limiter`, and the contract exposes the admin
override as `NebulaNomadContract::set_rate_limit_config`.

### Tests

| Suite | What it covers |
| --- | --- |
| `src/rate_limiter.rs` (unit, 12 tests) | within-limit / over-limit calls, per-address and per-operation isolation, admin override, non-admin and zero-value rejection, window reset after expiry, default/override config reads, `("RateLimit", "hit")` event |
| `tests/security/rate_limiting_tests.rs` | every `Operation` is enforced at its real entry point and recovers after its window |

The unit tests call module functions directly, so each call runs inside its own
contract frame via `test_helpers::in_contract` (Soroban SDK 22+ only allows
storage access from a contract frame).
