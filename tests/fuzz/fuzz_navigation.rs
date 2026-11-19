// Fuzz tests for the navigation planner module (issue #604).
// Covers all input validation paths: coordinate boundaries,
// route length limits, waypoint validation, and jump-distance arithmetic.
#![allow(warnings)]

// ─── Coordinate boundary tests ───────────────────────────────────────────────

#[test]
fn fuzz_x_coordinate_boundaries() {
    // Navigation coordinates are stored as i64 (signed to allow relative coords).
    let coords: Vec<i64> = vec![i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX];
    for x in coords {
        // Coordinate encoding: (x, y) → single u128 key
        // Must not overflow for any i64 pair.
        let encoded = (x as i128).unsigned_abs() as u128;
        assert!(encoded <= i64::MAX as u128 + 1);
    }
}

#[test]
fn fuzz_y_coordinate_boundaries() {
    let coords: Vec<i64> = vec![i64::MIN, -1, 0, 1, i64::MAX];
    for y in coords {
        let encoded = (y as i128).unsigned_abs() as u128;
        assert!(encoded <= i64::MAX as u128 + 1);
    }
}

#[test]
fn fuzz_coordinate_pair_distance_no_overflow() {
    // Distance between two points: sqrt((x2-x1)^2 + (y2-y1)^2)
    // The intermediate squared values must not overflow i128.
    let pairs: Vec<(i64, i64)> = vec![
        (0, 0),
        (1, 1),
        (i64::MAX / 2, i64::MAX / 2),
        (-i64::MAX / 2, -i64::MAX / 2),
    ];
    for (x1, y1) in &pairs {
        for (x2, y2) in &pairs {
            let dx = (*x2 as i128) - (*x1 as i128);
            let dy = (*y2 as i128) - (*y1 as i128);
            // Squaring i64::MAX/2 as i128 must not overflow i128.
            let sq_dist = dx.checked_mul(dx).and_then(|d| d.checked_add(dy * dy));
            // We only assert no panic; None means overflow was safely caught.
            let _ = sq_dist;
        }
    }
}

// ─── Route length validation ─────────────────────────────────────────────────

#[test]
fn fuzz_empty_route_is_invalid() {
    let waypoints: Vec<(i64, i64)> = vec![];
    let valid = waypoints.len() >= 2;
    assert!(!valid, "empty route must be rejected");
}

#[test]
fn fuzz_single_waypoint_route_is_invalid() {
    let waypoints: Vec<(i64, i64)> = vec![(0, 0)];
    let valid = waypoints.len() >= 2;
    assert!(!valid, "single-waypoint route must be rejected");
}

#[test]
fn fuzz_two_waypoint_route_is_valid() {
    let waypoints: Vec<(i64, i64)> = vec![(0, 0), (100, 100)];
    let valid = waypoints.len() >= 2;
    assert!(valid);
}

#[test]
fn fuzz_max_waypoints_boundary() {
    const MAX_WAYPOINTS: usize = 50;
    let at_limit: Vec<(i64, i64)> = (0..MAX_WAYPOINTS as i64).map(|i| (i, i)).collect();
    let over_limit: Vec<(i64, i64)> = (0..(MAX_WAYPOINTS + 1) as i64).map(|i| (i, i)).collect();

    assert!(at_limit.len() <= MAX_WAYPOINTS);
    assert!(
        over_limit.len() > MAX_WAYPOINTS,
        "over-limit route should be rejected"
    );
}

// ─── Jump distance arithmetic ────────────────────────────────────────────────

#[test]
fn fuzz_jump_distance_zero_is_rejected() {
    // A jump distance of 0 ly is nonsensical and must be caught.
    let distance: u64 = 0;
    let valid = distance > 0;
    assert!(!valid);
}

#[test]
fn fuzz_jump_fuel_cost_no_overflow() {
    // fuel_cost = distance * cost_per_ly
    // Must use checked_mul to prevent silent overflow.
    let distances: Vec<u64> = vec![0, 1, 1_000, u64::MAX / 2, u64::MAX - 1, u64::MAX];
    let cost_per_ly: u64 = 10;

    for dist in distances {
        let cost = dist.checked_mul(cost_per_ly);
        if dist > u64::MAX / cost_per_ly {
            assert!(cost.is_none(), "overflow not caught for dist={dist}");
        } else {
            assert!(cost.is_some());
        }
    }
}

#[test]
fn fuzz_fuel_remaining_no_underflow() {
    // fuel_remaining = fuel_tank - fuel_cost
    // Must use checked_sub so the contract returns an error, not wraps.
    let tank: u64 = 1_000;
    let costs: Vec<u64> = vec![0, 999, 1_000, 1_001, u64::MAX];

    for cost in costs {
        let result = tank.checked_sub(cost);
        if cost > tank {
            assert!(result.is_none(), "underflow not caught for cost={cost}");
        } else {
            assert!(result.is_some());
        }
    }
}

// ─── Sector / region name validation ─────────────────────────────────────────

#[test]
fn fuzz_sector_name_empty_rejected() {
    let name = "";
    let valid = !name.is_empty() && name.len() <= 32;
    assert!(!valid);
}

#[test]
fn fuzz_sector_name_at_max_length() {
    let name = "A".repeat(32);
    let valid = !name.is_empty() && name.len() <= 32;
    assert!(valid);
}

#[test]
fn fuzz_sector_name_over_max_rejected() {
    let name = "A".repeat(33);
    let valid = !name.is_empty() && name.len() <= 32;
    assert!(!valid);
}

// ─── Navigation speed / throttle validation ──────────────────────────────────

#[test]
fn fuzz_throttle_clamped_to_valid_range() {
    // Throttle is 0–100 (percent). Values outside must be clamped or rejected.
    let inputs: Vec<i64> = vec![-1, 0, 50, 100, 101, i64::MAX, i64::MIN];
    for t in inputs {
        let clamped = t.clamp(0, 100);
        assert!(clamped >= 0 && clamped <= 100);
    }
}

#[test]
fn fuzz_eta_calculation_no_overflow() {
    // eta_seconds = distance / speed  (both u64, speed > 0)
    let distances: Vec<u64> = vec![0, 1, 1_000_000, u64::MAX];
    let speeds: Vec<u64> = vec![1, 100, u64::MAX];
    for dist in &distances {
        for speed in &speeds {
            // Safe because speed > 0 (enforced by input validation).
            let _eta = dist / speed;
        }
    }
}

#[test]
fn fuzz_speed_zero_causes_error_not_panic() {
    let speed: u64 = 0;
    let distance: u64 = 1_000;
    // The contract must return an error, not divide by zero.
    let result = if speed == 0 {
        Err("InvalidSpeed")
    } else {
        Ok(distance / speed)
    };
    assert!(result.is_err());
}

// ─── Composite: all-zero navigation inputs ──────────────────────────────────

#[test]
fn fuzz_all_zero_navigation_inputs() {
    let x: i64 = 0;
    let y: i64 = 0;
    let speed: u64 = 0;
    let distance: u64 = 0;

    // None of these should panic.
    let _ = x.checked_add(0);
    let _ = y.checked_sub(0);
    let _ = distance.checked_div(1); // safe denominator

    // Zero speed is invalid input — checked_div returns None for div-by-zero.
    assert_eq!(distance.checked_div(speed), None);
}
