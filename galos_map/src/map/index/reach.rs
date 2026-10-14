//! How far the systems near the camera reach, read a cell at a time.
//!
//! A reach is drawn only up close. A system's shell takes its size from its
//! reach once the reach subtends [`WORTH_SIZING`] from the eye, and the
//! largest reach on record is about a fifth of a light year, so past some
//! fifty light years every system is drawn at the size a mark is whatever it
//! reaches. The reaches were a table of every scanned system in the galaxy,
//! 76 M rows held whole for the few hundred near enough to matter.
//!
//! So each cell's reaches are read from the file beside its payload
//! ([`galos_index::prelude::Source::reach`]), for the payloads the map holds
//! within [`NEAR`] of the eye, and laid against the payload's addresses into
//! [`Names::reaches`], which every system is built off. A system drawn before
//! its cell's reaches land is drawn at the stand-in, and [`take`] sets its
//! reach the frame they do.
//!
//! A refresh asks after each held cell's reaches by their own stamp, and lets
//! go of the ones written since; [`ask`] reads them again. A payload let go
//! takes its reaches with it.
//!
//! [`WORTH_SIZING`]: crate::map::bodies::spawn::WORTH_SIZING

use crate::map::camera::OrbitCamera;
use crate::map::galaxy::System;
use crate::map::galaxy::walk::ResidentCells;
use crate::map::index::{Names, Transport};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{IoTaskPool, Task, block_on};
use galos_index::prelude::{CellId, Part, Stamp};
use rustc_hash::FxHashSet;

/// How near the eye a cell has to be for its reaches to be read, in light
/// years
///
/// A shell is sized by its reach once that subtends
/// [`crate::map::bodies::spawn::WORTH_SIZING`], four thousandths of a radian,
/// which a fifth of a light year — about the largest reach on record — does
/// out to fifty light years. Measured to the nearest point of the cell's
/// cube, so a cell the eye is inside is always near.
pub(crate) const NEAR: f64 = 64.;

/// How many cells one read asks for at most. The cells within [`NEAR`] are a
/// few dozen at the deepest levels; this bounds what a jump across the
/// galaxy queues behind it.
const BATCH: usize = 64;

/// How far the eye moves before the near cells are worked out again, in
/// light years. A frame's move is far less than this flying in close.
const STEP: f64 = NEAR / 8.;

pub fn plugin(app: &mut App) {
    app.init_resource::<Reading>();
    app.init_resource::<Moved>();
    app.add_systems(
        Update,
        (take, ask).chain().in_set(crate::map::schedule::MapSet::Fetch),
    );
}

/// One cell's reaches as they land: the cell, the stamp it was read under,
/// and the column in its payload's order.
type Landed = (CellId, Option<Stamp>, Vec<Option<f32>>);

/// The read on the wire, and where the eye was when the near cells were
/// last worked out
#[derive(Resource, Default)]
pub(crate) struct Reading {
    task: Option<Task<Vec<Landed>>>,
    asked_from: Option<DVec3>,
}

/// Read the reaches of the near cells the map holds a payload of and no
/// reaches for
///
/// Only when the eye has moved [`STEP`], or the payloads held have changed,
/// and one read at a time. Cells far past [`NEAR`] have their reaches let go
/// on the way, so what is held stays a view's worth wherever the camera has
/// been.
pub(crate) fn ask(
    transport: Option<Res<Transport>>,
    resident: Res<ResidentCells>,
    names: Res<Names>,
    cameras: Query<&OrbitCamera>,
    mut reading: ResMut<Reading>,
    mut moved: ResMut<Moved>,
) {
    let Some(transport) = transport else { return };
    if reading.task.is_some() {
        return;
    }
    let Ok(orbit) = cameras.single() else { return };
    let eye = orbit.eye();
    let stepped = reading
        .asked_from
        .is_none_or(|from| from.distance(eye) >= STEP);
    if !stepped && !resident.is_changed() && !names.is_changed() {
        return;
    }
    reading.asked_from = Some(eye);
    let at = eye.to_array();

    // Let go of what is far behind, and lay what is held against any more of
    // its payload the map has read since.
    for (id, _) in names.reaches.stamps() {
        if id.bounds().distance_to(at) > NEAR * 4. {
            names.reaches.forget(id);
        } else if let Some(cell) = resident.0.cell(id)
            && names.reaches.laid(id).is_some_and(|laid| laid < cell.points.len())
        {
            let addresses: Vec<i64> =
                cell.points.iter().map(|point| point.id64 as i64).collect();
            moved.0.extend(names.reaches.lay(id, &addresses));
        }
    }

    let mut wanted: Vec<(f64, CellId)> = resident
        .0
        .iter()
        .filter(|(id, _)| !names.reaches.holds(*id))
        .map(|(id, _)| (id.bounds().distance_to(at), id))
        .filter(|(away, _)| *away <= NEAR)
        .collect();
    if wanted.is_empty() {
        return;
    }
    wanted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    wanted.truncate(BATCH);

    let source = transport.0.clone();
    reading.task = Some(IoTaskPool::get().spawn(async move {
        let mut landed = Vec::with_capacity(wanted.len());
        for (_, id) in wanted {
            // The stamp first, so a cell written between the two is held
            // under the older stamp and read again on the next poll.
            let stamp = source.stamp(Part::Reach(id)).await.ok().flatten();
            let column = source.reach(id, usize::MAX).await.unwrap_or_default();
            landed.push((id, stamp, column));
        }
        landed
    }));
}

/// The systems whose reach has moved since [`take`] last set them
#[derive(Resource, Default)]
pub(crate) struct Moved(pub(crate) FxHashSet<i64>);

/// Take a landed read into [`Names::reaches`], and set the reach of every
/// drawn system it moved
///
/// A cell let go of while its read was on the wire is passed over: its
/// reaches would be laid against a payload the map no longer holds. One read
/// asked twice is harmless, a cell's reaches replacing its own.
pub(crate) fn take(
    resident: Res<ResidentCells>,
    names: Res<Names>,
    mut reading: ResMut<Reading>,
    mut moved: ResMut<Moved>,
    mut systems: Query<&mut System>,
) {
    if let Some(task) = reading.task.as_mut()
        && let Some(landed) = block_on(future::poll_once(task))
    {
        reading.task = None;
        for (id, stamp, column) in landed {
            let Some(cell) = resident.0.cell(id) else { continue };
            let addresses: Vec<i64> =
                cell.points.iter().map(|point| point.id64 as i64).collect();
            moved.0.extend(names.reaches.put(id, stamp, column, &addresses));
        }
    }
    if moved.0.is_empty() {
        return;
    }
    let moved = std::mem::take(&mut moved.0);
    for mut system in &mut systems {
        if !moved.contains(&system.address) {
            continue;
        }
        let reach = names.reaches.get(system.address);
        if system.reach != reach {
            system.reach = reach;
        }
    }
}
