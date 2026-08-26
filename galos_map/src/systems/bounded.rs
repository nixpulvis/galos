//! Drawing only the systems the walk marks, off the index's own payloads
//!
//! An alternative source of star entities to the spyglass region fetch. The
//! spyglass reads a sphere and spawns every system in it; this reads the cells
//! the walk marks (`Planned::marks`) and spawns one entity per system in their
//! payloads. The walk spends a point budget, so a zoom out draws a bounded set
//! of marks with everything coarser summed into splats, rather than the
//! million entities the transform walk would then pay for every frame.
//!
//! On by default, behind [`LodFetch`]. While it is on the spyglass region
//! fetch and its eviction stand down through their run conditions and this
//! takes their place; turned off, the spyglass drives the map as it once did.
//! Only one source of systems runs at a time. The spyglass radius lives on as
//! an optional clamp on the walk — see [`reach`].
//!
//! It owns no drawing of its own: a built system is pushed onto the same
//! [`PendingSpawns`] queue the spyglass fills and turned into an entity by
//! [`super::spawn::drain_spawns`], and an evicted one onto [`PendingEvictions`]
//! for [`super::drain_evictions`]. The rest of the map — visibility, sizing,
//! pointing, selection, labels — reads a [`System`] without caring which
//! source spawned it.

use crate::schedule::MapSet;
use crate::systems::aggregate::Planned;
use crate::systems::fetch::{FetchTasks, RawSystem};
use crate::systems::bodies::spawn::HeldSystem;
use crate::systems::spawn::{PendingSpawns, build_system};
use crate::systems::{PendingEvictions, Spyglass, System};
use crate::{Names, Populated, ResidentIndex, Transport};
use crate::camera::OrbitCamera;
use bevy::prelude::*;
use bevy::math::DVec3;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::{CellId, Point, Resident, resolvable_count};
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<LodFetch>();
    app.init_resource::<ResidentCells>();
    app.init_resource::<BoundedTasks>();

    // Clears the map when the source is switched, before either source runs,
    // so the two never overlap on screen.
    app.add_systems(Update, switch.in_set(MapSet::Search));
    app.add_systems(Update, fetch.in_set(MapSet::Fetch).run_if(enabled));
    // Arrived payloads land in the cache; the draw reads them from there.
    app.add_systems(Update, collect.in_set(MapSet::Populate).run_if(enabled));
    // Then draw each resident cell's resolvable prefix — grown and shed per
    // system with distance — and drop whatever falls outside every prefix.
    app.add_systems(
        Update,
        reconcile.in_set(MapSet::Populate).after(collect).run_if(enabled),
    );
    // Free the payloads of cells the walk no longer wants at all.
    app.add_systems(
        Update,
        evict_payloads.in_set(MapSet::Present).run_if(enabled),
    );
}

/// Whether the walk's level-of-detail fetch drives the map
///
/// On by default: the walk — clamped to the spyglass reach when the bound is
/// on — is the map's source. Turned off, the old spyglass region fetch drives
/// it instead, until that path is retired (see the TODO on
/// `fetch::fetch_spyglass`).
#[derive(Resource)]
pub struct LodFetch(pub bool);

impl Default for LodFetch {
    fn default() -> Self {
        LodFetch(true)
    }
}

/// Whether the bounded source is on, for the systems it drives to run under.
pub(crate) fn enabled(bounded: Res<LodFetch>) -> bool {
    bounded.0
}

/// Whether the spyglass source should run, which is whenever the bounded one
/// is not.
pub(crate) fn spyglass(bounded: Res<LodFetch>) -> bool {
    !bounded.0
}

/// The spyglass reach as a clamp on the walk, in light years, or `None` when
/// the bound is off and the walk runs to the whole sky.
///
/// Under the walk the spyglass is a clamp, not a source: it never changes the
/// LOD, only where the LOD is cut off. A correct walk draws the same systems
/// inside the bubble whether the clamp is on or off — the clamp only sheds the
/// far, faint tail the walk would otherwise resolve across the whole
/// separation sphere, which is what a dense near view pays for. Off is the
/// whole sky, thinned by resolvability alone. The bound is the spyglass's
/// `clear`: to bound the view is to clear away what the reach does not hold.
fn reach(spyglass: &Spyglass) -> Option<f64> {
    spyglass.clear.then_some(spyglass.radius as f64)
}

/// Whether a cell's box comes within `radius` of `center`, measured to its
/// nearest point so a cell straddling the edge is kept and its own points
/// filtered by their distance — the same nearest-point test the region fetch
/// used to gather a sphere off the cell grid.
fn cell_in_reach(id: CellId, center: DVec3, radius: f64) -> bool {
    id.bounds().distance_to(center.to_array()) <= radius
}

/// The cell payloads the map holds, the resident half of the walk's predicate
///
/// Keyed by cell, so [`Resident::missing`] is the marks a fetch must load and
/// [`Resident::stale`] the held cells the walk no longer asks for.
#[derive(Resource, Default)]
struct ResidentCells(Resident);

/// The payload reads in flight, one per marks cell not yet resident or asked
#[derive(Resource, Default)]
struct BoundedTasks(HashMap<CellId, Task<io::Result<Vec<Point>>>>);

/// Clear the map when the source switches, so one does not draw over the other
///
/// Both sources spawn [`System`] entities and neither evicts the other's, so a
/// flip would otherwise leave the old set standing. On the frame the switch
/// changes, every system is queued for eviction and both sources' memory is
/// reset, so whichever is now on rebuilds from nothing.
///
/// TODO(bounded): the whole-map clear is here only because both sources can
/// spawn at once behind the toggle. When the spyglass path is retired and the
/// toggle with it, there is one source and nothing to clear between — drop
/// this system then.
fn switch(
    bounded: Res<LodFetch>,
    systems: Query<Entity, With<System>>,
    mut evictions: ResMut<PendingEvictions>,
    mut resident: ResMut<ResidentCells>,
    mut tasks: ResMut<BoundedTasks>,
    mut fetched: ResMut<FetchTasks>,
    mut last: Local<Option<bool>>,
) {
    // Not `is_changed`: the settings checkbox takes `&mut` of this every frame
    // it is drawn, which marks the resource changed whether or not the value
    // moved. Only a real flip should clear the map, so the value is compared
    // against the last one seen.
    if *last == Some(bounded.0) {
        return;
    }
    *last = Some(bounded.0);
    for entity in &systems {
        evictions.0.insert(entity);
    }
    resident.0 = Resident::default();
    tasks.0.clear();
    fetched.fetched.clear();
    fetched.surveyed.clear();
}

/// Ask for the payloads of the marks cells the map does not hold yet
///
/// Only the cells not already resident or already on the wire, so a still view
/// whose marks are all held asks for nothing and a zoom asks only for the
/// annulus it newly reaches. Run every frame rather than on a plan change: a
/// switch turning this source on holds a still camera whose plan has not
/// moved, and its marks must still be asked for.
fn fetch(
    planned: Res<Planned>,
    resident: Res<ResidentCells>,
    transport: Res<Transport>,
    spyglass: Res<Spyglass>,
    cameras: Query<&OrbitCamera>,
    mut tasks: ResMut<BoundedTasks>,
) {
    let bubble = reach(&spyglass).zip(cameras.single().ok());
    let pool = AsyncComputeTaskPool::get();
    for id in resident.0.missing(&planned.0) {
        // Past the clamp, a marks cell beyond the reach is left unfetched, so a
        // zoom out never loads the far sky the walk still marks — only its
        // nearer, brighter tail is drawn.
        if let Some((radius, camera)) = bubble
            && !cell_in_reach(id, camera.center, radius)
        {
            continue;
        }
        if tasks.0.contains_key(&id) {
            continue;
        }
        let source = transport.0.clone();
        tasks
            .0
            .insert(id, pool.spawn(async move { source.payload(id).await }));
    }
}

/// Take the payloads that have arrived into the resident cache
///
/// The transport half only: a payload lands keyed by its cell and the draw
/// reads it from there. Reading it is [`reconcile`]'s, run straight after, so a
/// cell's systems are chosen from what is now held.
fn collect(
    mut tasks: ResMut<BoundedTasks>,
    mut resident: ResMut<ResidentCells>,
) {
    tasks.0.retain(|&id, task| {
        let Some(result) = block_on(future::poll_once(task)) else {
            return true;
        };
        if let Ok(points) = result {
            resident.0.insert(id, points);
        }
        false
    });
}

/// Draw each resident cell's resolvable prefix, grown and shed per system as
/// the camera moves
///
/// A cell's payload is magnitude-ordered, and [`resolvable_count`] says how many
/// of its systems separate on screen from where the eye stands. Drawing that
/// prefix — and only it — is what lets a cell fill in and empty one system at a
/// time rather than switching on whole: a single system is drawn wherever it is
/// resolvable, so the index's cell boundaries stop showing through.
///
/// The prefix is pushed to the shared spawn queue, which builds only the
/// systems not already on the map, and everything outside every cell's prefix
/// is queued to drop. The held system is spared, since the camera's
/// FloatingOrigin hangs under it.
fn reconcile(
    cameras: Query<(&OrbitCamera, &Camera)>,
    index: Res<ResidentIndex>,
    resident: Res<ResidentCells>,
    populated: Res<Populated>,
    names: Res<Names>,
    holding: Res<HeldSystem>,
    spyglass: Res<Spyglass>,
    systems: Query<(Entity, &System)>,
    mut pending: ResMut<PendingSpawns>,
    mut evictions: ResMut<PendingEvictions>,
) {
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = crate::systems::aggregate::view(orbit, camera) else {
        return;
    };
    let now = Instant::now();
    // Clearing, the spyglass clamps the drawn set to a bubble about the camera:
    // the LOD is untouched inside it, only the far tail is shed.
    let bubble = reach(&spyglass);

    let existing: HashSet<i64> =
        systems.iter().map(|(_, system)| system.address).collect();

    // The resolvable prefix of every resident cell: the systems close enough to
    // separate. Build only the ones not already drawn; note every one wanted.
    let mut wanted: HashSet<i64> = HashSet::new();
    for (id, cell) in resident.0.iter() {
        let Some(indexed) = index.0.get(id) else { continue };
        if let Some(radius) = bubble
            && !cell_in_reach(id, orbit.center, radius)
        {
            continue;
        }
        let target =
            (resolvable_count(indexed, &view) as usize).min(cell.points.len());
        for point in &cell.points[..target] {
            // A cell straddling the bubble draws only the points inside it, so
            // the edge is a sphere about the camera, not the cell grid.
            if let Some(radius) = bubble
                && orbit.center.distance(DVec3::from(point.pos)) > radius
            {
                continue;
            }
            let address = point.id64 as i64;
            wanted.insert(address);
            if !existing.contains(&address) {
                pending.push(
                    build_from_point(point, &populated, &names),
                    false,
                    now,
                );
            }
        }
    }

    // Everything outside every prefix goes: the tail a cell sheds as it
    // recedes, the systems of a cell whose payload has been freed, and —
    // clearing — whatever fell outside the bubble above.
    for (entity, system) in &systems {
        if Some(entity) == holding.of() {
            continue;
        }
        if !wanted.contains(&system.address) {
            evictions.0.insert(entity);
        }
    }
}

/// Free the payloads of cells the walk no longer wants
///
/// [`Resident::stale`] is the held cells outside the marks — those with nothing
/// left to resolve from here. Their entities are dropped by [`reconcile`], which
/// finds them outside every prefix once the payload is gone; this only frees the
/// memory the payload held.
fn evict_payloads(
    planned: Res<Planned>,
    spyglass: Res<Spyglass>,
    cameras: Query<&OrbitCamera>,
    mut resident: ResMut<ResidentCells>,
) {
    let mut stale = resident.0.stale(&planned.0);
    // The payloads the walk still marks but the clamp no longer reaches, so a
    // bubble that has moved on does not go on holding the sky behind it.
    if let (Some(radius), Ok(orbit)) = (reach(&spyglass), cameras.single()) {
        stale.extend(
            resident
                .0
                .iter()
                .map(|(id, _)| id)
                .filter(|&id| !cell_in_reach(id, orbit.center, radius)),
        );
    }
    for id in stale {
        resident.0.remove(id);
    }
}

/// One payload point as a drawable system: placed where the payload puts it,
/// named and coloured off the resident tables
///
/// The position comes straight from the payload, in light years — finer than
/// the names table's whole-light-year placement, and present for every system,
/// named or not. The name and the political columns are the same join the
/// spyglass path does, keyed by the point's id.
pub(crate) fn build_from_point(
    point: &Point,
    populated: &Populated,
    names: &Names,
) -> System {
    build_system(
        &RawSystem { address: point.id64 as i64, position: point.pos },
        populated,
        names,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    /// A payload point becomes a system placed exactly at its own position,
    /// named by its id where the resident tables hold nothing on it.
    #[test]
    fn a_point_becomes_a_placed_system() {
        let at = [1234.5, -678.25, 90123.75];
        let point = Point { id64: 7, pos: at, magnitude: 0., temp_bucket: 0 };

        let system = build_from_point(
            &point,
            &Populated::default(),
            &Names::new(Vec::new()),
        );

        assert_eq!(system.address, 7);
        assert_eq!(system.name(), "7", "an unlisted point takes its id");
        assert_eq!(system.position(), DVec3::from(at), "placed exactly");
    }

    /// The clamp is the spyglass reach, and only while it is clearing
    ///
    /// Clearing, the walk is cut off at the reach; not clearing, it runs to the
    /// whole sky and the clamp stands down — the toggle never switches the LOD
    /// off, only where it ends.
    #[test]
    fn the_clamp_is_the_reach_only_while_clearing() {
        let mut spyglass = Spyglass {
            fetch: true,
            radius: 50.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        };
        assert_eq!(reach(&spyglass), Some(50.), "a clearing spyglass clamps");
        spyglass.clear = false;
        assert_eq!(reach(&spyglass), None, "not clearing runs the whole walk");
    }

    /// A cell is in the bubble by its nearest corner, so one well beyond the
    /// reach is out and the one the eye sits in is in.
    #[test]
    fn a_cell_beyond_the_reach_is_out_of_the_bubble() {
        let here = [100.0, 200.0, 24000.0];
        let cell = CellId::of_point(here, 10);
        let center = DVec3::from(here);

        assert!(cell_in_reach(cell, center, 10.0), "the cell the eye sits in");

        let far = center + DVec3::new(5000.0, 0.0, 0.0);
        assert!(
            !cell_in_reach(cell, far, 100.0),
            "a cell thousands of light years off, a reach of a hundred"
        );
    }
}
