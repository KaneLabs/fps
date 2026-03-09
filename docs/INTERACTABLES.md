# Replicated World Objects

All interactive world objects (doors, pickups, destructibles) are **server-spawned replicated entities**. The server owns the entity and its state. Clients receive the entity via lightyear replication and add rendering locally.

## The Pattern

### 1. Server spawns the entity

In server-only startup code, spawn the entity with:
- `Position`, `Rotation` (from Avian3D — these are registered for replication)
- Physics components: `RigidBody`, `Collider`, `Friction`, `Sensor` (server-only, not replicated)
- Your custom state component: `DoorState`, `Equippable`, `Interactable`, etc.
- `Replicate::to_clients(NetworkTarget::All)` — tells lightyear to send this entity to all clients
- **Do NOT use `InterpolationTarget`** for static/rarely-moving objects (see pitfalls below)

```rust
// In spawn_server_interactive_objects (server-only startup system)
commands.spawn((
    Position(Vec3::new(0.0, 2.0, -5.0)),
    Rotation::default(),
    RigidBody::Static,
    Collider::cuboid(3.0, 4.0, 0.3),
    DoorState { open: false },
    Name::new("Door"),
    Replicate::to_clients(NetworkTarget::All),
));
```

### 2. Register the component in protocol.rs

Every custom component that needs to sync must be registered. World object components are server-authoritative — no `.add_prediction()`.

```rust
app.register_component::<DoorState>();
app.register_component::<Equippable>();
```

### 3. Client adds rendering via `Added<T>` systems

Lightyear inserts all replicated components in a single batch when the entity arrives on the client. Use a system in the `Update` schedule with an `Added<YourComponent>` query filter to add rendering exactly once.

This is the pattern used by lightyear's own examples (`avian_3d_character`, `fps`).

```rust
// In client.rs — registered in Update schedule
pub fn init_replicated_doors(
    door_query: Query<
        (Entity, &DoorState, &Position, &Rotation),
        Added<DoorState>,
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, door_state, pos, rot) in door_query.iter() {
        commands.entity(entity).insert((
            Mesh3d(door_mesh),
            MeshMaterial3d(wood),
            Transform::from_translation(pos.0).with_rotation(rot.0),
            Visibility::default(),
            Collider::cuboid(3.0, 4.0, 0.3),  // client-side collider for shape casts
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        ));
    }
}
```

**Why `Added<T>` and not observers?** Lightyear adds the `Interpolated` marker to the entity *before* inserting the replicated components. An observer on `On<Add, (DoorState, Interpolated)>` fires too early — the components aren't available yet. `Added<T>` in a system runs in the next `Update` tick after insertion, when everything is present.

### 4. State changes replicate automatically

When the server mutates a replicated component (e.g. `door.open = true`), lightyear sends the update to all clients. Use `Changed<T>` systems on the client to react visually:

```rust
pub fn sync_door_state(
    mut door_query: Query<(Entity, &DoorState), Changed<DoorState>>,
    mut commands: Commands,
) {
    for (entity, door_state) in door_query.iter_mut() {
        if door_state.open {
            commands.entity(entity).remove::<Collider>().insert(Visibility::Hidden);
        }
    }
}
```

### 5. Shared observers for gameplay logic

Gameplay logic (door open, equip, mine) runs in shared observers that fire on both client and server via BEI input replay. Use `Has<Predicted>` to gate server-only side effects like despawns:

```rust
if !is_predicted {
    // Server only: despawn and spawn new replicated entity
    commands.entity(target).despawn();
}
```

## What goes where

| Concern | Where | Schedule |
|---------|-------|----------|
| Entity spawn (physics + state) | `world/mod.rs` `spawn_server_interactive_objects` | Server `Startup` |
| Component registration | `protocol.rs` `ProtocolPlugin::build()` | — |
| Client rendering init | `world/mod.rs` `init_replicated_*` | Client `Update` |
| Client visual sync | `world/mod.rs` `sync_*` | Client `Update` |
| Gameplay logic | `world/mod.rs` shared observers | Both (via BEI) |
| Server binary wiring | `server.rs` | `Startup` + observers |
| Client binary wiring | `client.rs` | `Update` + observers |

## Anti-Patterns

- **Using `InterpolationTarget` on entities with non-interpolatable components**: Lightyear inserts `Confirmed<C>` instead of `C` for interpolated entities. If the component doesn't have `.add_linear_interpolation()`, `C` is never derived from `Confirmed<C>`, so `Added<C>` never fires and `Query<&C>` finds nothing. Only use `InterpolationTarget` for entities whose components all have interpolation registered (like `Position`/`Rotation`). For static world objects, use `Replicate` alone.
- **Tuple observer triggers with `Interpolated`**: `On<Add, (T, Interpolated)>` fires before `T` is inserted.
- **Client-only collider changes**: If the server doesn't know, the player rubberbands.
- **Client mutates replicated component**: Server overwrites it on next replication tick.
- **Forgetting `register_component` in protocol.rs**: Component won't replicate even with `Replicate` on the entity.
