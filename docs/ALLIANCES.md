# Alliance Management

`src/alliance_manager.rs` runs player alliances (guilds).
`frontend/src/views/AlliancePanel.tsx` is the UI.

## Creation and membership (existing)

| Function | Notes |
| --- | --- |
| `found_alliance(founder, name)` | Name validated by `input_validation::validate_name`. The founder becomes leader. |
| `join_alliance(id, player)` | One alliance per player. Refused with `AllianceFull` at the level's member cap. |
| `leave_alliance(player)` | The leader must `transfer_leadership` first when other members remain (`LeaderMustTransfer`). |
| `contribute_to_treasury(player, amount)` | Daily anti-whale cap applies. |
| `get_alliance`, `get_player_alliance`, `get_alliance_treasury`, `get_member_contribution` | Reads. |

## Member management

`AllianceRole`: `Member < Officer < Leader`.

| Function | Who | Effect |
| --- | --- | --- |
| `get_member_role(id, member) -> Option<AllianceRole>` | anyone | `None` if not a member |
| `set_member_role(leader, member, role)` | leader | `Member` or `Officer` only. Leadership changes go through `transfer_leadership` |
| `kick_member(actor, member)` | officer+ | Target must be of strictly lower rank. Clears membership and role |
| `transfer_leadership(leader, new_leader)` | leader | New leader must be a member. Old leader becomes `Officer` |

The `Alliance.founder` field holds the **current leader**.
`transfer_leadership` updates it, so `guild_economy` (tax rate) and
`guild_quests` (quest creation), which authorize against `founder`, follow the
new leader automatically. Stored `Alliance` values keep their shape.

Events: `("alliance", "role")`, `("alliance", "kicked")`, `("alliance", "leader")`.

## Diplomacy

`DiplomaticStatus`: `Allied`, `Neutral` (default) or `AtWar`.

`propose_relation(leader, target_alliance, status) -> DiplomaticStatus`
(leader only) returns the relation after the call:

- **More hostile** (`Allied → Neutral`, anything → `AtWar`): applies immediately.
- **Friendlier** (`AtWar → Neutral`, `→ Allied`): the first call stores a
  proposal. The relation changes when the other leader proposes the same
  status, which is how a proposal is accepted.
- Proposing the current status, yourself, or an unknown alliance is rejected
  (`InvalidTarget` / `AllianceNotFound`).

Reads: `get_relation(a, b)` (symmetric) and `get_relation_proposal(from, to)`.
Events: `("alliance", "propose")`, `("alliance", "relation")`.

## Shared objectives

| Function | Who |
| --- | --- |
| `create_objective(actor, kind, target, reward_xp, deadline) -> id` | officer+. `target > 0` and `deadline` in the future, else `InvalidObjective` |
| `contribute_objective(member, id, amount)` | any member of that alliance |
| `get_objective(alliance_id, id)`, `get_objective_count(alliance_id)` | anyone |

Progress is capped at `target`. Reaching it marks the objective `completed`
and awards `reward_xp` once through `add_alliance_xp`, which may level the
alliance up. Contributions after completion or after the deadline return
`ObjectiveClosed`. Events: `("alliance", "obj_new")`, `("alliance", "obj_done")`.

## Benefits

`get_alliance_benefits(id) -> AllianceBenefits { level, member_cap, territory_discount_bps }`:

| | Formula | Cap |
| --- | --- | --- |
| Member cap | `50 + 5 × (level − 1)` | `MAX_MEMBER_CAP` = 100 |
| Territory discount | `250 bps × (level − 1)` | 2 500 bps (25 %) |

Applied in `join_alliance` (cap) and `claim_guild_territory`
(`TERRITORY_CLAIM_COST` 500 minus the discount). Levels come from
`add_alliance_xp`: level `n` needs `n × 1000` XP to advance.

## Errors added

| Code | Variant | Meaning |
| --- | --- | --- |
| 9 | `LeaderMustTransfer` | Leader tried to leave a non-empty alliance |
| 10 | `InvalidTarget` | Bad role, self-target, or no-op relation |
| 11 | `ObjectiveNotFound` | |
| 12 | `ObjectiveClosed` | Completed or past deadline |
| 13 | `InvalidObjective` | Zero target or amount, or past deadline |

## UI

`AlliancePanel` shows level, member cap and territory cost. It shows the member
table with Promote, Demote, Make leader and Kick buttons, shown only when the
viewer's role allows them (mirrored in `frontend/src/utils/alliances.ts`). It
lists relations with Declare, Propose or Accept actions for the leader, and
objectives with progress bars and a Contribute button that is disabled once
completed or expired.

## Complexity

- Role, diplomacy and objective operations: O(1) storage operations.
- `kick_member` / `leave_alliance`: O(M) rebuild of the member list (M ≤ 100).
