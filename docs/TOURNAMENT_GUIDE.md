# Tournament Guide

Issue #587 (builds on #284 / #537). Code: `src/tournament.rs` (state and
lifecycle) and `src/bracket_generator.rs` (pure bracket and pairing
algorithms). UI: `frontend/src/views/Tournaments.tsx` and
`frontend/src/components/TournamentBracket.tsx`.

## Formats

| Format | `max_players` | Real matches for `n` players | Notes |
| --- | --- | --- | --- |
| `SingleElimination` | power of two, 4–64 | `n - 1` | Padded to a power of two. Byes go to the top seeds. |
| `DoubleElimination` | power of two, 4–64 | `2n - 2`, or `2n - 1` with a reset | Winners bracket, losers bracket, then a grand final. The reset match is played only if the losers-bracket champion wins the first grand final. |
| `RoundRobin` | 4–16 | `n(n - 1) / 2` | Circle method. With an odd `n`, one player sits out each round. |
| `Swiss` | 4–64 | `ceil(log2 n) * floor(n / 2)` | Players on equal scores are paired, with no rematches. With an odd field, the lowest-ranked player who hasn't had a bye gets one (counts as a win). |

Seeding is by PvP ELO (highest first, ties in registration order). The
`seed_order` line-up keeps seeds 1 and 2 apart until the final.

## Lifecycle

```text
create ──► reg ──start──► active ──last match──► final ──finalize──► done
             └────────── cancel (refunds) ─────────┘          (after dispute window)
```

1. **Create.** The legacy call `create_tournament(...)` makes a single-elimination
   tournament that starts combats automatically and has no dispute window.
   `create_tournament_with_config(env, organizer, TournamentConfig { .. })` lets
   you choose:
   * `format`, `resource_type`, `entry_fee`, `max_players`, `registration_window_secs`
   * `prize_distribution_bps`: share per placement, best first, summing to 10,000 or less
   * `first_round_at` (UTC, 0 = when started), `round_interval_secs`, `match_window_secs` (5 min to 7 days)
   * `dispute_window_secs` (0–7 days), `timezone_offset_minutes` (−720 to +840, display only)
   * `auto_start_combats`
2. **Register.** `register_for_tournament` is rate-limited. The entry fee moves
   into the contract's escrow balance.
3. **Sponsor (optional).** `sponsor_prize(sponsor, id, resource_type, amount)`
   adds an extra prize pool in any resource. Several pools are allowed.
4. **Start.** The organizer or an RBAC admin calls `start_tournament`, which
   seeds the players and builds the bracket. Byes resolve immediately. Swiss
   rounds are paired one at a time as each round finishes.
5. **Play.** See the next section.
6. **Final.** When the last match is decided, placements are computed and the
   status becomes `final`. After `dispute_window_secs`, with no disputes open,
   anyone can call `finalize_tournament` to pay out. With a window of 0,
   payout happens immediately.

## Matches, scheduling and forfeits

Each match has a scheduling slot `sched`: one more than the latest match
feeding it. Kick-off is `schedule_base + (sched - 1) * round_interval_secs`,
in UTC. A match becomes **ready** once both players are known. Its deadline
is `max(kick-off, ready time) + match_window_secs`, so a match that becomes
ready late still gets its full window. Use `local_kickoff(id, match_id)` or
`formatKickoff()` in the UI to show the organizer's local time.

| Call | Who | What |
| --- | --- | --- |
| `check_in(player, id, match)` | match player | Open from 15 min before kick-off until the deadline. |
| `start_match(caller, id, match)` | player or admin | Starts the PvP combat. Not needed with `auto_start_combats`. |
| `report_result(id, match)` | anyone | Copies the winner from the finished combat. Legacy: `report_match_result(id, round, index)`. |
| `claim_forfeit(caller, id, match)` | anyone, after the deadline | If only one player checked in, that player wins. If nobody did, the higher seed advances. If both did, or a combat is running, it fails with `ForfeitNotAllowed`. |

Spectators watch a match through `pvp_combat::add_spectator` using the
match's `combat_id`.

## Admin panel (organizer or RBAC `admin`)

| Call | Effect |
| --- | --- |
| `admin_set_result(caller, id, match, winner)` | Referee decision on a ready or live match. |
| `extend_match_deadline(caller, id, match, secs)` | Gives a match more time before forfeits can be claimed. |
| `disqualify_player(caller, id, player)` | During registration: removes and refunds the player. While active: forfeits their current and future matches. |
| `cancel_tournament(caller, id)` | Refunds every entry fee and sponsor pool. |
| `get_admin_overview(id)` | Counts: registrants, matches decided/live/overdue, open disputes, pools. |

## Disputes

* `raise_dispute(player, id, match, reason)` can only be called by one of the
  match's players, within `dispute_window_secs` of the result, and only while
  the matches it feeds are still undecided. Results that already moved the
  bracket on are locked (`ResultLocked`).
* While a dispute is open, matches fed by the disputed one can't be decided,
  and the tournament can't be finalized.
* `resolve_dispute(admin, id, match, overturn)`: overturning swaps the winner
  and loser and re-routes both into the downstream matches. Overturning a
  double-elimination grand final can bring the reset match back, which sets
  the tournament back to `active`.

## Prizes

| Type | Funded by | Paid in | Undistributed share goes to |
| --- | --- | --- | --- |
| Entry-fee pool | registrants | `resource_type` | organizer |
| Sponsor pools | `sponsor_prize` | any `ResourceType` | that sponsor |
| Title | – | `get_player_titles(player)` += 1 for the champion | – |

Every pool uses the same `prize_distribution_bps`. All transfers use
`resource_minter::move_balance` through the contract's escrow, so total supply
never changes. Placements: elimination ranks the champion first, then losers
by how late they went out. Round robin and Swiss rank by wins, then tiebreak
(Swiss: Buchholz; round robin: Sonneborn–Berger), then seed.

## Testing

* `src/bracket_generator.rs` checks the generators for every field size 2–64:
  match counts, byes, rematches and schedule ordering.
* `src/tournament.rs` covers the legacy flow, all four formats, scheduling and
  time zones, check-ins and forfeits, the admin panel, disputes and multi-pool
  prizes. `simulate_quick` (about 300 matches) runs in CI.
* `simulate_ten_thousand_matches` plays more than 10,000 matches across every
  format and checks that each tournament completes, every player is placed
  exactly once, match counts are right and prize payouts are exact. It is too
  slow for a debug or coverage run, so it is opt-in:

```bash
cargo test --lib tournament:: bracket_generator::
cargo test --release --lib simulate_ten_thousand -- --ignored
```
