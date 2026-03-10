# Interpolation & Prediction Architecture

## The Rule
**Interpolated (remote) entities must NEVER run local physics.** They are visual puppets — their Position comes only from lightyear's server snapshot interpolation. Only predicted (local) entities run physics simulation.

## Why
The server already ran the authoritative physics. Re-running it locally on the interpolated entity with slightly different timing produces different results. Lightyear's interpolation system writes Position from confirmed server snapshots, and if `character_controller` also writes Position from local physics, the two fight each other — causing overshoot, oscillation, and jitter.

## Entity Types

| Entity | Who sees it | Systems that should run | Position source |
|--------|------------|------------------------|----------------|
| **Predicted** (local player) | Owning client | Full physics: `character_controller`, `clear_xz_velocity`, shared observers | Client physics (rollback-corrected by server) |
| **Interpolated** (remote player) | All other clients | None — just visual rendering | Lightyear interpolation between server snapshots |
| **Server** (authoritative) | Server only | Full physics: `character_controller`, `clear_xz_velocity`, shared observers | Server physics (ground truth) |

## Implementation

Any shared system that writes to physics state (Position, CharacterVelocity, etc.) must filter out interpolated entities:

```rust
// Use With<Predicted> or Without<Interpolated> on queries
fn character_controller(
    query: Query<..., (With<PlayerContext>, With<Collider>, Without<Interpolated>)>,
)
```

Interpolated entities still need their Collider for raycast hit detection (shooting), but must not run through the movement/physics pipeline.

## How Industry FPS Games Do It

- **Source Engine (CS, Valorant, TF2)**: Remote players use pure entity interpolation — ~100ms buffer of server snapshots, smooth lerp between them. No local physics. What you see is slightly in the past but always smooth.
- **Overwatch**: Same — remote players are "ghosts" interpolating between server snapshots. 100ms buffer with adaptive delay based on jitter. Physics only runs on owned entities.
- **Universal pattern**: Physics for your entity, pure interpolation for everyone else.

## Lightyear Interpolation Config

Lightyear's `add_linear_interpolation()` uses an adaptive delay:
- `min_delay`: 5ms (default)
- `send_interval_ratio`: 1.7x server send interval (default)
- Actual delay = `max(send_interval * ratio, min_delay)` plus jitter margin

The `enable_correction()` flag applies only to **predicted** entities (smooths rollback corrections on Transform). It has no effect on interpolated entities.

## Common Mistakes

1. **Running shared movement observers on interpolated entities** — `rebroadcast_inputs: true` replays inputs for all entities. Gate observers with `Has<Predicted>` checks if they write physics state.
2. **Running `character_controller` on all `PlayerContext` entities** — must exclude `Interpolated`.
3. **Writing to Transform directly on interpolated entities** — lightyear's `PositionButInterpolateTransform` mode handles Position→Transform sync. Writing Transform directly gets overwritten.
