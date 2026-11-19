# Input validation patterns

Issue #611. Contract entry points must turn bad input into a typed error, never
a trap. Validation lives in `src/input_validation.rs` (checks) and
`src/validation_macros.rs` (early-return macros).

## Order of checks in an entry point

1. `caller.require_auth()`
2. rate limit (`rate_limiter::check_rate_limit`, see `docs/RATE_LIMITING.md`)
3. **input validation** (ranges, self-reference, durations, sizes)
4. state validation (exists, right phase, caller owns it)
5. arithmetic with `checked_*` / `safe_*`, never bare `+ - *` on caller-influenced values

## Helpers

| Helper | Rejects |
| --- | --- |
| `check_range`, `check_positive`, `check_non_negative`, `check_percentage` | out-of-range numbers |
| `check_distinct_addresses` | self-challenge / self-trade (`SelfReference`) |
| `checked_deadline(now, window, min, max)` | zero/huge windows and `u64` overflow (`InvalidTimestamp`) |
| `check_bps_distribution(shares, max_entries)` | too many shares, `u32` overflow, total > 10_000 |
| `safe_mul`, `safe_add` | `i128` overflow (`ArithmeticOverflow`) |
| `validate_name/description/metadata_uri/cid`, `check_no_injection` | empty, oversized, control-char strings |
| `validate_region_id`, `validate_seed` | region 0 / > max, degenerate seeds |
| `check_array_size`, `check_unique_elements` | oversized / duplicate lists |

Macros (`#[macro_export]`, usable as `crate::name!`): `validate_range!`,
`validate_positive!`, `validate_non_negative!`, `validate_percentage!`,
`validate_distinct!`, `validate_string_length!`, `validate_array_size!`,
`validate_no_duplicates!`, `validate_no_injection!`, `safe_mul!`, `safe_add!`,
and the generic `ensure!(cond, err)`. They `return Err(..)` via `?`, so the
function's error type needs `From<ValidationError>` (or map explicitly with
`.map_err`, as the game modules do to keep their own error codes).

## What was added to which module

| Module | Validation |
| --- | --- |
| `tournament` | player count power of two in [4, 64]; registration window in [60 s, 30 d] with overflow-safe deadline; prize bps list bounded, overflow-safe, <= 100%; unaffordable entry fee now returns `InsufficientFunds` instead of `RegistrationClosed` |
| `pvp_combat` | no self-challenges; stake in [0, `MAX_STAKE`] for challenges and matchmaking; move `power` in [1, `MAX_MOVE_POWER`] |
| `crafting` | XP / level arithmetic saturates (`add_xp(u32::MAX)` no longer panics) |
| `trading` | order price and quantity in (0, `MAX_ORDER_VALUE`]; AMM mint/swap/remove maths is checked and returns `InvalidAmount` instead of trapping on overflow |

## Adding validation to a new function

```rust
pub fn create_thing(env: &Env, owner: &Address, rival: &Address, window: u64)
    -> Result<u64, ThingError>
{
    owner.require_auth();
    check_rate_limit(env, owner, Operation::Thing)?;
    check_distinct_addresses(owner, rival).map_err(|_| ThingError::InvalidParams)?;
    let deadline = checked_deadline(env.ledger().timestamp(), window, MIN, MAX)
        .map_err(|_| ThingError::InvalidWindow)?;
    ...
}
```

Test every new bound at both edges (`MIN-1`, `MIN`, `MAX`, `MAX+1`) plus the
type extremes (`0`, `-1`, `u64::MAX`, `i128::MAX`); see
`tests/security/input_validation_tests.rs`.
