# Access control patterns

Issue #608. This documents how authorization works in the contract, what the
`require_auth()` audit found, and how it is tested
(`tests/security/access_control_tests.rs`).

## The five patterns

| # | Pattern | Where | How it is enforced |
| - | --- | --- | --- |
| 1 | **Signature** - the actor proves they authorised this call | every state-changing entry point | `actor.require_auth()` (or `ensure_auth!`) before any effect |
| 2 | **Admin-only** - a stored admin / RBAC admin role | `access_control`, `pvp_combat`, `emergency_controls`, `rate_limiter`, oracle, pricing, repair, upgrade, scheduler ... | compare the signed `caller` to the stored admin / `has_role` / `check_permission` |
| 3 | **Ownership** - the actor owns the object | ships, limit orders, vaults, escrows | stored `owner`/`trader` is compared with the signed caller; **object fields supplied by the caller are overwritten**, never trusted |
| 4 | **Membership** - the actor belongs to a guild / combat / queue | `alliance_manager`, `guild_economy`, `guild_quests`, `pvp_combat`, `ship_abilities` | membership looked up from storage, not from a parameter |
| 5 | **Role in an object** - organizer, challenger, opponent, signer | `tournament`, challenges, multisig proposals | compare to the object's stored role holder |

Rules of thumb:

* `require_auth()` proves *who signed*. It does not prove *that they may do this*.
  Always follow it with pattern 2-5 where the data belongs to someone.
* Put `require_auth()` in the **entry point** (the `#[contractimpl]` function in
  `lib.rs`, or the first module function it calls). Do not repeat it in helpers
  that the same frame also calls: the host rejects a second `require_auth()` for
  the same address in one frame (`Auth, ExistingValue`).
* Never take the actor from a parameter you do not authenticate. A function
  that takes `player: Address` but never calls `player.require_auth()` lets
  anyone act as `player`.
* Admin bootstrap functions must be one-shot (`AlreadyInitialized`).

## Audit results

All ~480 contract entry points were reviewed; every function taking an actor
address was checked for authentication, directly or in the module function it
forwards to. Findings, all fixed in this change:

| Finding | Impact | Fix |
| --- | --- | --- |
| `use_ability` had no `require_auth` and accepted any address, treating strangers as player 2 | anyone could attack either combatant / drain their energy | auth in the entry point **and** participant check (`Unauthorized`) |
| `init_combat_abilities`, `tick_ability_cooldowns` unauthenticated | anyone could reset another player's ability cooldowns | `player.require_auth()` |
| `process_anti_whale_action(user, amount)` unauthenticated | anyone could burn a victim's daily trade/mint cap (griefing) | `user.require_auth()` |
| `generate_yield_forecast`, `update_forecast_model` unauthenticated | anyone could poison a player's forecast history | `player.require_auth()` |
| `create_player_reputation` unauthenticated | anyone could create storage entries for arbitrary addresses | `player.require_auth()` |
| `guild_quests::create_guild_quest` accepted any signer for any alliance | quests credit `reward_essence` to the treasury on completion, so anyone could mint treasury funds | only the alliance founder may create quests (`Unauthorized`); zero targets rejected |
| `access_control::transfer_admin` re-entered `revoke_role`/`grant_role`, each re-authenticating the caller in the same frame | the transfer could never succeed on-chain (`Auth, ExistingValue`), so admin rotation was impossible | roles are moved with the storage helpers directly |

Intentionally permissionless (documented in the audit test's allow-list):
`apply_elo_decay` (keeper), `process_matchmaking` (keeper), read-only getters.

Known limitations, not changed here: `init_roles`, `set_pvp_admin` and the other
`initialize_*` bootstraps can be called first by anyone (first caller wins);
deployments must run them in the deploy transaction. `spawn_guild_boss` lets any
member spend treasury funds (design decision, no role check).

## How it is tested

| Test | What it proves |
| --- | --- |
| `state_changing_entry_points_reject_unsigned_calls` | with all mocks cleared, ~20 representative entry points across PvP, trading, ships, guilds and admin fail (`require_auth` reached). A positive control proves the same calls work when signed |
| `every_state_changing_entry_point_authenticates_its_actor` | static scan of **every** `#[contractimpl]` function with an actor parameter: it must authenticate directly or in its callee; exceptions are listed with a reason. New unauthenticated entry points fail CI |
| RBAC tests | one-shot init, admin-only grant/revoke/batch/permissions, role-scoped permissions, admin transfer locks out the old admin, emergency mode, delegation needs the role, multisig signer/threshold/replay rules |
| admin tests | PvP admin (set once, gates config), emergency controls, rate-limit config |
| ownership tests | ship transfer, limit-order cancel (and forged `trader` field), challenge accept/decline, combat turns, ability use |
| guild tests | membership rules, founder-only quests, member-only progress and treasury actions |
| tournament / matchmaking tests | organizer-only start, registration rules, per-player queue leave/dequeue |

## Adding a new entry point

1. `actor.require_auth()` first.
2. Check admin/ownership/membership against **stored** state.
3. Add a test with a different, validly signed caller expecting the typed error,
   and (for new user entry points) add it to the unsigned-call test.
4. Run `cargo test --test security_access_control`; the static audit will tell
   you if you forgot step 1.
