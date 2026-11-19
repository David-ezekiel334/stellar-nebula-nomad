//! Bracket and pairing algorithms for the tournament system (Issue #587).
//!
//! Everything here is pure: it takes a player count (and, for Swiss, the
//! standings so far) and returns a list of [`MatchSpec`]s describing *which
//! slots feed which match*. It never reads or writes contract storage, so the
//! same code drives the on-chain [`crate::tournament`] module, the simulation
//! tests and any off-chain tooling that wants to preview a bracket.
//!
//! Players are referred to by **seed index** (`0` = top seed). A slot is fed by
//! a seed, by the winner or loser of an earlier match, or is a bye:
//!
//! | Format | Builder | Real matches for `n` players |
//! | --- | --- | --- |
//! | Single elimination | [`single_elimination`] | `n - 1` |
//! | Double elimination | [`double_elimination`] | `2n - 2` (or `2n - 1` with a grand-final reset) |
//! | Round robin | [`round_robin`] | `n(n - 1) / 2` |
//! | Swiss | [`swiss_pairings`], once per round | `rounds * floor(n / 2)` |
//!
//! Elimination brackets are padded to the next power of two. The padding slots
//! become byes, and [`seed_order`] places them against the top seeds.

use soroban_sdk::{contracttype, Env, Vec};

// ── Formats & slots ─────────────────────────────────────────────────────────

/// Supported tournament formats.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TournamentFormat {
    /// One loss and you are out. Bracket padded to a power of two with byes.
    SingleElimination,
    /// Winners bracket + losers bracket + grand final (with a reset match
    /// when the losers-bracket champion wins the first grand final).
    DoubleElimination,
    /// Everyone plays everyone once (circle method).
    RoundRobin,
    /// `ceil(log2 n)` rounds; each round pairs players on equal scores who
    /// have not met yet.
    Swiss,
}

/// What feeds one side of a match.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotSource {
    /// No player: the other side advances automatically.
    Bye,
    /// The player at this seed index.
    Seed(u32),
    /// The winner of the match with this id.
    WinnerOf(u32),
    /// The loser of the match with this id (double elimination only).
    LoserOf(u32),
}

/// Which part of a bracket a match belongs to.
pub const SIDE_WINNERS: u32 = 0;
/// Losers bracket (double elimination).
pub const SIDE_LOSERS: u32 = 1;
/// Grand final (double elimination).
pub const SIDE_GRAND_FINAL: u32 = 2;
/// Grand-final reset: only played if the losers-bracket champion wins the
/// first grand final (double elimination).
pub const SIDE_GRAND_FINAL_RESET: u32 = 3;
/// Round robin / Swiss pool.
pub const SIDE_POOL: u32 = 4;

/// Sentinel for "no match" in `winner_to` / `loser_to` links.
pub const NO_MATCH: u32 = u32::MAX;

/// One match in a generated bracket.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchSpec {
    /// Dense, 0-based id; equals the spec's index in the generated list.
    pub match_id: u32,
    /// Round number within its side (1-based).
    pub round: u32,
    /// One of the `SIDE_*` constants.
    pub side: u32,
    /// Scheduling slot (1-based): one more than the latest match feeding it,
    /// so every match is scheduled after all of its inputs.
    pub sched: u32,
    pub slot1: SlotSource,
    pub slot2: SlotSource,
}

// ── Limits ──────────────────────────────────────────────────────────────────

/// Smallest field any format accepts.
pub const MIN_PLAYERS: u32 = 2;
/// Largest elimination / Swiss field.
pub const MAX_PLAYERS: u32 = 64;
/// Largest round-robin field: 16 players = 120 matches over 15 rounds, which
/// keeps the per-round storage writes inside Soroban's per-transaction limits.
pub const MAX_ROUND_ROBIN_PLAYERS: u32 = 16;

// ── Small integer helpers ──────────────────────────────────────────────────

/// Smallest power of two `>= n` (returns 1 for `n <= 1`).
pub fn next_power_of_two(n: u32) -> u32 {
    let mut p = 1u32;
    while p < n {
        p *= 2;
    }
    p
}

/// `floor(log2 n)` for `n >= 1` (0 for `n <= 1`).
pub fn log2_floor(mut n: u32) -> u32 {
    let mut r = 0u32;
    while n > 1 {
        n /= 2;
        r += 1;
    }
    r
}

/// `ceil(log2 n)` for `n >= 1`.
pub fn log2_ceil(n: u32) -> u32 {
    log2_floor(next_power_of_two(n))
}

/// True for 1, 2, 4, 8, ...
pub fn is_power_of_two(n: u32) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

/// Shift a UTC timestamp into a display time zone. All on-chain scheduling is
/// in UTC; the organizer's `timezone_offset_minutes` is stored only so clients
/// can show local kick-off times. Returns `None` if the result would be
/// negative or overflow.
pub fn to_local_timestamp(utc: u64, timezone_offset_minutes: i32) -> Option<u64> {
    let shift = i64::from(timezone_offset_minutes).checked_mul(60)?;
    let local = i64::try_from(utc).ok()?.checked_add(shift)?;
    u64::try_from(local).ok()
}

// ── Seeding ─────────────────────────────────────────────────────────────────

/// Standard bracket line-up for `size` slots (a power of two): returns seed
/// indices in bracket order, so adjacent pairs `(0,1), (2,3), ...` are the
/// first-round matches.
///
/// Built recursively so that seeds 0 and 1 can only meet in the final, seeds
/// 0–3 only in the semifinals, and so on: `[0]` → `[0, 1]` →
/// `[0, 3, 1, 2]` → `[0, 7, 3, 4, 1, 6, 2, 5]`. Each pair sums to `size - 1`,
/// so when only `n < size` players exist the missing seeds `n..size` (byes)
/// are drawn against the top seeds.
pub fn seed_order(env: &Env, size: u32) -> Vec<u32> {
    let mut order: Vec<u32> = Vec::new(env);
    order.push_back(0);
    let mut len = 1u32;
    while len < size {
        let next_len = len * 2;
        let mut next: Vec<u32> = Vec::new(env);
        for s in order.iter() {
            next.push_back(s);
            next.push_back(next_len - 1 - s);
        }
        order = next;
        len = next_len;
    }
    order
}

fn seed_or_bye(seed: u32, n: u32) -> SlotSource {
    if seed < n {
        SlotSource::Seed(seed)
    } else {
        SlotSource::Bye
    }
}

fn source_sched(specs: &Vec<MatchSpec>, src: &SlotSource) -> u32 {
    match src {
        SlotSource::Bye | SlotSource::Seed(_) => 0,
        SlotSource::WinnerOf(m) | SlotSource::LoserOf(m) => {
            specs.get(*m).map(|s| s.sched).unwrap_or(0)
        }
    }
}

/// Append a match, deriving its id and scheduling slot. Returns the id.
fn push_match(
    specs: &mut Vec<MatchSpec>,
    round: u32,
    side: u32,
    slot1: SlotSource,
    slot2: SlotSource,
) -> u32 {
    let match_id = specs.len();
    let sched = source_sched(specs, &slot1).max(source_sched(specs, &slot2)) + 1;
    specs.push_back(MatchSpec {
        match_id,
        round,
        side,
        sched,
        slot1,
        slot2,
    });
    match_id
}

/// Build winners-bracket rounds into `specs`. Returns the match ids of each
/// round (outer index = round - 1).
fn build_winners_bracket(env: &Env, specs: &mut Vec<MatchSpec>, n: u32) -> Vec<Vec<u32>> {
    let size = next_power_of_two(n.max(MIN_PLAYERS));
    let order = seed_order(env, size);
    let mut rounds: Vec<Vec<u32>> = Vec::new(env);

    let mut current: Vec<u32> = Vec::new(env);
    let mut i = 0u32;
    while i < size {
        let a = order.get(i).unwrap_or(0);
        let b = order.get(i + 1).unwrap_or(0);
        let id = push_match(specs, 1, SIDE_WINNERS, seed_or_bye(a, n), seed_or_bye(b, n));
        current.push_back(id);
        i += 2;
    }
    rounds.push_back(current.clone());

    let mut round = 2u32;
    while current.len() > 1 {
        let mut next: Vec<u32> = Vec::new(env);
        let mut j = 0u32;
        while j < current.len() {
            let a = current.get(j).unwrap_or(0);
            let b = current.get(j + 1).unwrap_or(0);
            let id = push_match(
                specs,
                round,
                SIDE_WINNERS,
                SlotSource::WinnerOf(a),
                SlotSource::WinnerOf(b),
            );
            next.push_back(id);
            j += 2;
        }
        rounds.push_back(next.clone());
        current = next;
        round += 1;
    }
    rounds
}

// ── Elimination formats ────────────────────────────────────────────────────

/// Single-elimination bracket for `n` players (`n >= 2`).
pub fn single_elimination(env: &Env, n: u32) -> Vec<MatchSpec> {
    let mut specs: Vec<MatchSpec> = Vec::new(env);
    build_winners_bracket(env, &mut specs, n);
    specs
}

/// Double-elimination bracket for `n` players (`n >= 2`).
///
/// Layout for a padded size `2^k`:
/// * winners bracket rounds `1..=k`, same as single elimination;
/// * losers round 1 pairs the winners-round-1 losers;
/// * for each winners round `j` in `2..=k`, a "drop-in" losers round pairs
///   losers-bracket survivors with that round's winners-bracket losers
///   (in reverse order on even rounds, to delay rematches), followed by a
///   consolidation round (except after the last drop-in);
/// * the grand final pits the winners-bracket champion (slot 1) against the
///   losers-bracket champion (slot 2), plus a reset match that is only
///   played if slot 2 wins the first grand final.
pub fn double_elimination(env: &Env, n: u32) -> Vec<MatchSpec> {
    let mut specs: Vec<MatchSpec> = Vec::new(env);
    let wb = build_winners_bracket(env, &mut specs, n);
    let k = wb.len();
    let wb_final = wb.get(k - 1).and_then(|r| r.get(0)).unwrap_or(0);

    // Losers bracket.
    let mut lb_round = 1u32;
    let mut survivors: Vec<u32> = Vec::new(env); // match ids whose winner is alive in LB
    let mut lb_champion_src: Option<SlotSource> = None;

    if k >= 2 {
        let r1 = wb.get(0).unwrap_or(Vec::new(env));
        let mut j = 0u32;
        while j < r1.len() {
            let a = r1.get(j).unwrap_or(0);
            let b = r1.get(j + 1).unwrap_or(0);
            let id = push_match(
                &mut specs,
                lb_round,
                SIDE_LOSERS,
                SlotSource::LoserOf(a),
                SlotSource::LoserOf(b),
            );
            survivors.push_back(id);
            j += 2;
        }

        for wb_round in 2..=k {
            // Drop-in round: LB survivors vs this WB round's losers.
            lb_round += 1;
            let dropping = wb.get(wb_round - 1).unwrap_or(Vec::new(env));
            let count = dropping.len();
            let mut next: Vec<u32> = Vec::new(env);
            for i in 0..count {
                let survivor = survivors.get(i).unwrap_or(0);
                let wb_idx = if wb_round % 2 == 0 { count - 1 - i } else { i };
                let dropper = dropping.get(wb_idx).unwrap_or(0);
                let id = push_match(
                    &mut specs,
                    lb_round,
                    SIDE_LOSERS,
                    SlotSource::WinnerOf(survivor),
                    SlotSource::LoserOf(dropper),
                );
                next.push_back(id);
            }
            survivors = next;

            // Consolidation round (halve the LB field) unless this was the
            // last drop-in.
            if wb_round < k {
                lb_round += 1;
                let mut halved: Vec<u32> = Vec::new(env);
                let mut j = 0u32;
                while j < survivors.len() {
                    let a = survivors.get(j).unwrap_or(0);
                    let b = survivors.get(j + 1).unwrap_or(0);
                    let id = push_match(
                        &mut specs,
                        lb_round,
                        SIDE_LOSERS,
                        SlotSource::WinnerOf(a),
                        SlotSource::WinnerOf(b),
                    );
                    halved.push_back(id);
                    j += 2;
                }
                survivors = halved;
            }
        }
        lb_champion_src = survivors.get(0).map(SlotSource::WinnerOf);
    }

    // Two-player bracket: the loser of the only winners match goes straight
    // to the grand final.
    let slot2 = lb_champion_src.unwrap_or(SlotSource::LoserOf(wb_final));
    let gf = push_match(
        &mut specs,
        k + 1,
        SIDE_GRAND_FINAL,
        SlotSource::WinnerOf(wb_final),
        slot2,
    );
    push_match(
        &mut specs,
        k + 2,
        SIDE_GRAND_FINAL_RESET,
        SlotSource::WinnerOf(gf),
        SlotSource::LoserOf(gf),
    );
    specs
}

// ── Round robin ─────────────────────────────────────────────────────────────

/// Full round-robin schedule for `n` players using the circle method.
///
/// With an odd `n`, a phantom player is added and its pairings are dropped
/// (that player sits the round out), giving `n` rounds; otherwise `n - 1`.
/// Every pair meets exactly once.
pub fn round_robin(env: &Env, n: u32) -> Vec<MatchSpec> {
    let mut specs: Vec<MatchSpec> = Vec::new(env);
    if n < MIN_PLAYERS {
        return specs;
    }
    let m = if n % 2 == 0 { n } else { n + 1 };
    // ring[0] stays fixed; ring[1..] rotates one step per round.
    let mut ring: Vec<u32> = Vec::new(env);
    for i in 0..m {
        ring.push_back(i);
    }
    for round in 1..m {
        for i in 0..(m / 2) {
            let a = ring.get(i).unwrap_or(0);
            let b = ring.get(m - 1 - i).unwrap_or(0);
            if a < n && b < n {
                let match_id = specs.len();
                specs.push_back(MatchSpec {
                    match_id,
                    round,
                    side: SIDE_POOL,
                    sched: round,
                    slot1: SlotSource::Seed(a.min(b)),
                    slot2: SlotSource::Seed(a.max(b)),
                });
            }
        }
        // Rotate positions 1..m clockwise.
        let last = ring.get(m - 1).unwrap_or(0);
        let mut idx = m - 1;
        while idx > 1 {
            let prev = ring.get(idx - 1).unwrap_or(0);
            ring.set(idx, prev);
            idx -= 1;
        }
        ring.set(1, last);
    }
    specs
}

// ── Swiss ───────────────────────────────────────────────────────────────────

/// Number of Swiss rounds for `n` players: `ceil(log2 n)`, at least 1. That is
/// enough rounds for a single undefeated player to emerge in a field of `n`.
pub fn swiss_round_count(n: u32) -> u32 {
    log2_ceil(n.max(MIN_PLAYERS)).max(1)
}

/// Encode an unordered seed pair for "have these two already met?" lookups.
pub fn pair_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (u64::from(lo) << 32) | u64::from(hi)
}

fn has_played(played: &Vec<u64>, a: u32, b: u32) -> bool {
    let key = pair_key(a, b);
    played.iter().any(|k| k == key)
}

/// Pair one Swiss round.
///
/// * `standings`: active seed indices, best first (score, then tiebreaks).
/// * `played`: [`pair_key`]s of every pairing so far.
/// * `had_bye`: seed indices that already received a bye.
///
/// If the field is odd, the lowest-ranked player without a bye sits out with
/// a bye (scored as a win). The rest are paired greedily top-down: each
/// player takes the highest-ranked opponent they have not met. Only if no such
/// opponent is left is a rematch allowed, so the round always completes.
///
/// Returns `(pairs, bye)`, where each pair is `(higher_ranked, lower_ranked)`.
pub fn swiss_pairings(
    env: &Env,
    standings: &Vec<u32>,
    played: &Vec<u64>,
    had_bye: &Vec<u32>,
) -> (Vec<(u32, u32)>, Option<u32>) {
    let mut pool: Vec<u32> = standings.clone();
    let mut bye: Option<u32> = None;

    if pool.len() % 2 == 1 {
        // Lowest-ranked player who has not had a bye yet (fallback: the
        // lowest-ranked player overall).
        let mut pick = pool.len() - 1;
        let mut i = pool.len();
        while i > 0 {
            i -= 1;
            let p = pool.get(i).unwrap_or(0);
            if !had_bye.iter().any(|b| b == p) {
                pick = i;
                break;
            }
        }
        bye = pool.get(pick);
        pool.remove(pick);
    }

    let mut pairs: Vec<(u32, u32)> = Vec::new(env);
    while pool.len() >= 2 {
        let top = pool.get(0).unwrap_or(0);
        let mut opp_idx = 1u32;
        for j in 1..pool.len() {
            if !has_played(played, top, pool.get(j).unwrap_or(0)) {
                opp_idx = j;
                break;
            }
        }
        let opp = pool.get(opp_idx).unwrap_or(0);
        pairs.push_back((top, opp));
        pool.remove(opp_idx);
        pool.remove(0);
    }
    (pairs, bye)
}

// ── Bracket introspection ──────────────────────────────────────────────────

/// Number of rounds (scheduling slots) the format needs for `n` players, not
/// counting an unplayed grand-final reset.
pub fn total_rounds(env: &Env, format: TournamentFormat, n: u32) -> u32 {
    match format {
        TournamentFormat::Swiss => swiss_round_count(n),
        TournamentFormat::RoundRobin => {
            if n % 2 == 0 {
                n.saturating_sub(1)
            } else {
                n
            }
        }
        TournamentFormat::SingleElimination => log2_ceil(n.max(MIN_PLAYERS)),
        TournamentFormat::DoubleElimination => {
            let specs = double_elimination(env, n);
            // Ignore the reset match (last spec).
            let mut max = 0u32;
            for i in 0..specs.len().saturating_sub(1) {
                max = max.max(specs.get(i).map(|s| s.sched).unwrap_or(0));
            }
            max
        }
    }
}

/// For every spec, where its winner and loser go next:
/// `(winner_to, winner_slot, loser_to, loser_slot)`, with [`NO_MATCH`] / 0
/// when the result feeds nothing. Slots are 1 or 2.
pub fn forward_links(env: &Env, specs: &Vec<MatchSpec>) -> Vec<(u32, u32, u32, u32)> {
    let mut links: Vec<(u32, u32, u32, u32)> = Vec::new(env);
    for _ in 0..specs.len() {
        links.push_back((NO_MATCH, 0, NO_MATCH, 0));
    }
    for spec in specs.iter() {
        for (slot_no, src) in [(1u32, &spec.slot1), (2u32, &spec.slot2)] {
            match src {
                SlotSource::WinnerOf(m) => {
                    if let Some(mut l) = links.get(*m) {
                        l.0 = spec.match_id;
                        l.1 = slot_no;
                        links.set(*m, l);
                    }
                }
                SlotSource::LoserOf(m) => {
                    if let Some(mut l) = links.get(*m) {
                        l.2 = spec.match_id;
                        l.3 = slot_no;
                        links.set(*m, l);
                    }
                }
                SlotSource::Bye | SlotSource::Seed(_) => {}
            }
        }
    }
    links
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    /// Resolve an elimination bracket off-chain: `pick(a, b)` chooses the
    /// winner of each real match. Returns `(champion, real_matches_played)`.
    fn play_elimination(
        env: &Env,
        specs: &Vec<MatchSpec>,
        n: u32,
        mut pick: impl FnMut(u32, u32) -> u32,
    ) -> (u32, u32) {
        const NONE: u32 = u32::MAX;
        let len = specs.len() as usize;
        let mut winner = std::vec![NONE; len];
        let mut loser = std::vec![NONE; len];
        let mut played = 0u32;
        let resolve = |src: &SlotSource, winner: &[u32], loser: &[u32]| -> u32 {
            match src {
                SlotSource::Bye => NONE,
                SlotSource::Seed(s) => {
                    if *s < n {
                        *s
                    } else {
                        NONE
                    }
                }
                SlotSource::WinnerOf(m) => winner[*m as usize],
                SlotSource::LoserOf(m) => loser[*m as usize],
            }
        };
        let mut champion = NONE;
        for spec in specs.iter() {
            let id = spec.match_id as usize;
            if spec.side == SIDE_GRAND_FINAL_RESET {
                // Only played if the losers-bracket champion won the GF.
                let gf = id - 1;
                let gf_spec = specs.get(gf as u32).unwrap();
                let wb_champ = resolve(&gf_spec.slot1, &winner, &loser);
                if winner[gf] == wb_champ {
                    champion = winner[gf];
                    break;
                }
            }
            let a = resolve(&spec.slot1, &winner, &loser);
            let b = resolve(&spec.slot2, &winner, &loser);
            let (w, l) = match (a != NONE, b != NONE) {
                (true, true) => {
                    played += 1;
                    let w = pick(a, b);
                    assert!(w == a || w == b);
                    (w, if w == a { b } else { a })
                }
                (true, false) => (a, NONE),
                (false, true) => (b, NONE),
                (false, false) => (NONE, NONE),
            };
            winner[id] = w;
            loser[id] = l;
            champion = w;
        }
        let _ = env;
        (champion, played)
    }

    #[test]
    fn seed_order_matches_standard_brackets() {
        let env = Env::default();
        let o8 = seed_order(&env, 8);
        let expect = [0u32, 7, 3, 4, 1, 6, 2, 5];
        for (i, e) in expect.iter().enumerate() {
            assert_eq!(o8.get(i as u32).unwrap(), *e);
        }
        for size in [2u32, 4, 8, 16, 32, 64] {
            let o = seed_order(&env, size);
            assert_eq!(o.len(), size);
            // Every pair sums to size - 1 and every seed appears once.
            let mut seen = std::vec![false; size as usize];
            let mut i = 0;
            while i < size {
                assert_eq!(o.get(i).unwrap() + o.get(i + 1).unwrap(), size - 1);
                seen[o.get(i).unwrap() as usize] = true;
                seen[o.get(i + 1).unwrap() as usize] = true;
                i += 2;
            }
            assert!(seen.iter().all(|s| *s));
        }
    }

    #[test]
    fn single_elimination_has_n_minus_one_real_matches_for_every_size() {
        let env = Env::default();
        for n in 2..=64u32 {
            let specs = single_elimination(&env, n);
            assert_eq!(specs.len(), next_power_of_two(n) - 1);
            let (champ, played) = play_elimination(&env, &specs, n, |a, b| a.min(b));
            assert_eq!(played, n - 1, "n={n}");
            assert_eq!(champ, 0, "top seed wins when lower seed always wins");
        }
    }

    #[test]
    fn byes_only_go_to_top_seeds_and_never_meet_each_other() {
        let env = Env::default();
        for n in 3..=64u32 {
            let size = next_power_of_two(n);
            let specs = single_elimination(&env, n);
            let byes = size - n;
            let mut bye_matches = 0;
            for spec in specs.iter().filter(|s| s.round == 1) {
                match (&spec.slot1, &spec.slot2) {
                    (SlotSource::Seed(s), SlotSource::Bye)
                    | (SlotSource::Bye, SlotSource::Seed(s)) => {
                        assert!(*s < byes, "n={n}: bye went to seed {s}");
                        bye_matches += 1;
                    }
                    (SlotSource::Bye, SlotSource::Bye) => panic!("double bye n={n}"),
                    _ => {}
                }
            }
            assert_eq!(bye_matches, byes);
        }
    }

    #[test]
    fn double_elimination_match_counts_and_reset_rule() {
        let env = Env::default();
        for n in 2..=64u32 {
            let specs = double_elimination(&env, n);
            // Upsets everywhere: the higher seed number wins, so the
            // losers-bracket champion takes the grand final and forces a reset.
            let (_, played_reset) = play_elimination(&env, &specs, n, |a, b| a.max(b));
            // Favourites everywhere: the winners-bracket champion wins the
            // first grand final, no reset.
            let (champ, played_no_reset) = play_elimination(&env, &specs, n, |a, b| a.min(b));
            assert_eq!(champ, 0);
            assert_eq!(played_no_reset, 2 * n - 2, "n={n}");
            assert!(
                played_reset == 2 * n - 1 || played_reset == 2 * n - 2,
                "n={n} played={played_reset}"
            );
        }
    }

    #[test]
    fn double_elimination_everyone_but_champion_loses_twice() {
        let env = Env::default();
        for n in [2u32, 3, 5, 8, 13, 16, 31, 64] {
            let specs = double_elimination(&env, n);
            let mut losses = std::vec![0u32; n as usize];
            const NONE: u32 = u32::MAX;
            let len = specs.len() as usize;
            let mut w = std::vec![NONE; len];
            let mut l = std::vec![NONE; len];
            let res = |s: &SlotSource, w: &[u32], l: &[u32]| match s {
                SlotSource::Bye => NONE,
                SlotSource::Seed(x) => {
                    if *x < n {
                        *x
                    } else {
                        NONE
                    }
                }
                SlotSource::WinnerOf(m) => w[*m as usize],
                SlotSource::LoserOf(m) => l[*m as usize],
            };
            for spec in specs.iter() {
                let id = spec.match_id as usize;
                let a = res(&spec.slot1, &w, &l);
                let b = res(&spec.slot2, &w, &l);
                if spec.side == SIDE_GRAND_FINAL_RESET
                    && w[id - 1] == res(&specs.get(id as u32 - 1).unwrap().slot1, &w, &l)
                {
                    break;
                }
                if a != NONE && b != NONE {
                    // Deterministic pseudo-random winner.
                    let pick_a = (a * 7 + b * 13 + id as u32) % 3 != 0;
                    let (win, lose) = if pick_a { (a, b) } else { (b, a) };
                    losses[lose as usize] += 1;
                    w[id] = win;
                    l[id] = lose;
                } else {
                    w[id] = if a != NONE { a } else { b };
                }
            }
            let undefeated_twice = losses.iter().filter(|x| **x < 2).count();
            assert_eq!(undefeated_twice, 1, "n={n} losses={losses:?}");
        }
    }

    #[test]
    fn round_robin_pairs_everyone_exactly_once() {
        let env = Env::default();
        for n in 2..=MAX_ROUND_ROBIN_PLAYERS {
            let specs = round_robin(&env, n);
            assert_eq!(specs.len(), n * (n - 1) / 2);
            let mut seen = std::collections::BTreeSet::new();
            for s in specs.iter() {
                let (SlotSource::Seed(a), SlotSource::Seed(b)) = (s.slot1.clone(), s.slot2.clone())
                else {
                    panic!("non-seed slot");
                };
                assert!(a < b && b < n);
                assert!(seen.insert(pair_key(a, b)), "duplicate pair n={n}");
            }
            // Nobody plays twice in one round.
            for round in 1..=total_rounds(&env, TournamentFormat::RoundRobin, n) {
                let mut busy = std::vec![false; n as usize];
                for s in specs.iter().filter(|s| s.round == round) {
                    for slot in [s.slot1.clone(), s.slot2.clone()] {
                        if let SlotSource::Seed(p) = slot {
                            assert!(!busy[p as usize]);
                            busy[p as usize] = true;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn swiss_pairings_avoid_rematches_and_rotate_byes() {
        let env = Env::default();
        for n in [4u32, 5, 7, 8, 9, 16, 33, 64] {
            let mut played: Vec<u64> = Vec::new(&env);
            let mut had_bye: Vec<u32> = Vec::new(&env);
            let mut wins = std::vec![0u32; n as usize];
            let rounds = swiss_round_count(n);
            let mut rematches = 0;
            for r in 0..rounds {
                // Standings: wins desc, then seed asc.
                let mut order: std::vec::Vec<u32> = (0..n).collect();
                order.sort_by(|a, b| wins[*b as usize].cmp(&wins[*a as usize]).then(a.cmp(b)));
                let mut standings = Vec::new(&env);
                for p in order {
                    standings.push_back(p);
                }
                let (pairs, bye) = swiss_pairings(&env, &standings, &played, &had_bye);
                assert_eq!(pairs.len(), n / 2);
                assert_eq!(bye.is_some(), n % 2 == 1);
                if let Some(b) = bye {
                    assert!(!had_bye.iter().any(|x| x == b), "repeat bye n={n}");
                    had_bye.push_back(b);
                    wins[b as usize] += 1;
                }
                for (a, b) in pairs.iter() {
                    if has_played(&played, a, b) {
                        rematches += 1;
                    }
                    played.push_back(pair_key(a, b));
                    let w = if (a + b + r) % 2 == 0 { a } else { b };
                    wins[w as usize] += 1;
                }
            }
            assert_eq!(rematches, 0, "n={n}");
        }
    }

    #[test]
    fn schedule_slots_follow_dependencies() {
        let env = Env::default();
        for n in [2u32, 3, 6, 8, 11, 32] {
            for specs in [single_elimination(&env, n), double_elimination(&env, n)] {
                for s in specs.iter() {
                    for src in [&s.slot1, &s.slot2] {
                        if let SlotSource::WinnerOf(m) | SlotSource::LoserOf(m) = src {
                            assert!(specs.get(*m).unwrap().sched < s.sched);
                            assert!(*m < s.match_id);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn forward_links_point_back_to_sources() {
        let env = Env::default();
        let specs = double_elimination(&env, 8);
        let links = forward_links(&env, &specs);
        for (i, (wt, ws, lt, ls)) in links.iter().enumerate() {
            if wt != NO_MATCH {
                let dest = specs.get(wt).unwrap();
                let slot = if ws == 1 { dest.slot1 } else { dest.slot2 };
                assert_eq!(slot, SlotSource::WinnerOf(i as u32));
            }
            if lt != NO_MATCH {
                let dest = specs.get(lt).unwrap();
                let slot = if ls == 1 { dest.slot1 } else { dest.slot2 };
                assert_eq!(slot, SlotSource::LoserOf(i as u32));
            }
        }
    }

    #[test]
    fn local_time_conversion_handles_offsets() {
        assert_eq!(to_local_timestamp(1_000_000, 60), Some(1_003_600));
        assert_eq!(
            to_local_timestamp(1_000_000, -330),
            Some(1_000_000 - 19_800)
        );
        assert_eq!(to_local_timestamp(10, -720), None);
        assert_eq!(swiss_round_count(2), 1);
        assert_eq!(swiss_round_count(5), 3);
        assert_eq!(swiss_round_count(64), 6);
    }
}
