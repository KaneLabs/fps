//! Startup/build-time validation of world geometry.
//!
//! Two classes of bug shipped to production undetected before this existed, and
//! neither was cosmetic:
//!
//! 1. SPAWN POINTS INSIDE COLLIDERS. Two of eight spawn points were buried in
//!    boulders. The consequence is not "player looks wrong" — every controller
//!    shape cast uses `ignore_origin_penetration: true`, so a capsule spawned
//!    inside geometry is BLIND to it. It reads as airborne, sinks through the
//!    world, trips the kill plane, respawns at the same point, and loops
//!    forever. It was only reachable with 2+ players, because
//!    `select_spawn_point` picks furthest-from-living-players and solo play
//!    randomises across all eight.
//!
//! 2. TERRAIN INTRUDING INTO INTERIOR VOLUMES. A hillside slab reached into the
//!    mine tunnel and filled part of it solid. The comment beside that slab
//!    already stated the correct intent; only the numbers were wrong — which is
//!    exactly the case a comment cannot catch and a test can.
//!
//! This runs as a plain unit test (no Bevy app, no Avian) because
//! `world::world_blocks()` exposes the collider set as data. It also runs on
//! server startup as defence in depth: a server that refuses to boot beats one
//! that swallows players.

use bevy::prelude::*;

use crate::player::{CAPSULE_HEIGHT, CAPSULE_RADIUS, SPAWN_POINTS};
use crate::world::{Aabb, BlockKind, WorldBlock};

/// Total capsule height: cylinder length plus both hemispherical caps.
pub const CAPSULE_TOTAL_HEIGHT: f32 = CAPSULE_HEIGHT + 2.0 * CAPSULE_RADIUS;

/// Float-noise tolerance for the spawn penetration test.
///
/// Deliberately tiny, and NOT a safety margin. A capsule standing correctly on
/// a surface touches it: the distance from its spine to that surface is exactly
/// CAPSULE_RADIUS. Demanding any clearance beyond the radius therefore flags
/// every correctly-placed spawn as a violation — the first version of this file
/// did exactly that and reported the main ground plane as a defect. Only actual
/// PENETRATION is a bug; 1mm absorbs rounding without hiding anything real.
pub const PENETRATION_TOLERANCE: f32 = 0.001;

/// An enclosed space players must be able to occupy.
///
/// Declared rather than derived. Deriving the air pocket from surrounding walls
/// sounds tidier but fails silently when a wall moves — the derived volume moves
/// with it and the check still passes. A declared volume is a statement of
/// INTENT that geometry is checked against, which is the direction that catches
/// the wall moving.
#[derive(Debug, Clone, Copy)]
pub struct InteriorVolume {
    pub name: &'static str,
    pub aabb: Aabb,
}

/// Interior volumes that terrain must never intrude into.
///
/// Each is the open air pocket INSIDE a structure, inset to the inner faces of
/// that structure's own walls, so the walls themselves do not register (overlap
/// is strict — touching does not count).
pub fn interior_volumes() -> Vec<InteriorVolume> {
    vec![
        InteriorVolume {
            // Floor  pos(22, 0.8, -6)  size(3, 0.1, 8)  -> top y=0.85, x 20.5..23.5, z -10..-2
            // Walls  x=20.5 / x=23.5   size(0.4, ..)    -> inner faces x 20.7 / 23.3
            // Ceiling pos(22, 3.65, -6) size(3, 0.3, 8) -> underside y=3.50
            // (raised in 639ebaf along with the crossbeams to open the tunnel)
            name: "mine tunnel",
            aabb: Aabb {
                min: Vec3::new(20.7, 0.85, -10.0),
                max: Vec3::new(23.3, 3.50, -2.0),
            },
        },
        InteriorVolume {
            // Floor pos(0, 0.3, 0) size(8, 0.2, 6) -> top y=0.4
            // Walls x=+-4 size(0.4,..) -> inner faces x -3.8 / 3.8
            // North/south walls at z=-3 / z=3, 0.4 thick -> inner faces -2.8 / 2.8
            // Roof pos(0, 3.3, 0) size(9, 0.2, 7) -> underside y=3.2
            name: "cabin",
            aabb: Aabb {
                min: Vec3::new(-3.8, 0.4, -2.8),
                max: Vec3::new(3.8, 3.2, 2.8),
            },
        },
        InteriorVolume {
            // Floor pos(-14, 0.15, 2) size(5, 0.15, 4) -> top y=0.225, x -16.5..-11.5
            // West wall x=-16.5 size(0.3,..) -> inner face -16.35
            // North wall z=0 (rotated) 0.3 thick -> inner face z=0.15
            // Roof pos(-14, 2.5, 2) size(6, 0.1, 5) -> underside y=2.45
            // East side is open posts; south has a gap. Volume stops at the
            // floor's own extent rather than guessing where "outside" begins.
            name: "equipment shed",
            aabb: Aabb {
                min: Vec3::new(-16.35, 0.225, 0.15),
                max: Vec3::new(-11.65, 2.45, 4.0),
            },
        },
    ]
}


/// Defects that already existed when this validation was written.
///
/// THIS IS NOT AN EXCUSE LIST. Every entry is a real defect, found the moment
/// the checker first ran against master, triaged and escalated rather than
/// silently fixed — changing spawn placement and terrain is a gameplay change
/// and Ryan was mid-playtest. They are recorded so the mechanism can ship green
/// and start catching NEW breakage immediately, which is worth more than
/// blocking on a geometry pass.
///
/// The test asserts the current violation set EXACTLY equals this list, so it
/// cannot rot in either direction:
///   - a NEW violation fails the build (the point of the whole module)
///   - FIXING one also fails, telling you to delete its entry
/// An allowlist that silently absorbs both is how a guard stops covering its
/// case while still looking green.
pub const KNOWN_VIOLATIONS: &[&str] = &[
    // Shed spawn sits 2cm into the shed floor, and 40cm inside the workbench.
    "spawn:1:block(-14.00,0.15,2.00)",
    "spawn:1:block(-15.00,0.40,1.50)",
    // (spawn 2, mine entrance 50cm inside the hillside, was here — FIXED in
    //  8d95b79. The baseline's exact-match assertion detected the fix and
    //  refused to pass until this entry was deleted, which is the anti-rot
    //  direction working: an allowlist that silently absorbs a fix is how a
    //  guard stops covering its case while still looking green.)
    // Watchtower spawn 10cm into its own platform.
    "spawn:3:block(-7.50,3.80,-7.50)",
    // Truck spawn inside the truck bed and cab.
    "spawn:7:block(10.00,0.60,3.00)",
    "spawn:7:block(10.00,1.50,1.50)",
    // Hillside roofs the tunnel floor: its top (y=1.0) sits 15cm above the
    // tunnel's own floor (y=0.85) along the full 8m run, so players walk on the
    // hillside and headroom to the ceiling is 2.05m against a 2.0m capsule.
    // The comment beside this block says it "stops before mine tunnel entrance";
    // the numbers span straight through it.
    "interior:mine tunnel:block(18.00,0.50,-8.00)",
    // Western terrain reaches 38cm above the shed floor across its west end.
    "interior:equipment shed:block(-20.00,0.30,-10.00)",
];

/// A geometry defect found by validation.
#[derive(Debug, Clone)]
pub enum Violation {
    /// A spawn point's capsule intersects (or nearly intersects) a collider.
    SpawnBlocked {
        index: usize,
        spawn: Vec3,
        block: WorldBlock,
            /// How far the capsule body is inside the block, in metres.
        /// SATURATES at CAPSULE_RADIUS — see the note in the boulder test.
        penetration: f32,
    },
    /// A terrain block reaches into a space players must be able to occupy.
    InteriorIntruded {
        volume: &'static str,
        block: WorldBlock,
        overlap: Vec3,
    },
}

impl Violation {

    /// Stable identity for baselining. Derived from the spawn index / volume
    /// name and the offending block's centre — stable across formatting changes
    /// but not across the geometry actually moving, which is what we want.
    pub fn key(&self) -> String {
        match self {
            Violation::SpawnBlocked { index, block, .. } => format!(
                "spawn:{index}:block({:.2},{:.2},{:.2})",
                block.pos.x, block.pos.y, block.pos.z
            ),
            Violation::InteriorIntruded { volume, block, .. } => format!(
                "interior:{volume}:block({:.2},{:.2},{:.2})",
                block.pos.x, block.pos.y, block.pos.z
            ),
        }
    }

    /// CHOOSE-not-STOP: state the consequence and the ways out, not the rule.
    /// A check that only ever means STOP gets commented out; one that explains
    /// what breaks and how to fix it gets used.
    pub fn describe(&self) -> String {
        match self {
            Violation::SpawnBlocked { index, spawn, block, penetration } => {
                let b = block.aabb();
                format!(
                    "SPAWN POINT {index} AT {spawn:?} IS INSIDE WORLD GEOMETRY.\n\
                     \n\
                     It overlaps a {:?} block centred {:?} with full extents {:?} \
                     (spans {:?} to {:?}) by {penetration:.2}m.\n\
                     \n\
                     CONSEQUENCE: every controller shape cast sets \
                     ignore_origin_penetration, so a capsule spawned inside a \
                     collider is BLIND to it. The player reads as airborne, sinks \
                     through the world, hits the kill plane, respawns at this same \
                     point, and loops forever — they cannot play until the server \
                     restarts. It is also easy to miss in testing: select_spawn_point \
                     picks furthest-from-living-players, so solo play often never \
                     selects it and it only appears with 2+ players.\n\
                     \n\
                     FIX EITHER WAY:\n\
                     1. Raise the spawn. The capsule is {CAPSULE_TOTAL_HEIGHT}m tall, \
                        so its CENTRE must sit at least {:.2}m above the surface \
                        it stands on.\n\
                     2. Move the spawn off this geometry, or move the geometry.",
                    block.kind, block.pos, block.size, b.min, b.max,
                    CAPSULE_TOTAL_HEIGHT * 0.5,
                )
            }
            Violation::InteriorIntruded { volume, block, overlap } => {
                let b = block.aabb();
                format!(
                    "TERRAIN INTRUDES INTO THE {} INTERIOR.\n\
                     \n\
                     A {:?} block centred {:?} with full extents {:?} (spans {:?} \
                     to {:?}) fills {:.2} x {:.2} x {:.2}m of a space players are \
                     meant to walk through.\n\
                     \n\
                     CONSEQUENCE: the volume is solid where it should be open. \
                     Depending on how much is filled this walls players out of a \
                     room, entombs whatever was in there, or leaves a lip too tall \
                     to step over. None of it is visible from the code — the \
                     comment beside the block may well describe the correct intent \
                     while the numbers do something else.\n\
                     \n\
                     FIX EITHER WAY:\n\
                     1. Move or shrink the block so it stops at the structure's \
                        outer face.\n\
                     2. If the intrusion is deliberate, change the declared volume \
                        in world::validation::interior_volumes and say why — the \
                        volume is a statement of intent, so intent is what should \
                        change.\n\
                     \n\
                     NOTE: `Collider::cuboid` takes FULL extents and halves them \
                     internally, so this block reaches {:.2}m either side of its \
                     centre on z, not {:.2}m. Reading size as half-extents is the \
                     easiest way to place a block twice as far as intended.",
                    volume.to_uppercase(), block.kind, block.pos, block.size,
                    b.min, b.max, overlap.x, overlap.y, overlap.z,
                    block.size.z * 0.5, block.size.z,
                )
            }
        }
    }
}

/// Distance from a point to an axis-aligned box centred at the origin.
fn point_to_box_distance(p: Vec3, half: Vec3) -> f32 {
    (p.abs() - half).max(Vec3::ZERO).length()
}

/// How deep a vertical capsule centred at `center` penetrates `block`, or None.
///
/// Works in the block's local frame so rotated blocks are handled exactly
/// rather than via their (over-approximating) world AABB.
///
/// The capsule's spine is sampled rather than solved analytically. With a 1.0m
/// spine and 64 samples the spacing is ~1.6cm, so the reported depth can be
/// under-stated by at most that — far below SPAWN_CLEARANCE, and the check is
/// for gross embedding, not sub-centimetre contact.
pub fn capsule_penetration(center: Vec3, block: &WorldBlock) -> Option<f32> {
    let half = block.size * 0.5;
    let inv = block.rot.inverse();
    // Spine endpoints: the capsule is CAPSULE_HEIGHT long between cap centres.
    let spine_half = CAPSULE_HEIGHT * 0.5;
    const SAMPLES: usize = 64;

    let mut min_distance = f32::INFINITY;
    for i in 0..=SAMPLES {
        let t = i as f32 / SAMPLES as f32;
        let y = -spine_half + t * (2.0 * spine_half);
        let world = center + Vec3::new(0.0, y, 0.0);
        let local = inv * (world - block.pos);
        min_distance = min_distance.min(point_to_box_distance(local, half));
    }

    // Resting ON a surface gives min_distance == CAPSULE_RADIUS and is correct.
    // Only a spine closer than the radius means the capsule body is inside.
    let threshold = CAPSULE_RADIUS - PENETRATION_TOLERANCE;
    (min_distance < threshold).then_some(CAPSULE_RADIUS - min_distance)
}

/// Validate the whole map. Empty result means the geometry is sound.
pub fn validate_world(blocks: &[WorldBlock]) -> Vec<Violation> {
    let mut violations = Vec::new();

    // 1. No spawn point may put a player capsule inside geometry.
    for (index, spawn) in SPAWN_POINTS.iter().enumerate() {
        for block in blocks {
            if let Some(penetration) = capsule_penetration(*spawn, block) {
                violations.push(Violation::SpawnBlocked {
                    index,
                    spawn: *spawn,
                    block: *block,
                    penetration,
                });
            }
        }
    }

    // 2. No TERRAIN block may reach into a declared interior volume.
    //    Structures bound their own interiors and props legitimately sit inside
    //    them, so only landscape participates.
    for volume in interior_volumes() {
        for block in blocks.iter().filter(|b| b.kind == BlockKind::Terrain) {
            if let Some(overlap) = block.aabb().overlap_extent(&volume.aabb) {
                violations.push(Violation::InteriorIntruded {
                    volume: volume.name,
                    block: *block,
                    overlap,
                });
            }
        }
    }

    violations
}

/// Violations that are NOT in the recorded baseline — i.e. newly introduced.
pub fn unknown_violations(blocks: &[WorldBlock]) -> Vec<Violation> {
    validate_world(blocks)
        .into_iter()
        .filter(|v| !KNOWN_VIOLATIONS.contains(&v.key().as_str()))
        .collect()
}

/// Server startup check. Panics only on defects NOT already in the baseline.
///
/// Defence in depth behind the unit test: the test catches a bad commit in CI,
/// this catches anything that reaches a running server anyway. A server that
/// refuses to boot is loud and recoverable; one that boots and swallows players
/// into a respawn loop is neither.
///
/// It deliberately does NOT panic on the recorded baseline. Panicking on
/// pre-existing defects would mean this module could not be deployed at all
/// without first completing a full geometry pass — the check would be reverted
/// within the hour and nothing would be guarded. Known defects are logged at
/// WARN every boot so they stay visible rather than becoming invisible.
pub fn assert_world_valid_on_startup() {
    let blocks = crate::world::world_blocks();
    let all = validate_world(&blocks);
    let (known, unknown): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|v| KNOWN_VIOLATIONS.contains(&v.key().as_str()));

    for v in &known {
        warn!("[WORLD] known geometry defect (baselined, not yet fixed): {}", v.key());
    }

    if unknown.is_empty() {
        info!(
            "[WORLD] geometry validated: {} spawn points and {} interior volumes checked \
             against {} blocks; {} known defect(s) outstanding",
            SPAWN_POINTS.len(),
            interior_volumes().len(),
            blocks.len(),
            known.len(),
        );
        return;
    }

    for v in &unknown {
        error!("[WORLD] {}", v.describe());
    }
    panic!(
        "world geometry validation failed with {} NEW violation(s) — refusing to start. \
         See the [WORLD] errors above.",
        unknown.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::world_blocks;
    use std::collections::BTreeSet;

    /// THE check. Runs in CI on every commit.
    ///
    /// Exact-match against the baseline in both directions — see KNOWN_VIOLATIONS.
    #[test]
    fn world_geometry_matches_known_state() {
        let found: BTreeSet<String> =
            validate_world(&world_blocks()).iter().map(|v| v.key()).collect();
        let known: BTreeSet<String> =
            KNOWN_VIOLATIONS.iter().map(|s| s.to_string()).collect();

        let new: Vec<_> = found.difference(&known).cloned().collect();
        let fixed: Vec<_> = known.difference(&found).cloned().collect();

        if !new.is_empty() {
            let detail = validate_world(&world_blocks())
                .into_iter()
                .filter(|v| new.contains(&v.key()))
                .map(|v| v.describe())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n");
            panic!("NEW WORLD GEOMETRY DEFECT(S):\n\n{detail}");
        }
        assert!(
            fixed.is_empty(),
            "These known defects appear to be FIXED — delete them from \
             KNOWN_VIOLATIONS so the baseline cannot rot into an allowlist \
             that hides the next one:\n  {}",
            fixed.join("\n  ")
        );
    }

    /// The startup check must NOT panic on today's map. Without this, deploying
    /// this module would have crash-looped the live server on boot: every known
    /// defect is still present, and the first version panicked on any violation.
    #[test]
    fn startup_check_passes_on_current_map() {
        assert!(
            unknown_violations(&world_blocks()).is_empty(),
            "startup validation would panic and the server would not boot"
        );
    }

    /// Proves the checker has teeth: a guard nobody has seen fail is
    /// indistinguishable from one wired to `true`. Reconstructs the exact bug
    /// shipped to production — a spawn point buried in the NW boulder cluster.
    #[test]
    fn detects_the_boulder_spawn_bug_that_shipped() {
        // Boulder as authored: pos(-10, 0.7, -15), full extents (3, 1.4, 2.5)
        // -> top surface y = 1.4. The old spawn sat at y=1.5, so the capsule
        // bottom (1.5 - 1.0 = 0.5) was 0.9m inside the rock.
        let boulder = WorldBlock {
            pos: Vec3::new(-10.0, 0.7, -15.0),
            size: Vec3::new(3.0, 1.4, 2.5),
            rot: Quat::IDENTITY,
            friction: 0.7,
            kind: BlockKind::Terrain,
        };
        let old_spawn = Vec3::new(-10.0, 1.5, -15.0);
        let penetration = capsule_penetration(old_spawn, &boulder)
            .expect("the shipped bug must be detected");
        // The reported depth SATURATES at CAPSULE_RADIUS: once the spine itself
        // is inside the collider the spine-to-surface distance is 0 and there is
        // nothing further to measure. Saturation therefore means "the capsule
        // axis is inside the geometry", which is the maximally-bad case — the
        // number is a severity floor, not the true depth.
        assert!(
            (penetration - CAPSULE_RADIUS).abs() < 1e-4,
            "expected fully-embedded (saturated) reading, got {penetration}"
        );

        // And the shipped fix must pass.
        let fixed_spawn = Vec3::new(-10.0, 3.3, -15.0);
        assert!(capsule_penetration(fixed_spawn, &boulder).is_none());
    }

    /// A capsule standing correctly ON a surface must NOT be a violation.
    /// The first version of this module failed this and reported the main
    /// ground plane as a defect.
    #[test]
    fn resting_on_a_surface_is_not_penetration() {
        let ground = WorldBlock {
            pos: Vec3::new(0.0, -0.05, 0.0),
            size: Vec3::new(120.0, 0.1, 120.0),
            rot: Quat::IDENTITY,
            friction: 0.5,
            kind: BlockKind::Terrain,
        };
        // Ground top is y=0; a 2m capsule resting on it has its centre at y=1.
        assert!(capsule_penetration(Vec3::new(0.0, 1.0, 0.0), &ground).is_none());
        // One centimetre lower is genuine penetration.
        assert!(capsule_penetration(Vec3::new(0.0, 0.99, 0.0), &ground).is_some());
    }

    /// Extents are FULL lengths. Reading them as half-extents doubles every box
    /// and is the single easiest way to make this module lie.
    #[test]
    fn extents_are_full_not_half() {
        let b = WorldBlock {
            pos: Vec3::new(0.0, 0.0, 0.0),
            size: Vec3::new(10.0, 2.0, 6.0),
            rot: Quat::IDENTITY,
            friction: 0.5,
            kind: BlockKind::Terrain,
        };
        let a = b.aabb();
        assert_eq!(a.min, Vec3::new(-5.0, -1.0, -3.0));
        assert_eq!(a.max, Vec3::new(5.0, 1.0, 3.0));
    }

    /// A wall flush against the edge of the room it encloses must not register
    /// as intruding into it.
    #[test]
    fn touching_is_not_overlapping() {
        let room = Aabb { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(4.0, 3.0, 4.0) };
        let flush = Aabb { min: Vec3::new(-0.4, 0.0, 0.0), max: Vec3::new(0.0, 3.0, 4.0) };
        assert!(!flush.overlaps(&room));
        let inside = Aabb { min: Vec3::new(-0.4, 0.0, 0.0), max: Vec3::new(0.1, 3.0, 4.0) };
        assert!(inside.overlaps(&room));
    }

    /// Every declared volume must be non-degenerate — a volume with an inverted
    /// or zero axis silently checks nothing.
    #[test]
    fn declared_volumes_are_non_degenerate() {
        for v in interior_volumes() {
            let size = v.aabb.max - v.aabb.min;
            assert!(
                size.x > 0.0 && size.y > 0.0 && size.z > 0.0,
                "interior volume '{}' is degenerate ({size:?}) and would check nothing",
                v.name
            );
            assert!(
                size.y >= CAPSULE_TOTAL_HEIGHT,
                "interior volume '{}' is only {:.2}m tall — a {CAPSULE_TOTAL_HEIGHT}m \
                 player cannot stand in it, so either the volume is wrong or the \
                 structure is unusable",
                v.name, size.y
            );
        }
    }
}

#[cfg(test)]
mod tunnel_diagnosis {
    use super::*;
    use crate::world::world_blocks;

    /// Diagnostic: what actually occupies the mine tunnel, and is its mouth sealed?
    /// Run with: cargo test --lib tunnel_report -- --nocapture --ignored
    #[test]
    #[ignore = "diagnostic, not a check"]
    fn tunnel_report() {
        let blocks = world_blocks();
        let tunnel = interior_volumes()
            .into_iter()
            .find(|v| v.name == "mine tunnel")
            .unwrap();

        println!("\n=== TUNNEL INTERIOR {:?} .. {:?} ===", tunnel.aabb.min, tunnel.aabb.max);
        println!("--- ALL blocks overlapping the interior (any kind) ---");
        let mut any = false;
        for b in &blocks {
            if let Some(o) = b.aabb().overlap_extent(&tunnel.aabb) {
                any = true;
                println!(
                    "  {:?} pos={:?} size={:?} spans {:?}..{:?}  fills {:.2}x{:.2}x{:.2}m",
                    b.kind, b.pos, b.size, b.aabb().min, b.aabb().max, o.x, o.y, o.z
                );
            }
        }
        if !any { println!("  (none)"); }

        // Mouth: the approach just OUTSIDE the interior, at walking height.
        // A block sealing this would close the tunnel without ever entering the
        // declared volume — which is why the interior check alone cannot see it.
        let mouth = Aabb {
            min: Vec3::new(tunnel.aabb.min.x, tunnel.aabb.min.y, tunnel.aabb.max.z),
            max: Vec3::new(tunnel.aabb.max.x, tunnel.aabb.max.y, tunnel.aabb.max.z + 4.0),
        };
        println!("\n--- ALL blocks overlapping the MOUTH {:?}..{:?} ---", mouth.min, mouth.max);
        let mut any_mouth = false;
        for b in &blocks {
            if let Some(o) = b.aabb().overlap_extent(&mouth) {
                any_mouth = true;
                println!(
                    "  {:?} pos={:?} size={:?} spans {:?}..{:?}  fills {:.2}x{:.2}x{:.2}m",
                    b.kind, b.pos, b.size, b.aabb().min, b.aabb().max, o.x, o.y, o.z
                );
            }
        }
        if !any_mouth { println!("  (none)"); }

        // Headroom actually available to a 2m capsule along the tunnel.
        let floor_top = blocks.iter()
            .filter(|b| b.aabb().overlap_extent(&tunnel.aabb).is_some())
            .map(|b| b.aabb().max.y)
            .fold(tunnel.aabb.min.y, f32::max);
        println!(
            "\nEffective floor y={:.2}, ceiling y={:.2} -> headroom {:.2}m (capsule needs {CAPSULE_TOTAL_HEIGHT}m)",
            floor_top, tunnel.aabb.max.y, tunnel.aabb.max.y - floor_top
        );
    }
}
