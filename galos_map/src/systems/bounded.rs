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
//! [`super::spawn`]'s `drain_spawns`, and an evicted one onto
//! [`PendingEvictions`] for `super`'s `drain_evictions`. The rest of the map — visibility, sizing,
//! pointing, selection, labels — reads a [`System`] without caring which
//! source spawned it.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space::Map;
use crate::systems::aggregate::Planned;
use crate::systems::bodies::spawn::HeldSystem;
use crate::systems::fetch::{FetchTasks, RawSystem};
use crate::systems::filter::{Candidate, Cut, Filtering, Filters};
use crate::systems::scale::View;
use crate::systems::spawn::{PendingSpawns, build_system, system_at};
use crate::systems::{PendingEvictions, Spyglass, System};
use crate::{Names, Populated, ResidentIndex, Transport};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use chrono::{DateTime, Utc};
use galos_index::{
    CellId, MARK_SEPARATION_PX, Part, Point, Resident, STAR_SEPARATION_PX,
    Stamp, resolvable_count,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<LodFetch>();
    app.init_resource::<ResidentCells>();
    app.init_resource::<BoundedTasks>();
    app.init_resource::<AdmittedPoints>();

    // Clears the map when the source is switched, before either source runs,
    // so the two never overlap on screen.
    app.add_systems(Update, switch.in_set(MapSet::Search));
    app.add_systems(Update, fetch.in_set(MapSet::Fetch).run_if(enabled));
    // Arrived payloads land in the cache; the draw reads them from there.
    app.add_systems(Update, collect.in_set(MapSet::Populate).run_if(enabled));
    // Then draw each resident cell's resolvable prefix — grown and shed per
    // system with distance — and drop whatever falls outside every prefix.
    //
    // After the marking, which is what bumps [`Cut`] when a verdict moves.
    // The two conflict on it — one reads, the other writes — so left
    // unordered the schedule picks, and on the frame a filter is asked for
    // this would fill every cell's budget by the verdicts of the cut before
    // it while the standing systems were marked by the one after.
    app.add_systems(
        Update,
        reconcile
            .in_set(MapSet::Populate)
            .after(collect)
            .after(crate::systems::filter::Marking)
            .run_if(enabled),
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
pub(crate) struct ResidentCells(pub(crate) Resident);

/// The payload reads in flight, one per marks cell not yet resident or asked
///
/// Each carries the payload and the [`Stamp`] the transport gave for it, so a
/// refresh knows what it is holding and asks whether that has moved rather
/// than reading every resident cell again. Stamped before the read, so a
/// payload rewritten between the two is held under the older stamp and read
/// again on the next poll — the safe way round.
#[derive(Resource, Default)]
struct BoundedTasks(
    HashMap<CellId, Task<io::Result<(Vec<Point>, Option<Stamp>)>>>,
);

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
    map: Res<Map>,
    systems: Query<Entity, With<System>>,
    camera: Query<Entity, With<OrbitCamera>>,
    mut evictions: ResMut<PendingEvictions>,
    mut resident: ResMut<ResidentCells>,
    mut tasks: ResMut<BoundedTasks>,
    mut admitted: ResMut<AdmittedPoints>,
    mut held: ResMut<crate::refresh::Held>,
    mut fetched: ResMut<FetchTasks>,
    mut last: Local<Option<bool>>,
    mut commands: Commands,
) {
    // Not `is_changed`: the settings checkbox takes `&mut` of this every frame
    // it is drawn, which marks the resource changed whether or not the value
    // moved. Only a real flip should clear the map, so the value is compared
    // against the last one seen.
    if *last == Some(bounded.0) {
        return;
    }
    *last = Some(bounded.0);
    // Up out of whatever it was standing in first, as `super::despawn` does: a
    // camera that has descended into a system is a child of it, and that system
    // is about to be evicted and despawned with its children. Re-parenting it
    // to the map keeps the one floating origin when the system goes.
    if let Ok(eye) = camera.single() {
        commands.entity(eye).insert(ChildOf(map.0));
    }
    for entity in &systems {
        evictions.0.insert(entity);
    }
    // The payloads, what was worked out about them, and the stamps they were
    // read under: one set of three, dropped together. A stamp left behind for
    // a cell that is no longer resident is never asked about again — the
    // refresh stamps what it holds — so it is a row that would sit there for
    // the life of the process.
    resident.0 = Resident::default();
    admitted.cells.clear();
    held.clear();
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
        tasks.0.insert(
            id,
            pool.spawn(async move {
                // The stamp first: a payload republished between the two is
                // then held under the older stamp and re-read by the next
                // refresh, where the other order would hold a stamp for
                // contents the map does not have.
                let stamp = source.stamp(Part::Cell(id)).await.ok().flatten();
                Ok((source.payload(id).await?, stamp))
            }),
        );
    }
}

/// Take the payloads that have arrived into the resident cache
///
/// The transport half only: a payload lands keyed by its cell and the draw
/// reads it from there. Reading it is [`reconcile`]'s, run straight after, so a
/// cell's systems are chosen from what is now held.
///
/// The stamp it arrived under is noted with it, which is what lets
/// [`crate::refresh`] ask whether the cell has been republished since instead
/// of reading every resident payload on every poll.
fn collect(
    mut tasks: ResMut<BoundedTasks>,
    mut resident: ResMut<ResidentCells>,
    mut admitted: ResMut<AdmittedPoints>,
    mut held: ResMut<crate::refresh::Held>,
) {
    tasks.0.retain(|&id, task| {
        let Some(result) = block_on(future::poll_once(task)) else {
            return true;
        };
        if let Ok((points, stamp)) = result {
            adopt(&mut resident, &mut admitted, id, points);
            held.holding(id, stamp);
        }
        false
    });
}

/// Which of a resident cell's points the filters admit, kept until the
/// verdicts move
///
/// [`reconcile`] draws what the filters admit before what they exclude, so it
/// has to know which of every resident payload is which — a walk of every
/// point of every marks cell, asking the filters about each. That answer holds
/// still between the four things that can move it: a filter asked or lifted, a
/// span re-cut against the clock, the political table replaced by a refresh,
/// and the payload itself replaced. [`Cut`] counts the first three and
/// [`adopt`] drops a cell's list with its payload for the fourth, so the walk
/// is done once per cut rather than once per frame.
///
/// Indices into the cell's payload rather than addresses, ascending, so the
/// fill can walk the payload and the admitted list together and take what is
/// in one and not the other without a set to test against.
#[derive(Resource, Default)]
pub(crate) struct AdmittedPoints {
    /// The cut these were taken at
    cut: u64,
    cells: HashMap<CellId, Vec<u32>>,
}

impl AdmittedPoints {
    /// Drop what a new cut, or a map with nothing asked of it, has invalidated
    fn hold(&mut self, cut: u64, asking: bool) {
        if self.cut != cut || !asking {
            self.cut = cut;
            self.cells.clear();
        }
    }

    /// The indices of `points` the filters admit, walking them if this cut has
    /// not asked about this cell yet
    ///
    /// Empty where nothing is asked, since then every point is admitted and an
    /// order over them says nothing. [`reconcile`]'s fill draws the whole
    /// payload in that case, which is the pass this made before the filters
    /// had a say in it.
    fn of(
        &mut self,
        id: CellId,
        points: &[Point],
        filters: &Filters,
        populated: &Populated,
        now: DateTime<Utc>,
    ) -> &[u32] {
        if !filters.asking() {
            return &[];
        }
        self.cells.entry(id).or_insert_with(|| {
            points
                .iter()
                .enumerate()
                .filter(|(_, point)| {
                    filters.admits(&candidate(point, populated), now)
                })
                .map(|(index, _)| index as u32)
                .collect()
        })
    }

    /// Forget a cell, its payload having been freed
    pub(crate) fn forget(&mut self, id: CellId) {
        self.cells.remove(&id);
    }
}

/// Take `points` as a cell's payload, dropping whatever was worked out about
/// the one it replaces
///
/// The two go together and must: [`AdmittedPoints`] holds *indices into the
/// payload*, so a list kept across a replacement names whichever systems now
/// sit at those places. A republished cell is the case — see
/// [`crate::refresh`] — and a first read is the same call with nothing to
/// forget.
pub(crate) fn adopt(
    resident: &mut ResidentCells,
    admitted: &mut AdmittedPoints,
    id: CellId,
    points: Vec<Point>,
) {
    resident.0.insert(id, points);
    admitted.forget(id);
}

/// What the filters ask about a payload point: its address, the factions the
/// resident table puts in it, and the moment the payload carries.
///
/// The same three facts a [`System`] answers, so a point is weighed by the one
/// predicate a drawn system is, and without building a system to ask —
/// [`build_from_point`] clones a name and reads a reach, work worth avoiding
/// for a point that is not going to be drawn.
fn candidate<'a>(point: &Point, populated: &'a Populated) -> Candidate<'a> {
    let address = point.id64 as i64;
    Candidate {
        address,
        factions: populated
            .get(address)
            .map(|system| system.factions.as_slice())
            .unwrap_or(&[]),
        updated_at: DateTime::from_timestamp(point.updated_at as i64, 0),
    }
}

/// The order a cell's points are drawn in: what the filters admit, brightest
/// first, then the rest to fill what is left of the budget.
///
/// `admits` is ascending, so the fill walks it alongside the payload with one
/// cursor and yields the indices it does not name. `fill` false stops the
/// second half outright, for a dim of zero where an excluded system is not
/// drawn at all and queueing one costs a slot of the spawn budget and buys
/// nothing.
fn drawn_first<'a>(
    points: &'a [Point],
    admits: &'a [u32],
    fill: bool,
) -> impl Iterator<Item = usize> + 'a {
    let mut cursor = 0usize;
    let rest = (0..points.len()).filter(move |&index| {
        while cursor < admits.len() && (admits[cursor] as usize) < index {
            cursor += 1;
        }
        !(cursor < admits.len() && admits[cursor] as usize == index)
    });
    admits.iter().map(|&index| index as usize).chain(rest.take(if fill {
        points.len()
    } else {
        0
    }))
}

/// Draw each resident cell's resolvable prefix, admitted systems first, grown
/// and shed per system as the camera moves
///
/// A cell's payload is magnitude-ordered, and [`resolvable_count`] says how many
/// of its systems separate on screen from where the eye stands. Drawing that
/// many — and only that many — is what lets a cell fill in and empty one system
/// at a time rather than switching on whole: a single system is drawn wherever
/// it is resolvable, so the index's cell boundaries stop showing through.
///
/// *Which* of them fill that count is the filters' to say. The count is a
/// budget of marks the screen can tell apart, worked out from the slice's own
/// density and not from which systems are chosen, so spending it on what the
/// filters admit draws exactly as many marks as before, no closer together.
/// Taking the brightest of the payload instead spends the budget on whatever
/// happens to be bright: a faction is a handful of systems in a cell of
/// thousands, so a filter on one used to draw nothing at all from most cells
/// while the marks the screen could carry went unused.
///
/// So the order is: what the filters admit, brightest first, and then — only
/// where [`super::filter::DimTo`] still draws the excluded — the rest,
/// brightest first, to
/// fill whatever the admitted left. The excluded are what a short budget sheds
/// first, which is what they are for: the space a faction is read against
/// gives way to the faction. Where the admitted alone overrun the budget they
/// decimate among themselves by magnitude, exactly as the whole payload used
/// to. Where nothing is asked every system is admitted, the order is the
/// payload's own, and this costs nothing.
///
/// What this gives up is that the drawn set is no longer a prefix of the
/// cell's magnitude order: it is a subset chosen by admission, still in
/// magnitude order within each half. Nothing reads it as a prefix today.
/// Whoever writes the residual splat must subtract the aggregate of the
/// systems actually drawn — `Aggregate::remove` over exactly these points —
/// and not a rank range off [`resolvable_count`], or the glow will double the
/// light of every system the filters promoted into the budget.
///
/// The prefix is pushed to the shared spawn queue, which builds only the
/// systems not already on the map, and everything outside every cell's prefix
/// is queued to drop.
///
/// Three things are spared, as [`super::evict`] spares them on the spyglass
/// path: the system the camera is standing in, since its `FloatingOrigin`
/// hangs under it; a route's stops, which are how the way on is found; and
/// whatever the user has picked out, which they are holding onto by hand.
/// Dropping a selection here does not merely lose it — the ring and the row go
/// on naming it, so [`super::fetch`]'s `fetch_selected` builds the star again
/// the moment [`super::selection`]'s `follow_selection` rewrites the row, and
/// the walk drops it again on the next frame. A system flickering in and out
/// every frame is what that came to.
///
/// The set is written rather than added to, so a system the walk wants again
/// is not carried off by an eviction queued for it several frames ago and
/// still waiting on the budget.
fn reconcile(
    cameras: Query<(&OrbitCamera, &Camera)>,
    index: Res<ResidentIndex>,
    resident: Res<ResidentCells>,
    populated: Res<Populated>,
    names: Res<Names>,
    holding: Res<HeldSystem>,
    spyglass: Res<Spyglass>,
    view_mode: Res<View>,
    selection: Res<crate::systems::selection::Selection>,
    filtering: Filtering,
    cut: Res<Cut>,
    mut admitted: ResMut<AdmittedPoints>,
    systems: Query<(Entity, &System, Has<crate::systems::route::Hop>)>,
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
    // The realistic view resolves stars down to the point spread, far finer
    // than the map's marks, so a cluster stays a field of stars rather than
    // collapsing to its brightest few. See [`STAR_SEPARATION_PX`].
    let separation = match *view_mode {
        View::Map => MARK_SEPARATION_PX,
        View::Realistic => STAR_SEPARATION_PX,
    };

    let existing: HashSet<i64> =
        systems.iter().map(|(_, system, _)| system.address).collect();
    let picked: HashSet<i64> = selection.addresses().into_iter().collect();
    // Every stop of every route being shown. A line is only a line if it has
    // both ends of each leg to draw between, so these are wanted whatever the
    // walk resolves and wherever the bubble ends. See [`Filters::routed`].
    let routed = filtering.filters.routed();

    // With nothing asked every system is admitted, so there is no order to
    // impose: the admitted lists are dropped and the fill draws the payload in
    // its own order, which is what this did before the filters had a say.
    let asking = filtering.filters.asking();
    admitted.hold(cut.0, asking);
    // Whether the excluded are wanted on screen at all. Below the dim they are
    // never spawned ([`super::spawn`]) and dropped where they stand
    // ([`super::evict`]), so queueing them is a slot of the spawn budget spent
    // on a system that cannot land and rebuilt again next frame.
    let fill = !asking || filtering.excluded_are_drawn();
    // One clock for the pass, as the spawn batch takes one: a span's near edge
    // moves by a frame's worth in a frame.
    let wall = Utc::now();

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
        let target = (resolvable_count(indexed, &view, separation) as usize)
            .min(cell.points.len());
        let admits =
            admitted.of(id, &cell.points, &filtering.filters, &populated, wall);
        let order = drawn_first(&cell.points, admits, fill).take(target);
        for point in order.map(|index| &cell.points[index]) {
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

    // The route's own stops, which no cell prefix answers for. They lie
    // wherever the route goes rather than near the camera, so from far enough
    // out to see the whole of a route most of them fall outside every prefix
    // and outside the bubble both, and the walk would never build them.
    //
    // Wanted, which is the one thing said here: it is what builds the stops
    // the map has not got and, below, what keeps the ones it has. Read out of
    // the resident names table, as a searched system is, and pinned so the
    // queue does not weigh them against the reach and forget them unread.
    //
    // Being on the map is not being in view. A stop the spyglass does not
    // reach is hidden by [`crate::systems::visibility`] and the line is cut
    // back to it by [`crate::systems::route::trim`]; what this settles is
    // that the stop is there to be reached at all.
    for &address in &routed {
        wanted.insert(address);
        if !existing.contains(&address) {
            if let Some(system) = system_at(address, &populated, &names) {
                pending.push(system, true, now);
            }
        }
    }

    // Everything outside every prefix goes: the tail a cell sheds as it
    // recedes, the systems of a cell whose payload has been freed, and —
    // clearing — whatever fell outside the bubble above. Written whole, so a
    // system the walk has taken back is not still down for eviction.
    evictions.0 = systems
        .iter()
        .filter(|(entity, system, hop)| {
            if Some(*entity) == holding.of()
                || *hop
                || picked.contains(&system.address)
            {
                return false;
            }
            !wanted.contains(&system.address)
        })
        .map(|(entity, ..)| entity)
        .collect();
}

/// Free the payloads of cells the walk no longer wants
///
/// [`Resident::stale`] is the held cells outside the marks — those with nothing
/// left to resolve from here. Their entities are dropped by [`reconcile`], which
/// finds them outside every prefix once the payload is gone; this only frees the
/// memory the payload held, and the verdicts held about its points with it.
fn evict_payloads(
    planned: Res<Planned>,
    spyglass: Res<Spyglass>,
    cameras: Query<&OrbitCamera>,
    mut resident: ResMut<ResidentCells>,
    mut admitted: ResMut<AdmittedPoints>,
    mut held: ResMut<crate::refresh::Held>,
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
        admitted.forget(id);
        held.forget(id);
    }
}

/// One payload point as a drawable system: placed where the payload puts it,
/// named and coloured off the resident tables
///
/// The position comes straight from the payload, in light years — finer than
/// the names table's whole-light-year placement, and present for every system,
/// named or not. The name and the political columns are the same join the
/// spyglass path does, keyed by the point's id.
///
/// The one place a point becomes a system, the spyglass fetch included, so the
/// payload's [`Point::updated_at`] is read into a moment here rather than at
/// each caller. Unix seconds on the wire and a moment on the map: the payload
/// keeps four bytes a system and the filter compares against a clock.
pub(crate) fn build_from_point(
    point: &Point,
    populated: &Populated,
    names: &Names,
) -> System {
    build_system(
        &RawSystem {
            address: point.id64 as i64,
            position: point.pos,
            magnitude: Some(point.magnitude),
            temp_bucket: Some(point.temp_bucket),
            updated_at: DateTime::from_timestamp(point.updated_at as i64, 0),
        },
        populated,
        names,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::route::graph::{Drive, Routing};
    use bevy::math::DVec3;

    /// A payload point becomes a system placed exactly at its own position,
    /// named by its id where the resident tables hold nothing on it.
    #[test]
    fn a_point_becomes_a_placed_system() {
        let at = [1234.5, -678.25, 90123.75];
        let point = Point {
            id64: 7,
            pos: at,
            magnitude: 0.,
            temp_bucket: 0,
            updated_at: 0,
        };

        let system = build_from_point(
            &point,
            &Populated::default(),
            &Names::reaching(Vec::new(), Vec::new()),
        );

        assert_eq!(system.address, 7);
        assert_eq!(system.name(), "7", "an unlisted point takes its id");
        assert_eq!(system.position(), DVec3::from(at), "placed exactly");
    }

    /// A span admits the point the index says was updated inside it
    ///
    /// The whole chain the filter on time rests on: the payload's Unix second
    /// becomes the moment on the [`System`], and the span is measured against
    /// that rather than against when the star was drawn. Stamping the moment of
    /// the build here is what it did before the payload carried one, and it
    /// admitted every system on the map to every span.
    #[test]
    fn a_span_admits_a_point_by_the_moment_it_carries() {
        use crate::systems::filter::{Filter, Filters};
        use chrono::{Duration as Span, Utc};

        let now = Utc::now();
        let point = |id: u64, ago: i64| Point {
            id64: id,
            pos: [0.; 3],
            magnitude: 0.,
            temp_bucket: 0,
            updated_at: (now - Span::seconds(ago)).timestamp() as u32,
        };
        let built = |point: &Point| {
            build_from_point(
                point,
                &Populated::default(),
                &Names::reaching(Vec::new(), Vec::new()),
            )
        };

        let mut filters = Filters::default();
        filters.add(Filter::Recency {
            label: "Last 1 hour".into(),
            span: Span::hours(1),
        });

        assert!(
            filters.admit(&built(&point(1, 30)), now),
            "reported half a minute ago"
        );
        assert!(
            !filters.admit(&built(&point(2, 60 * 60 * 24)), now),
            "a day old, and the span asks about an hour"
        );
    }

    /// A payload point at `id`, faintness rising with the id
    fn point(id: u64) -> Point {
        Point {
            id64: id,
            pos: [0.; 3],
            magnitude: id as f32,
            temp_bucket: 0,
            updated_at: 0,
        }
    }

    /// The addresses the draw would take, in order, from a budget of `target`
    fn drawn(
        points: &[Point],
        admits: &[u32],
        fill: bool,
        target: usize,
    ) -> Vec<u64> {
        drawn_first(points, admits, fill)
            .take(target)
            .map(|index| points[index].id64)
            .collect()
    }

    /// What the filters admit takes the budget, however faint it is
    ///
    /// The count is the screen's, worked out from the slice's density and not
    /// from which systems fill it, so spending it on the admitted draws as many
    /// marks as before. Brightest-first over the whole payload is what drew
    /// nothing from a cell of thousands with a faction filter on.
    #[test]
    fn the_admitted_take_the_budget_before_the_excluded() {
        let points: Vec<Point> = (1..=6).map(point).collect();
        // The fourth and sixth brightest are the ones asked for.
        let admits = [3u32, 5];

        assert_eq!(
            drawn(&points, &admits, true, 2),
            vec![4, 6],
            "the admitted, brightest of them first"
        );
    }

    /// The excluded fill what the admitted leave, and so are shed first
    ///
    /// Which is what dimming is for: the space a faction is read against, and
    /// the first thing to give way when there is less room than systems.
    #[test]
    fn the_excluded_fill_what_is_left_and_go_first() {
        let points: Vec<Point> = (1..=6).map(point).collect();
        let admits = [3u32];

        assert_eq!(
            drawn(&points, &admits, true, 4),
            vec![4, 1, 2, 3],
            "the one admitted, then the brightest of the rest"
        );
        assert_eq!(
            drawn(&points, &admits, true, 1),
            vec![4],
            "a budget of one leaves nothing for the excluded"
        );
    }

    /// Where the excluded are not drawn at all they are not offered either
    ///
    /// At a dim of zero [`super::spawn`] refuses them and [`super::evict`]
    /// drops them, so queueing one spends a slot of the spawn budget on a
    /// system that cannot land — and it is rebuilt and queued again every frame,
    /// since it never becomes an entity to be found already drawn.
    #[test]
    fn nothing_excluded_is_offered_while_the_dim_drops_it() {
        let points: Vec<Point> = (1..=6).map(point).collect();
        let admits = [3u32];

        assert_eq!(
            drawn(&points, &admits, false, 4),
            vec![4],
            "the admitted alone, though the budget has room"
        );
    }

    /// A filter dense enough to overrun the budget decimates by magnitude
    ///
    /// The admitted are ordered among themselves as the whole payload used to
    /// be, so a filter admitting everything draws exactly what no filter draws.
    #[test]
    fn a_dense_filter_decimates_by_magnitude() {
        let points: Vec<Point> = (1..=6).map(point).collect();
        let all: Vec<u32> = (0..6).collect();

        assert_eq!(drawn(&points, &all, true, 3), vec![1, 2, 3]);
        assert_eq!(
            drawn(&points, &[], true, 3),
            vec![1, 2, 3],
            "and nothing asked is the payload's own order"
        );
    }

    /// The verdicts are taken once per cut and kept
    ///
    /// Choosing by admission means asking about every point of every resident
    /// payload, which is a walk to keep rather than to repeat each frame. It is
    /// redone when the filters move and not otherwise; see [`Cut`].
    #[test]
    fn the_verdicts_are_kept_until_the_filters_move() {
        use crate::systems::filter::Filter;
        use galos_index::meta::PopulatedSystem;

        let points: Vec<Point> = (1..=4).map(point).collect();
        let id = CellId::of_point([0.; 3], 4);
        // The third point is the only one a faction is present in.
        let populated = Populated(std::sync::Arc::new(HashMap::from([(
            3i64,
            PopulatedSystem {
                address: 3,
                name: "Held".into(),
                position: [0.; 3],
                population: 1,
                security: None,
                government: None,
                allegiance: None,
                primary_economy: None,
                secondary_economy: None,
                factions: vec![7],
                body_count: None,
                non_body_count: None,
            },
        )])));

        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 7, name: "Faction 7".into() });
        let now = Utc::now();

        let mut held = AdmittedPoints::default();
        held.hold(1, true);
        assert_eq!(
            held.of(id, &points, &filters, &populated, now),
            &[2],
            "the point the faction is present in, by its place in the payload"
        );

        // Asking for a second faction readmits nothing here, but the cut has
        // moved and the walk is taken again rather than the old answer kept.
        filters.add(Filter::Systems {
            label: "2 systems".into(),
            systems: vec![1, 4],
        });
        held.hold(2, true);
        assert_eq!(
            held.of(id, &points, &filters, &populated, now),
            &[0, 2, 3],
            "the faction's, and the two picked out by hand"
        );

        // Nothing asked admits everything, so there is no order to hold.
        held.hold(2, false);
        assert!(
            held.of(id, &points, &Filters::default(), &populated, now)
                .is_empty(),
        );
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

    /// Switching the source brings the camera up out of its system first
    ///
    /// The switch clears the whole map, the held system among it, and a camera
    /// that has descended into one is a child of it. Left there it would be
    /// despawned with the system and the big space would lose its one floating
    /// origin. So it comes up onto the map, as a clear does (`super::super::despawn`).
    #[test]
    fn switching_the_source_brings_the_camera_up_out_of_its_system() {
        use crate::systems::tests::system;

        let mut app = App::new();
        app.add_systems(Update, switch);
        app.init_resource::<PendingEvictions>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<BoundedTasks>();
        app.init_resource::<AdmittedPoints>();
        app.init_resource::<crate::refresh::Held>();
        app.init_resource::<FetchTasks>();
        app.insert_resource(LodFetch(true));

        let map = app.world_mut().spawn_empty().id();
        app.insert_resource(Map(map));
        let star = app.world_mut().spawn((system(1), ChildOf(map))).id();
        let eye =
            app.world_mut().spawn((OrbitCamera::default(), ChildOf(star))).id();

        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(eye).map(|of| of.parent()),
            Some(map),
            "the camera stayed inside a system the switch will despawn",
        );
    }

    /// A world with the walk holding nothing, so every system is out of reach
    /// of every prefix and only what is spared survives
    fn walking() -> App {
        let mut app = App::new();
        app.add_systems(Update, reconcile);
        app.init_resource::<PendingEvictions>();
        app.init_resource::<PendingSpawns>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<HeldSystem>();
        app.init_resource::<crate::systems::selection::Selection>();
        app.init_resource::<crate::systems::filter::Filters>();
        app.init_resource::<crate::systems::filter::DimTo>();
        app.init_resource::<crate::systems::filter::Cut>();
        app.init_resource::<AdmittedPoints>();
        app.insert_resource(ResidentIndex(galos_index::Index::default()));
        app.insert_resource(Populated::default());
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.insert_resource(View::Map);
        app.insert_resource(Spyglass {
            fetch: true,
            radius: 50.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()));
        app
    }

    /// Which systems the walk has queued to drop
    fn dropping(app: &mut App) -> Vec<i64> {
        let queued = app.world().resource::<PendingEvictions>().0.clone();
        let mut addresses: Vec<i64> = queued
            .iter()
            .filter_map(|&entity| app.world().get::<System>(entity))
            .map(|system| system.address)
            .collect();
        addresses.sort();
        addresses
    }

    /// The walk does not drop what the user has picked out
    ///
    /// The reported trouble: a selection panned away from and zoomed past fell
    /// outside every cell's prefix, so the walk dropped its star — and
    /// `fetch::fetch_selected` built it again the moment
    /// `selection::follow_selection` rewrote the row off the star that had just
    /// arrived. The two ran a frame apart, and the system flickered in and out
    /// for as long as the selection stood. Spared here, as
    /// [`super::evict`] spares it on the spyglass path.
    #[test]
    fn the_walk_does_not_drop_a_selection() {
        use crate::systems::selection::{Picked, Selection};
        use crate::systems::tests::system;

        let mut app = walking();
        let picked = system(1);
        app.world_mut().spawn(picked.clone());
        app.world_mut().spawn(system(2));
        app.world_mut().resource_mut::<Selection>().set(Picked::System(picked));

        app.update();

        assert_eq!(
            dropping(&mut app),
            vec![2],
            "the walk dropped the star the selection is drawn on"
        );
    }

    /// Nor a stop on a route, which is how the way on is found
    #[test]
    fn the_walk_does_not_drop_a_route_stop() {
        use crate::systems::route::Hop;
        use crate::systems::tests::system;

        let mut app = walking();
        app.world_mut().spawn((system(1), Hop::Next));
        app.world_mut().spawn(system(2));

        app.update();

        assert_eq!(dropping(&mut app), vec![2], "the walk dropped a stop");
    }

    /// And every stop of a route it is showing, not only the two adjacent ones
    ///
    /// Reported as a route drawn in pieces: the line ran in dashes, whole
    /// stretches of it missing. A route's stops lie wherever the route goes
    /// rather than near the camera, so from far enough out to see the whole
    /// of it most of them fall outside every cell's resolvable prefix. The
    /// walk never built them, and [`super::super::route::trim`] cuts the line
    /// at a stop the map does not hold exactly as it cuts one the spyglass
    /// has put away. [`Hop`] was all that was spared, and that marks two
    /// stops — the one behind and the one ahead — so the rest of the line
    /// went.
    ///
    /// Two halves to it: a stop already on the map is not dropped, and one
    /// the walk never built is asked for.
    #[test]
    fn the_walk_holds_every_stop_of_a_route() {
        use crate::systems::filter::{Filter, Filters};
        use crate::systems::tests::system;
        use galos_index::NameEntry;

        let mut app = walking();
        app.insert_resource(Names::reaching(
            (1..=3)
                .map(|address| NameEntry {
                    address,
                    name: format!("Stop {address}"),
                    position: [address as f32 * 10., 0., 0.],
                })
                .collect(),
            Vec::new(),
        ));
        app.world_mut().resource_mut::<Filters>().add(Filter::Route {
            label: "Stop 1 to Stop 3".into(),
            systems: vec![1, 2, 3],
            range: "10".into(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        // The first stop already drawn, the other two never built, and a
        // system that is on no route at all.
        app.world_mut().spawn(system(1));
        app.world_mut().spawn(system(4));

        app.update();

        assert_eq!(
            dropping(&mut app),
            vec![4],
            "the walk dropped a stop the line has to reach"
        );
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            2,
            "the stops the walk never built were never asked for"
        );
    }

    /// The walk keeps what the filters admit and sheds the rest
    ///
    /// End to end through the real pass: a resident cell, a faction filter, and
    /// a dim of zero where an excluded system is not drawn at all. The one
    /// system the faction is present in is the faintest of the five, so a
    /// brightest-first prefix kept the four it is not in and dropped it — the
    /// map went dark where the filter was supposed to show something.
    #[test]
    fn the_walk_keeps_what_the_filters_admit() {
        use crate::systems::filter::{Filter, Filters};
        use crate::systems::tests::system;
        use galos_index::meta::PopulatedSystem;
        use galos_index::{BuildParams, Snapshot};

        // Five systems a few light years apart, faintest last, and the faction
        // is in that faintest one.
        let held = 5i64;
        let inputs: Vec<galos_index::System> = (1..=5)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: [id as f64, 0., 0.],
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(ResidentIndex(built.index.clone()));
        {
            let mut resident = app.world_mut().resource_mut::<ResidentCells>();
            for cell in built.index.cells() {
                let points = built.payload(cell.id);
                if !points.is_empty() {
                    resident.0.insert(cell.id, points.to_vec());
                }
            }
        }
        app.insert_resource(Populated(std::sync::Arc::new(HashMap::from([(
            held,
            PopulatedSystem {
                address: held,
                name: "Held".into(),
                position: [held as f32, 0., 0.],
                population: 1,
                security: None,
                government: None,
                allegiance: None,
                primary_economy: None,
                secondary_economy: None,
                factions: vec![7],
                body_count: None,
                non_body_count: None,
            },
        )]))));
        for address in 1..=5 {
            let mut drawn = system(address);
            drawn.position = [address as f64, 0., 0.];
            app.world_mut().spawn(drawn);
        }

        // Nothing asked: the walk keeps every system it resolves, so whatever
        // it drops here it drops for being unresolvable and not for a filter.
        app.update();
        let unfiltered = dropping(&mut app);
        assert!(
            !unfiltered.contains(&held),
            "the faintest system resolves before any filter is asked"
        );

        // Asked for, with the excluded still drawn faintly: the budget has
        // room for all five, so the dim ones stay as the space the faction is
        // read against.
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Faction { id: 7, name: "Faction 7".into() });
        app.update();
        assert!(
            dropping(&mut app).is_empty(),
            "the excluded are drawn at this dim and the budget holds them"
        );

        // Dimmed to nothing, where an excluded system is not drawn at all:
        // only what the filter admits is wanted, and the rest go.
        app.insert_resource(crate::systems::filter::DimTo(0.));
        app.update();

        let dropped = dropping(&mut app);
        assert!(
            !dropped.contains(&held),
            "the walk dropped the one system the filter asked for"
        );
        assert_eq!(
            dropped,
            (1..held).collect::<Vec<i64>>(),
            "and it kept systems no filter admits, at a dim that drops them"
        );
    }

    /// An eviction is re-decided every frame, not left standing
    ///
    /// The queue is drained against a budget, so an entry may wait frames. Held
    /// rather than rewritten, a system the walk had taken back — picked out
    /// since it was queued, or resolvable again — would be carried off by the
    /// eviction it no longer deserves.
    #[test]
    fn an_eviction_is_let_go_of_when_the_walk_takes_a_system_back() {
        use crate::systems::selection::{Picked, Selection};
        use crate::systems::tests::system;

        let mut app = walking();
        let picked = system(1);
        app.world_mut().spawn(picked.clone());
        app.update();
        assert_eq!(dropping(&mut app), vec![1], "nothing was queued to drop");

        // Picked out while the eviction sits in the queue.
        app.world_mut().resource_mut::<Selection>().set(Picked::System(picked));
        app.update();

        assert!(
            dropping(&mut app).is_empty(),
            "the selection was carried off by a stale eviction"
        );
    }
}
