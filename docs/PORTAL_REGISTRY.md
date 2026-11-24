# Portal Registry and Navigation

`src/portal_registry.rs` manages player-built portals between nebulae.
`frontend/src/views/PortalNetwork.tsx` is the navigation UI.

## Core (existing)

| Function | Notes |
| --- | --- |
| `initialize_portal_registry(admin)` | Sets the registry admin. |
| `register_portal(owner, source, target)` / `register_portal_batch` (≤ 8) | New portal at 100 % stability. |
| `query_portal_status(id)` | `(stability %, travel cost)`. |
| `refresh_portal(owner, id)` | Back to 100 %. |
| `get_portal(id)` | Raw `Portal`. |

Stability decays `DECAY_RATE_PER_DAY` (2) points per full day since the last
refresh. Travel cost is `BASE_TRAVEL_COST * 100 / stability`. Travel is refused
below `MIN_STABLE_PCT` (10 %).

## Discovery

- `discover_portals(nebula_id) -> Vec<u64>`: IDs of portals leaving that
  nebula, in registration order. The `PortalsFromNebula(source)` index is
  written by `register_portal`.
- `get_portal_count()`: IDs run from `0` to `count - 1`.

Portals registered before this release are not in the discovery index. Use
`get_portal_topology` to list every portal.

## Connections

- `set_portal_open(caller, id, open)`: close or reopen a portal. Allowed for
  the portal owner or the registry admin. Emits `("portal", "state") -> (id, open)`.
- `is_portal_open(id)`: portals are open unless closed.
- A closed portal returns `PortalError::PortalClosed` (6) on travel.

## Topology

`get_portal_topology(start_id, limit) -> Vec<PortalLink>` returns one page,
with `limit` capped at `MAX_TOPOLOGY_PAGE` (50):

```rust
pub struct PortalLink {
    pub id: u64,
    pub source_nebula: u64,
    pub target_nebula: u64,
    pub stability: u32,   // live, after decay
    pub travel_cost: i128,
    pub open: bool,
    pub access: PortalAccessMode,
}
```

Page through with `start_id = 0, 50, 100, …` until `start_id >= get_portal_count()`.

## Access control

`PortalAccessMode`: `Public` (default), `OwnerOnly`, `Allowlist`.

| Function | Who |
| --- | --- |
| `set_portal_access(caller, id, mode)` | owner or admin |
| `set_portal_allowed(caller, id, traveler, allowed)` | owner or admin |
| `can_travel(traveler, &portal) -> bool` | read-only |
| `travel_through_portal_as(traveler, id)` | `traveler.require_auth()`, enforces the mode, emits `("portal", "travel") -> (traveler, id, cost)` |

The owner always passes `OwnerOnly` and `Allowlist`. The legacy anonymous
`travel_through_portal(id)` (still the contract entry point) only works for
`Public` portals. For anything else it returns `NotAuthorized`, so it cannot be
used to bypass a restriction.

Check order for travel: access, then closed, then stability. Travel never
charges anything itself. The calling contract charges the returned cost, as before.

## Metadata

`set_portal_metadata(owner, id, PortalMetadata { name, description, tags })`
is owner only. Limits: name 1–32 bytes, description ≤ 256 bytes, ≤ 8 tags,
otherwise `InvalidMetadata` (7). Read it with `get_portal_metadata(id) -> Option<PortalMetadata>`.

## Storage

New state lives under new `PortalKey` variants (`PortalsFromNebula`,
`PortalClosed`, `PortalAccess`, `PortalAllowed`, `PortalMeta`). The stored
`Portal` struct is unchanged, so existing portals keep decoding.

## UI

`PortalNetwork` takes topology links (merged with metadata), the current
nebula and the traveler. It shows:

- outgoing portals from the current nebula, with stability meter, cost,
  access mode and tags, and a Travel button that is disabled with a readable
  reason when travel is refused (mirrors the contract checks in `frontend/src/utils/portals.ts`),
- the full network table (route, stability, open or closed, access),
- Close and Reopen buttons on the viewer's own portals when `onToggleOpen` is passed.

## Complexity

- `register_portal`: O(P_n) to append to the source nebula's index (P_n = portals leaving it).
- `discover_portals`: O(P_n). `get_portal_topology`: O(limit), where limit ≤ 50.
- Access checks: O(1).
