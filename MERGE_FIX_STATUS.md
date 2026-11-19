# Merge Fix Status - Upstream Integration

## Summary

The merge from upstream `Space-Nebula/stellar-nebula-nomad` (main) is **complete**.
All 73 compilation errors from the original merge are resolved. The 30 fixed in
the first pass are listed under [Phase 1](#phase-1-30-errors-fixed-during-the-merge),
and the 43 that were left are listed under
[Phase 2](#phase-2-the-43-remaining-errors-issue-584).

| Check | Status |
| --- | --- |
| `cargo build` | ✅ clean, no warnings |
| `cargo clippy --all-targets --all-features -- -D warnings` | ✅ clean |
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo test --all` | ✅ all suites green |
| `python3 scripts/check_storage_keys.py --strict` | ✅ no collisions |

`build_errors.txt` (the 1,141-line compiler log captured during the merge) has
been removed. It described a tree that no longer exists, and none of its errors
reproduce.

---

## Phase 1: 30 errors fixed during the merge

### 1. Module declarations
- Added `pub mod rate_limiter;` and `pub mod nebula_gen;` to `src/lib.rs`.

### 2. Symbol length violations
Fixed every `symbol_short!()` longer than 9 characters:
- `cache_ttl_manager.rs`: `namespace_invalidated` → `ns_invald`, `invalidated` → `invalid`, `configured` → `config`
- `migration_framework.rs`: `incompatible` → `incomp`, `batch_completed` → `batch_ok`, `rolled_back` → `rollback`
- `metrics_exporter.rs`: `tx_success` → `tx_ok`, `tx_failure` → `tx_fail`, `error_spike` → `err_spike`

### 3. Type exports
- `AccessControlError` re-exported from `access_control`. Documented and tested in #586.
- `resource_minter` exports updated to match the new implementation.

### 4. Deprecated functions
- Temporary stubs for `harvest_resources`, `auto_list_on_dex` and `harvest_and_list`.
  These have since been replaced by real implementations (see item 2 below).

---

## Phase 2: the 43 remaining errors (Issue #584)

Each group below shows how the errors were resolved and how to check that
they stay fixed.

### Type compatibility

| # | Problem | Resolution | Verification |
| --- | --- | --- | --- |
| 1 | **`NebulaLayout` defined twice** (`nebula_explorer` vs `nebula_gen`) | One definition remains, `nebula_explorer::NebulaLayout`. `nebula_gen` handles generation and TTL and works with that type. `resource_minter` imports it from `nebula_explorer`. | `grep -rn "struct NebulaLayout" src` → 1 hit |
| 2 | **Harvest types missing** (`HarvestError`, `HarvestResult`, `DexOffer`) | Fully migrated into `resource_minter.rs` and re-exported by `dex_integration`. `harvest_resources`, `harvest_and_list` and `cancel_listing` are real contract entry points again, not deprecation stubs. | `tests/economy/test_dex_integration.rs` (15 tests) |
| 3 | **`Rarity` import removed** | `nebula_explorer::Rarity` is defined next to `calculate_rarity_tier`. The unrelated `nft_marketplace::Rarity` stays module-scoped and is never glob-imported alongside it. | `cargo build` |

### Soroban SDK incompatibilities

| # | Problem | Resolution | Verification |
| --- | --- | --- | --- |
| 4 | **`Vec<u8>` in `cache_ttl_manager.rs`** | Cache values are `soroban_sdk::Bytes`. | `grep -n "Vec<u8>" src/cache_ttl_manager.rs` → none |
| 5 | **`Symbol::to_string()`** (`pvp_combat.rs`, `state_snapshot.rs`) | Symbols are stored and compared directly. Nothing round-trips through `String`. | `grep -n "to_string()" src/pvp_combat.rs src/state_snapshot.rs` → none |
| 6 | **`&&Address` → `Val` conversions** | Call sites pass `&Address` / owned `Address`. | `cargo build` |
| 6a | **Unit tests pinned to ledger protocol 22** (SDK 28 rejects it with `Error(Context, InternalError)`) | Test `LedgerInfo` now uses protocol 28 in `access_control`, `nebula_gen` and `resource_minter`. | 59 previously-failing tests pass |
| 6b | **Module unit tests touching storage outside a contract frame** (SDK 22+ panics) | New `test_helpers::contract_env()` / `in_contract()` run each call in its own frame of a registered contract. Modules with their own stub contract use a local `at()` helper. | see "Tests re-enabled" |

### Move / borrow checker

| # | Problem | Resolution | Verification |
| --- | --- | --- | --- |
| 7 | **`storage_uri` moved** (`state_snapshot.rs`) | Borrowed for the checksum, cloned into the record and the event. | `cargo build` |
| 8 | **`namespace` moved** (`cache_ttl_manager.rs`) | The namespace index is keyed by a clone. The original value is kept for the event. | `cargo build` |
| 9 | **`caller` dereferenced** (`content_tools.rs`) | Takes `&Address` throughout. | `content_tools` unit tests |

### Regression found while re-enabling tests

| Problem | Resolution |
| --- | --- |
| **`ResourceMinterContract::mint_resource` lost its reentrancy guard** in the merge. The doc comment still promised Issue #472 protection, the guarded body `mint_resource_unguarded` was still there but never called, and the public entry point ran unguarded. | `mint_resource` now does `require_auth()` and then `with_guard(env, \|\| mint_resource_unguarded(..))`, exactly as before the merge. `test_mint_rejected_while_guard_held` covers it. |

### Tests re-enabled

The merge disabled unit tests by rewriting `#[test]` as `// // #[test]`. In
every module the merge touched, they are back on and passing:

| Module | Tests | Fix needed |
| --- | --- | --- |
| `access_control` | 33 (+2 new) | protocol 28, per-call frames |
| `rate_limiter` | 8 (+4 new) | per-call frames |
| `resource_minter` | 38 | protocol 28, reentrancy guard restored |
| `nebula_gen` | 25 | protocol 28 |
| `migration_framework` | 8 | one frame per call (duplicate-auth `Error(Auth, ExistingValue)`) |
| `pvp_combat` | 5 | `mock_all_auths()`, one frame per call |
| `content_tools` | 3 | `mock_all_auths()`, one frame per call |
| `nebula_explorer` | 1 | none |
| `tournament` | rewritten (see `docs/TOURNAMENT_GUIDE.md`) | Issue #587 |

`cargo test --lib` went from **216** to **343** passing tests before the
tournament work, with no failures.

About 400 `// // #[test]` markers remain in modules the merge did **not**
touch (e.g. `config_updater`, `navigation_planner`, `token_burning`,
`daily_rewards`, `skins`). They need the same mechanical treatment and are best
handled one module per PR. Every fix pattern they need is listed above.

---

## Changed APIs

### resource_minter

**Before (pre-merge)**:
```rust
harvest_resources(env, ship_id, layout) -> Result<HarvestResult, HarvestError>
auto_list_on_dex(env, resource, min_price) -> Result<DexOffer, HarvestError>
```

**After**:
```rust
// Contract entry points (ResourceMinterContract)
mint_resource(env, caller, ship_id, anomaly_index, resource_type, amount)
    -> Result<ResourceRecord, MinterError>          // guarded again (#472/#584)
balance(env, owner, resource_type) -> u64
total_supply(env, resource_type) -> u64
total_minted(env, resource_type) -> u64

// Module helpers used by other modules
harvest_resources(env, ship_id, &layout) -> Result<HarvestResult, HarvestError>
balance_of / credit_balance / debit_balance / move_balance / reduce_supply
```

### access_control (Issue #586)
- `AccessControlError` is `pub` and re-exported at the crate root
  (`stellar_nebula_nomad::AccessControlError`). It is returned by `init_rbac`,
  `grant_role`, `grant_role_batch`, `revoke_role`, `grant_permission`,
  `revoke_permission` and `transfer_admin`. Discriminants are unchanged.

### rate_limiter (Issue #585)
- Declared `pub mod rate_limiter;` with module-level docs. No signature changes:
  `check_rate_limit`, `get_rate_limit_config`, `set_rate_limit_config`,
  `Operation::ALL`, `Operation::default_config`.

### test_helpers (test / `fuzz` feature only)
- New `contract_env() -> Env`, `in_contract(&env, || ..)` and
  `unit_contract_id(&env)` for module-level unit tests.

### tournament (Issue #587)
- Adds tournament formats, scheduling, forfeits, admin controls, disputes and
  prize types. Existing single-elimination calls keep their signatures. See
  `docs/TOURNAMENT_GUIDE.md`.

### Module structure
- `rate_limiter`: per-address DoS prevention
- `nebula_gen`: layout generation with TTL lifecycle (replaces the generator half of `nebula_explorer`)
- `bracket_generator`: new; pure bracket and pairing algorithms for `tournament`

## Files modified for Issue #584
- `MERGE_FIX_STATUS.md`: this document
- `build_errors.txt`: removed (stale merge log)
- `src/resource_minter.rs`: reentrancy guard restored, tests re-enabled
- `src/nebula_gen.rs`, `src/nebula_explorer.rs`, `src/migration_framework.rs`,
  `src/pvp_combat.rs`, `src/content_tools.rs`: tests re-enabled and ported to SDK 28
- `src/test_helpers.rs`: contract-frame helpers
- Repository-wide `cargo fmt` so the CI `Lint` job's `cargo fmt --check` passes

## Merge details
- **Upstream**: Space-Nebula/stellar-nebula-nomad (main branch)
- **Files added**: 2091 objects, 1.30 MiB
- **Major changes**: property-based testing, rate limiting, nebula generation refactor
