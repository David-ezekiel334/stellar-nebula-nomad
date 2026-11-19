# Changelog

All notable changes to Nebula Nomad are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- **Access control audit (#608):** `use_ability`, `init_combat_abilities`,
  `tick_ability_cooldowns`, `process_anti_whale_action`,
  `generate_yield_forecast`, `update_forecast_model` and
  `create_player_reputation` now require the acting address's signature;
  `use_ability` also rejects non-participants. Only an alliance's founder can
  create guild quests (previously any signer could mint treasury rewards).
  `access_control::transfer_admin` no longer re-authenticates the caller inside
  `revoke_role`/`grant_role` (it could never succeed on-chain). New
  `tests/security/access_control_tests.rs` (incl. a static audit of every entry
  point) and `docs/ACCESS_CONTROL_PATTERNS.md`.
- **Rate limiting (#610):** new `CombatInitiation`, `TournamentRegistration`,
  `Crafting` and `Trading` operations enforced in `pvp_combat`, `tournament`,
  `crafting` and `trading`; `get_rate_limit_config`; saturating window math.
  See `docs/RATE_LIMITING.md`.
- **Input validation (#611):** bounded tournament players/window/prize shares,
  PvP stakes and move power, limit-order price/quantity; checked AMM
  arithmetic; self-challenge rejected; new helpers/macros in
  `input_validation` and `validation_macros`. See `docs/INPUT_VALIDATION.md`.
  `TournamentError::InsufficientFunds` replaces a misleading
  `RegistrationClosed` for unaffordable entry fees.

### Fixed

- **Storage key collisions (#609):** 24 colliding `(variant, payload)` key
  signatures across 117 key enums (e.g. `Admin` in 23 modules) are now
  module-prefixed, e.g. `StakingAdmin`, `DaoConfig`. **Stored key encodings
  changed**; existing deployments need a migration (see
  `docs/STORAGE_KEY_CONVENTIONS.md`). Adds `scripts/check_storage_keys.py`
  (run in CI) and `tests/security/storage_key_collisions.rs`.
- `main` did not compile: fixed conflicting `MatchResult`/elo re-exports in
  `lib.rs`, a missing return in `ship_abilities::hauler_abilities`, and the
  `tests/fuzz` borrow/overflow assertions. The `tournament` and
  `validation_macros` modules existed but were never declared; they are now
  compiled.

### Changed

- **Breaking (#573):** the anomaly-based layout in `nebula_gen` is renamed
  `NebulaLayout` → `NebulaGenLayout`. Two `#[contracttype]` structs named
  `NebulaLayout` (one in `nebula_explorer`, one in `nebula_gen`) produced a
  conflicting contract spec. `nebula_explorer::NebulaLayout` (the 16×16 cell
  grid returned by `generate_nebula_layout` / `scan_nebula`) is now the only
  `NebulaLayout`. Migration: replace `nebula_gen::NebulaLayout` with
  `nebula_gen::NebulaGenLayout` (or the crate-root `NebulaGenLayout`, which was
  already its re-export name). Stored data is unaffected: Soroban encodes struct
  values by field name, not type name.

### Fixed

- `cargo test` builds again (#572): the duplicate `pub mod clan_wars;` in
  `lib.rs` (E0428) is removed, and the disabled
  `marketplace_listing_still_emits_once` gas test, which called the removed
  `nft_marketplace::list_ship`, is excluded from compilation until it is ported
  to `create_listing`. The Test & Build matrix no longer cancels stable when
  nightly fails (`fail-fast: false`).
- Harvest migration (#574) is complete: `harvest_resources`, `auto_list_on_dex`
  and the `HarvestError` / `HarvestResult` / `DexOffer` / `HarvestedResource`
  types are implemented in `resource_minter`, `dex_integration::harvest_and_list`
  builds on them, and every import across the crate resolves. Callers that
  only need to credit resources should use `mint_resource`.
- `CacheEntry.value` is a Soroban `Bytes` (#575), not `Vec<u8>`, so it is a
  valid `#[contracttype]` field.

Economy rebalance: a compounding ship-upgrade curve, two new resource sinks, and
an oracle-driven dynamic pricing engine for the DEX.

### Added

#### Dynamic pricing engine (#452)

- `src/dynamic_pricing.rs`: a per-resource price oracle with exponential moving
  average smoothing (`smoothing_bps`, default 20%), a deviation guard that
  rejects prints more than ±50% from the average, a realized-volatility EWMA
  that widens the published-price band, and a bounded supply/demand adjustment
  (±25%) driven by the balancer's parity convention.
- Published prices are clamped to the average ± `band_bps`; the band widens with
  observed volatility up to ±50%, so a calm market gets a tight band and a
  volatile one absorbs shocks instead of repricing violently.
- Admin configuration with a per-resource cooldown (default 60s). A print
  outside the deviation cap is **not** an error: it returns the current state
  with `rejected_last` and `rejected_prints` incremented, an event emitted, and
  **no** history entry — a rejected print must not move the average or become a
  history point. (A returned `Err` would roll back the counter and the event,
  because Soroban discards state on a failed invocation.)
- Bounded history (24 observations per resource) with `get_pricing_history` and
  `get_price_state` for inspection.
- `get_dynamic_price` returns `ResourceNotFound` for a resource that has never
  been observed, rather than inventing a price.
- `list_at_market` in the DEX prices offers from the oracle instead of a
  caller-supplied price, and returns a structured `DynamicListError`
  (`PriceUnavailable`, `HarvestFailed`, `ShipNotFound`, `NotOwner`,
  `RateLimitExceeded`, `ArithmeticOverflow`) so a listing failure says which
  precondition failed.

#### Ship repair sink (#453)

- `src/ship_repair.rs`: `repair_ship` and `quote_repair`, burning harvested
  resources to restore durability. Configurable per-point cost (default 2) with
  an optional emergency surcharge (+50% for emergency field patches) and a
  per-call cap (default 50 points).
- `get_total_repair_burn` — a cumulative, monotonically non-decreasing
  accounting of every resource destroyed through repair.
- New rate-limit operation `Operation::ShipRepair`, defaulting to 3 calls per
  300 seconds per address, counted across all of a player's ships.

#### Ship upgrade telemetry (#454)

- `get_total_upgrade_spend` — cumulative resources consumed by upgrades,
  reported separately from the new dynamic-pricing sink so the two can be
  tuned independently.

#### Crafting sink telemetry (#453)

- `get_total_craft_sink` — cumulative resources consumed by crafting.
- `craft_with_overcharge`: a special crafting action that charges 100% extra on
  the primary input in exchange for a **guaranteed** discovery roll. Normal
  crafting keeps its probabilistic discovery; the overcharge buys certainty.

#### Runbooks (#451)

- `docs/runbooks/deployment.md` — pre-flight checks, storage-key collision
  check, deploy, post-deploy configuration, and promotion gates.
- `docs/runbooks/troubleshooting.md` — symptom-first diagnostics for all three
  subsystems, with the exact error codes and constraints to check.
- `docs/runbooks/rollback.md` — four rollback tiers, from an on-chain config
  retune to redeploying the previous WASM, with the cost and the state
  consequences of each.
- `docs/runbooks/incident-response.md` — severity definitions, a 15-minute
  first-response procedure, and playbooks for fund loss, sink corruption,
  mispricing, and error spikes.

### Changed

#### Ship upgrade cost curve (#454)

- Upgrade costs now compound at +60% per installed module instead of being
  flat, with a configurable `max_cost` ceiling per module.
- For a blueprint with `resource_cost = 100`, the cumulative cost to reach
  fully-upgraded is 1579, up from 500 under the flat schedule. The cost of the
  fifth module rises most sharply (409 → 654), which is the intended
  progression pressure.
- Setting `growth_bps = 0` restores the legacy flat schedule exactly. This is
  the primary rollback lever and requires no redeploy.
- Existing hulls are unaffected: installed modules persist, and already-spent
  resources are not refunded.

### Fixed

- Storage keys in the new modules are module-prefixed (`PricingConfig`,
  `PricingAdmin`, `PricingState`, `PricingHistory`). A `#[contracttype]` unit
  variant encodes to the instance key `["<VariantName>"]`, so a bare `Config`
  or `Admin` variant collides across modules and whichever module initialised
  last silently owns the slot. The new modules avoid this; pre-existing
  collisions elsewhere in the repository are unchanged and tracked separately.

### Notes

- This repository has a large pre-existing test-failure backlog in modules
  unrelated to this release. The release gate for these changes is:
  `cargo test --lib ship_upgrade::`, `cargo test --lib ship_repair::`,
  `cargo test --lib dynamic_pricing::`, `cargo test --test test_ship_upgrade`,
  `cargo test --test test_ship_repair`, `cargo test --test test_dynamic_pricing`,
  and `cargo test --test test_dex_integration`.
