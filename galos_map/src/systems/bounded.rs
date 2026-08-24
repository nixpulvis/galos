//! Drawing only the systems the walk marks, off the index's own payloads
//!
//! An alternative source of star entities to the spyglass region fetch. The
//! spyglass reads a sphere and spawns every system in it; this reads the cells
//! the walk marks (`Planned::marks`) and spawns one entity per system in their
//! payloads. The walk spends a point budget, so a zoom out draws a bounded set
//! of marks with everything coarser summed into splats, rather than the
//! million entities the transform walk would then pay for every frame.
//!
//! Off by default, behind [`BoundedFetch`]. While it is off the spyglass
//! drives the map as it always has; while it is on the spyglass region fetch
//! and eviction stand down through their run conditions and these take their
//! place. Only one source of systems runs at a time.
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
use crate::systems::spawn::{PendingSpawns, build_system};
use crate::systems::{PendingEvictions, System};
use crate::{Names, Populated, Transport};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::{CellId, Point, Resident};
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<BoundedFetch>();
    app.init_resource::<ResidentCells>();
    app.init_resource::<BoundedTasks>();

    // Clears the map when the source is switched, before either source runs,
    // so the two never overlap on screen.
    app.add_systems(Update, switch.in_set(MapSet::Search));
    app.add_systems(Update, fetch.in_set(MapSet::Fetch).run_if(enabled));
    app.add_systems(Update, spawn.in_set(MapSet::Populate).run_if(enabled));
    // In `Present` with the spyglass eviction it stands in for, so a drop this
    // frame is decided with the rest of what the frame draws.
    app.add_systems(Update, evict.in_set(MapSet::Present).run_if(enabled));
}

/// Whether the walk-bounded fetch drives the map in place of the spyglass
///
/// Off by default: the spyglass region fetch is the shipped path. Turning this
/// on draws the map from the walk's marks instead — the bounded model — which
/// is not yet verified and should not be the default until it is.
#[derive(Resource, Default)]
pub struct BoundedFetch(pub bool);

/// Whether the bounded source is on, for the systems it drives to run under.
pub(crate) fn enabled(bounded: Res<BoundedFetch>) -> bool {
    bounded.0
}

/// Whether the spyglass source should run, which is whenever the bounded one
/// is not.
pub(crate) fn spyglass(bounded: Res<BoundedFetch>) -> bool {
    !bounded.0
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
    bounded: Res<BoundedFetch>,
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
    mut tasks: ResMut<BoundedTasks>,
) {
    let pool = AsyncComputeTaskPool::get();
    for id in resident.0.missing(&planned.0) {
        if tasks.0.contains_key(&id) {
            continue;
        }
        let source = transport.0.clone();
        tasks
            .0
            .insert(id, pool.spawn(async move { source.payload(id).await }));
    }
}

/// Turn the payloads that arrived into systems, queued for `drain_spawns`
///
/// One system per point, named and coloured off the resident tables, pushed
/// onto the same queue the spyglass fills so the entity is built the one way.
/// The cell's payload is kept resident, keyed by cell, for the evictor to
/// weigh against the next plan.
///
/// TODO(bounded): builds on the main thread, one cell at a time. The region
/// fetch builds off-thread and hands finished rows back; the marks are bounded
/// so this stays small, but a wide zoom landing many cells at once would
/// rather build them on the pool.
fn spawn(
    mut tasks: ResMut<BoundedTasks>,
    mut resident: ResMut<ResidentCells>,
    populated: Res<Populated>,
    names: Res<Names>,
    mut pending: ResMut<PendingSpawns>,
) {
    let now = Instant::now();
    tasks.0.retain(|&id, task| {
        let Some(result) = block_on(future::poll_once(task)) else {
            return true;
        };
        if let Ok(points) = result {
            for point in &points {
                pending.push(
                    build_from_point(point, &populated, &names),
                    false,
                    now,
                );
            }
            resident.0.insert(id, points);
        }
        false
    });
}

/// Drop the payloads the walk has left behind, and queue their systems to go
///
/// [`Resident::stale`] is the held cells outside the current marks. Each is
/// dropped from the cache and its systems queued for `drain_evictions`, found
/// by the addresses the payload carried.
///
/// TODO(bounded): only the cells the walk drops are evicted, so a system the
/// filters have dimmed to nothing stays resident where the spyglass evictor
/// would have despawned it. Harmless to the view, but it holds memory nothing
/// is drawing.
fn evict(
    planned: Res<Planned>,
    mut resident: ResMut<ResidentCells>,
    systems: Query<(Entity, &System)>,
    mut evictions: ResMut<PendingEvictions>,
) {
    let stale = resident.0.stale(&planned.0);
    if stale.is_empty() {
        return;
    }
    let mut dropped = HashSet::new();
    for id in stale {
        if let Some(cell) = resident.0.remove(id) {
            for point in cell.points.iter() {
                dropped.insert(point.id64 as i64);
            }
        }
    }
    for (entity, system) in &systems {
        if dropped.contains(&system.address) {
            evictions.0.insert(entity);
        }
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
}
