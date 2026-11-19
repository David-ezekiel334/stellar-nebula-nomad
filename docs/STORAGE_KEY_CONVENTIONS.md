# Storage key conventions

Issue #609. Every module in this crate runs inside the same contract and
therefore shares one storage namespace (per tier: `instance`, `persistent`,
`temporary`).

## Why collisions happen

A `#[contracttype]` enum variant is serialised as
`Vec[Symbol(<variant name>), ..payload fields]`. The **enum's own name and the
module it lives in are not part of the key**. Two key enums that both declare

```rust
// dao.rs                         // staking.rs
enum DataKey { Admin, .. }        enum DataKey { Admin, .. }
```

encode `DataKey::Admin` to the identical `["Admin"]` key, so whichever module
initialises last silently overwrites the other's admin. The same applies to
payload variants with identical field types, e.g. `Challenge(u64)`.
(Variants that differ in payload types, such as `Entry(Address, Operation)` vs
`Entry(u64)`, do not clash, but rely on that only when unavoidable.)

## The convention

1. **Prefix every key variant with the module it belongs to**:
   `<ModulePrefix><Thing>`, e.g. `StakingAdmin`, `DaoConfig`, `PvpChallenge(u64)`,
   `GasSponsorSessionKey(..)`. The prefix is the module name in PascalCase,
   abbreviated when needed to stay readable (`Pvp`, `Gov`, `Eth`, `SessMgr`).
2. **Never use bare generic names** for a key: `Admin`, `Config`, `Counter`,
   `State`, `Paused`, `Version`, `Metrics`, `Registry`, `Session`, `Leaderboard`,
   `Initialized`.
3. **Variant names must be at most 32 characters** (a Soroban `Symbol`).
4. Name key enums `<Module>Key` (or `DataKey` for a module-private one) so the
   validators can find them. Do not reuse a variant name for a different
   meaning in the same module.
5. Do not rename a variant of an already-deployed contract without a migration
   (see `docs/MIGRATION_GUIDE.md`): the rename changes the stored key.

## Enforcement

| Check | How to run |
| --- | --- |
| Validator script (collisions, >32 chars) | `python3 scripts/check_storage_keys.py` |
| Same plus bare generic names | `python3 scripts/check_storage_keys.py --strict` |
| Rust test suite | `cargo test --test security_storage_keys` |

CI runs the strict validator and the Rust tests, so a new module that reuses
an existing `(name, payload)` pair fails the build.

## What changed in Issue #609

An audit of the 117 key enums (587 variants) found 24 colliding
`(variant, payload)` signatures, the worst being `Admin`, which was declared by
23 different enums, plus `Config`, `Counter`, `Paused`, `Session`, `Proposal(u64)`,
`PlayerStats(Address)`, `Version`, and others. Every colliding variant, and
every remaining bare generic name, was renamed with its module prefix. Because
variants are renamed in place, no call-site logic changed.

**Upgrade note:** key encodings changed for the renamed variants. A contract
that already holds data under the old keys needs a migration that copies the
old keys to the new ones; fresh deployments are unaffected. The collisions
themselves meant old deployments could not hold correct data for the colliding
keys.
