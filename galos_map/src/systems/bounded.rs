//! Drawing only the systems the walk marks, off the index's own payloads
//!
//! The map's one source of star entities. It reads the cells the walk marks
//! (`Planned::marks`) and spawns one entity per system in their payloads. The
//! walk spends no budget: a cell's slice draws exactly where its systems
//! separate on screen and everything coarser is summed into splats, so what
//! is drawn is bounded by what the screen can resolve rather than by the
//! million entities a sphere wide enough to hold the same sky would pay a
//! transform for every frame. That sphere is what this replaced — a spyglass
//! region fetch that read everything in reach at full density — and the count
//! reaching the map is held down here instead: see [`reach`] and the
//! per-point clamp in `reconcile`.
//!
//! The spyglass lives on as a bound and not as a source. It says how far the
//! walk is clamped and how much of what the walk holds is drawn, never what
//! is loaded — see [`reach`].
//!
//! It owns no drawing of its own: a built system is pushed onto the
//! [`PendingSpawns`] queue a route's stops and a picked-out system arrive on
//! too, and turned into an entity by [`super::spawn`]'s `drain_spawns`; an
//! evicted one goes onto [`PendingEvictions`] for `super`'s
//! `drain_evictions`. The rest of the map — visibility, sizing, pointing,
//! selection, labels — reads a [`System`] without caring where it came from.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::aggregate::{Accounted, Planned};
use crate::systems::bodies::spawn::HeldSystem;
use crate::systems::fetch::RawSystem;
use crate::systems::filter::{Candidate, Cut, Filtering, Prepared};
use crate::systems::scale::{ScalePopulation, View, by_population};
use crate::systems::spawn::{PendingSpawns, build_system, system_at};
use crate::systems::{PendingEvictions, Spyglass, System};
use crate::{Names, Populated, Transport};
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::system::SystemParam;
use bevy::log::tracing::Instrument;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use chrono::{DateTime, Utc};
use galos_index::screen::{Empty, frame_marks, share, wanted};
use galos_index::{
    CellId, Inhabited, Part, Point, Resident, Stamp,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io;
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    app.init_resource::<ResidentCells>();
    app.init_resource::<BoundedTasks>();
    app.init_resource::<PointOrders>();
    app.init_resource::<Republished>();
    app.init_resource::<Keeping>();
    app.init_resource::<Sampled>();
    app.init_resource::<Blobs>();

    app.add_systems(Update, fetch.in_set(MapSet::Fetch));
    // Arrived payloads land in the cache; the draw reads them from there.
    app.add_systems(Update, collect.in_set(MapSet::Populate));
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
            .after(crate::systems::filter::Marking),
    );
    // Free the payloads of cells the walk no longer wants at all.
    app.add_systems(Update, evict_payloads.in_set(MapSet::Present));
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

/// Whether the clamp still reaches a cell, the bound being off where there
/// is none
fn in_reach(id: CellId, orbit: &OrbitCamera, bubble: Option<f64>) -> bool {
    bubble.is_none_or(|radius| cell_in_reach(id, orbit.center(), radius))
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
/// Keyed by cell, so [`Resident::missing`] is the marks a fetch must load.
/// What is dropped again is [`Keeping`]'s to say, not the marked set's.
#[derive(Resource, Default)]
pub(crate) struct ResidentCells(pub(crate) Resident);

/// How long a payload the walk has stopped marking is held before it is freed
///
/// **The whole of why it is held at all.** A payload freed the frame it stops
/// being marked is a payload read again the frame it is marked next, and a
/// zoom marks a different set every frame: measured over one flight out to
/// the galaxy and back ([`super::flight`]), 2,615 payload reads of which
/// **1,818 were cells read a second time** — seventy per cent of the reads,
/// and with them the systems built out of them, evicted and built again.
///
/// Two seconds, which is longer than a zoom step and shorter than a change of
/// mind. What bounds the memory that buys is [`SLACK`], not this.
const KEEP: Duration = Duration::from_secs(2);

/// How many times the marked set's worth of payloads may be held at once
///
/// The ceiling under [`KEEP`], and what keeps the grace from being a leak: a
/// held set is allowed to run to twice what the view asks for, and past that
/// the least recently wanted are freed however new they are. Proportional to
/// the view rather than a fixed count, so the memory a wide zoom holds stays
/// what that zoom needs — which is what it was before the grace existed.
const SLACK: usize = 2;

/// How often the held payloads are swept when nothing is moving
///
/// A quarter of a second, an eighth of [`KEEP`]. What the sweep decides
/// turns on a grace measured in seconds, so running it every frame is
/// sixty answers to a question that changes once; what it costs is a pass
/// over everything held, which at a wide zoom is a hundred thousand
/// payloads. See [`evict_payloads`].
const SWEEPS: Duration = Duration::from_millis(250);

/// Which cells the walk marks, and when each held payload was last wanted
///
/// Two answers with one owner, because the second is only meaningful against
/// the first. The marked set is a `Vec` on [`Planned`] and every reader of it
/// asks the same question — *how much of this cell is drawn* — so it is kept
/// here as a map and rebuilt only when the plan moves ([`fetch`]), rather
/// than walked or rebuilt by each of the three systems that ask.
///
/// The stamps are what [`KEEP`] is measured from. A cell wanted this frame is
/// stamped with this frame; one nobody has asked for keeps the stamp of the
/// last frame that did, and is freed once that is [`KEEP`] old.
#[derive(Resource, Default)]
pub(crate) struct Keeping {
    /// The marked set, rebuilt when [`Planned`] moves
    marked: FxHashSet<CellId>,
    /// When each held payload was last wanted
    seen: FxHashMap<CellId, Instant>,
}

impl Keeping {
    /// Take the plan's marks as the set every reader asks against
    fn marks(&mut self, marks: &[galos_index::MarkRef]) {
        self.marked.clear();
        self.marked.extend(marks.iter().map(|mark| mark.id));
    }

    /// Whether the walk marks this cell
    ///
    /// What [`reconcile`] draws from: a held payload the walk no longer marks
    /// is kept against the next frame that marks it, and drawing it meanwhile
    /// would put back the far, faint sky the walk had just shed.
    fn marks_it(&self, id: CellId) -> bool {
        self.marked.contains(&id)
    }

    /// Note that `id` is wanted as of `now`
    fn wanted(&mut self, id: CellId, now: Instant) {
        self.seen.insert(id, now);
    }

    /// When `id` was last wanted, taking arrival as wanted
    ///
    /// A payload that has just landed has never been through a pass that
    /// stamps it, and reading its absence as "wanted nobody knows when" would
    /// free it before it was ever drawn.
    fn last(&mut self, id: CellId, now: Instant) -> Instant {
        *self.seen.entry(id).or_insert(now)
    }

    /// Done with: the payload is gone and so is the stamp
    fn forget(&mut self, id: CellId) {
        self.seen.remove(&id);
    }
}

/// The cells whose payload has been replaced since [`reconcile`] last read it
///
/// A republished cell is mostly the same addresses said again, and a drawn
/// system is a [`System`] built out of a payload point once. So `reconcile`'s
/// ordinary test — build only what is not already on the map — is exactly
/// wrong for one: every address is already there, nothing is queued, and the
/// systems keep the columns of the first read for as long as they stay drawn.
/// What goes stale with them is everything the cells carry and the names table
/// does not: the moment a span is cut against, the magnitude the sky is
/// painted from, the position, and the political columns
/// ([`super::filter`]'s `mark` re-asks a system's own copy of those).
///
/// So a replaced payload is noted here and its cell is rebuilt whole on the
/// next walk, `spawn_systems` replacing each system in place. Noted rather
/// than acted on at once because the walk is where a cell's drawn prefix is
/// known, and rebuilding a system the prefix does not reach would spend the
/// spawn budget on something about to be evicted.
#[derive(Resource, Default)]
pub(crate) struct Republished(HashSet<CellId>);

impl Republished {
    /// Whether this cell wants rebuilding rather than filling in
    fn holds(&self, id: CellId) -> bool {
        self.0.contains(&id)
    }

    /// Done with: the walk has rebuilt what it draws of this cell
    ///
    /// Per cell rather than cleared whole, since a walk skips the cells
    /// outside the bubble and a cell it never reached is still republished.
    fn settled(&mut self, id: CellId) {
        self.0.remove(&id);
    }
}

/// The merged marks the frame draws, one a cell whose whole contents fall
/// inside a mark
///
/// **Not entities, and that is the point.** A blob stands for a subtree
/// rather than for a system: it has no name, no bodies, no route and
/// nothing to select in the sense a star has, and at galaxy scale there
/// are tens of thousands of them. Spawning that many would pay a
/// transform, a visibility and a picking test apiece for a set that is
/// rewritten whenever the eye moves. So they are a list, laid into
/// [`super::field`]'s one mesh beside the stars.
///
/// **Drawn as a mark and no larger.** A blob is one mark because everything
/// it holds falls inside one, not because it is worth more than one: a mark
/// drawn wider for standing over more systems would say the density with
/// size, and the density is what the *number* of marks says. It is thinned
/// on the same [`share`] every read cell is, so a merged region comes out
/// no denser or thinner than a read one beside it.
///
/// Written by [`reconcile`].
#[derive(Resource, Default)]
pub struct Blobs(pub(crate) Vec<Blob>);

/// One merged mark: where it stands and what it stands for.
#[derive(Copy, Clone)]
pub(crate) struct Blob {
    /// The average of the marks it stands for, in linear light, and the
    /// fade the filters leave it: see [`super::merged::Standing`].
    pub(crate) light: Vec3,
    pub(crate) fade: f32,
    /// The cell it stands for, which is what names it: the system a merged
    /// mark is pointed at is read out of this cell's own payload. See
    /// [`super::merged`].
    pub(crate) id: CellId,
    /// How many systems are under it, for the readout.
    pub(crate) count: u64,
    /// Its systems' count-weighted centroid, light years: where the mark is
    /// painted, which is where they are and not where the box is.
    pub(crate) at: [f64; 3],
    /// The brightest absolute magnitude under it, or [`None`] where the
    /// cell carries no photometry.
    pub(crate) m_min: Option<f32>,
}

/// What the draw has worked out about the resident payloads: the orders a
/// cell's points are drawn in, which cells have been published again since it
/// last looked, and which cells the walk marks.
///
/// Bundled so [`reconcile`] reads all three without spending three of Bevy's
/// system-parameter slots, that walk being at the limit.
#[derive(SystemParam)]
pub(crate) struct Worked<'w> {
    /// Who lives in each cell, which is what the population scale draws
    /// from; see [`super::populated::PopulatedCells`].
    populated_cells: Res<'w, super::populated::PopulatedCells>,
    /// What each merged mark stands for and what the filters leave of it;
    /// see [`super::merged::Standing`].
    standing: Res<'w, super::merged::Standing>,
    orders: ResMut<'w, PointOrders>,
    republished: ResMut<'w, Republished>,
    /// The marked set, which this pass is the one to take from the plan: it
    /// is the first of the three to read it and the only one that must not
    /// read it a frame late.
    keeping: ResMut<'w, Keeping>,
    planned: Res<'w, Planned>,
    /// What each marked cell's marks account for, which the field subtracts
    /// from the aggregate it lays down so the two never draw one system twice.
    drawn: ResMut<'w, crate::systems::aggregate::Drawn>,
    /// What the frame's marks came to, for the panel to read.
    sampled: ResMut<'w, Sampled>,
    /// The merged marks the frame draws, which the merge above settles.
    blobs: ResMut<'w, Blobs>,
}

/// The payload reads in flight, one per marks cell not yet resident or asked
///
/// Each carries the payload and the [`Stamp`] the transport gave for it, so a
/// refresh knows what it is holding and asks whether that has moved rather
/// than reading every resident cell again. Stamped before the read, so a
/// payload rewritten between the two is held under the older stamp and read
/// again on the next poll — the safe way round.
#[derive(Resource, Default)]
pub(crate) struct BoundedTasks(
    HashMap<CellId, Task<io::Result<(Vec<Point>, Option<Stamp>)>>>,
);

#[cfg(test)]
impl BoundedTasks {
    /// The cells with a read in flight, for a caller that wants to know what
    /// a frame put to the transport
    ///
    /// Read by the flight guard ([`super::flight`]), which counts a cell read
    /// twice over one flight as work paid for twice.
    pub(crate) fn cells(&self) -> impl Iterator<Item = CellId> + '_ {
        self.0.keys().copied()
    }

    /// Whether nothing is on the wire
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How much more of a cell is read than the share asks for
///
/// A share moves with the camera, and a cell already held at exactly what
/// the last frame wanted is one re-read the moment the frame wants one
/// more. So a read takes a few times the ask and a floor besides, and a
/// cell is asked for again only once the share has grown past what it
/// holds. Four and sixteen: an octave and a half of zoom before a re-read,
/// and sixteen points is under a kilobyte.
const READ_SLACK: usize = 4;
const READ_LEAST: usize = 16;

/// Ask for the payloads of the marks cells the map does not hold enough of
///
/// **A prefix and not the payload.** A cell's payload is magnitude-ordered
/// and the draw takes a share of it ([`wanted`]), so the rest is bytes
/// faulted, held and never looked at: measured over `.index/full`, one
/// flight held 121 M points and 5.8 GB to draw eight thousand marks, and
/// the fill-in after every camera move was the map waiting on them. What is
/// asked for here is what the draw will take, with [`READ_SLACK`] to spare.
///
/// Only the cells not already held deep enough or already on the wire, so a
/// still view whose marks are all held asks for nothing and a zoom asks
/// only for the annulus it newly reaches, plus whatever the growing share
/// has outgrown. Run every frame rather than on a plan change: a still
/// camera whose plan has not moved may yet have marks nobody has asked for
/// — the map opening on one, or a payload freed and wanted again.
///
/// Where a filter is asked the whole cell is read instead. The filters
/// promote systems out of magnitude order — a faction is a handful of
/// systems anywhere in a payload — so a prefix is the one thing that cannot
/// answer them.
pub(crate) fn fetch(
    planned: Res<Planned>,
    resident: Res<ResidentCells>,
    transport: Res<Transport>,
    filters: Res<crate::systems::filter::Filters>,
    view_mode: Res<View>,
    scale_population: Res<ScalePopulation>,
    cameras: Query<(&OrbitCamera, &Camera)>,
    mut tasks: ResMut<BoundedTasks>,
) {
    // **Only when something it reads has moved.** The scan below is the
    // one flat cost a still view used to pay for nothing: a pass over
    // every marked cell, which at a wide zoom is tens of thousands of
    // them, answering the same empty ask every frame. What it turns on is
    // the plan, what the map holds and what the filters want, and those
    // are exactly the three resources here that change.
    if !planned.is_changed()
        && !resident.is_changed()
        && !filters.is_changed()
        && !view_mode.is_changed()
        && !scale_population.is_changed()
    {
        return;
    }
    // **Nothing at all while the sky is read as populations.** That mode
    // draws the systems anybody lives in and takes them from the resident
    // table ([`super::populated::PopulatedCells`]), so a payload answers nothing it
    // asks — and a payload read for nothing is the whole galaxy faulted in
    // to draw a few hundred marks. What is already held is left to
    // [`evict_payloads`] to let go of on its own grace.
    //
    // Unless a span is asked, a moment being a payload's to carry; see
    // [`Filters::asking_a_span`] and [`reconcile`].
    if crate::systems::scale::by_population(&view_mode, &scale_population)
        && !filters.asking_a_span()
    {
        return;
    }
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = crate::systems::aggregate::view(orbit, camera) else {
        return;
    };
    let pool = AsyncComputeTaskPool::get();
    let whole = filters.asking();
    // What the draw will spread over. Off the plan alone — a mark carries
    // its own slice — which is what lets the share be struck here as well
    // as in [`reconcile`] and lets the two agree without either of them
    // touching the index.
    let share = share(population(&planned.0), frame_marks(&view));
    // The two halves of the frame's flat cost, measured apart: the set
    // arithmetic over every marked cell, and the asking that follows it. A
    // still view asks for nothing and pays the first of them anyway, which is
    // what a capture has to be able to see.
    let asking = {
        let _zone = info_span!("missing cells").entered();
        let mut asking = Vec::new();
        for mark in &planned.0.marks {
            // Past the clamp there is nothing to test for: the walk is
            // clamped to the reach itself, so a cell it marks is a cell
            // the bubble touches. See [`galos_index::Reach`].
            let id = mark.id;
            if tasks.0.contains_key(&id) {
                continue;
            }
            let slice = mark.slice as usize;
            let want = if whole {
                slice
            } else {
                (wanted(share, slice, id) * READ_SLACK)
                    .max(READ_LEAST)
                    .min(slice)
            };
            if want == 0 {
                continue;
            }
            let held = resident.0.cell(id).map_or(0, |held| held.points.len());
            if held >= want {
                continue;
            }
            asking.push((id, want));
        }
        asking
    };
    let _zone = info_span!("cell tasks", missing = asking.len()).entered();
    for (id, want) in asking {
        let source = transport.0.clone();
        tasks.0.insert(
            id,
            pool.spawn(
                async move {
                    // The stamp first: a payload republished between the two is
                    // then held under the older stamp and re-read by the next
                    // refresh, where the other order would hold a stamp for
                    // contents the map does not have.
                    let stamp =
                        source.stamp(Part::Cell(id)).await.ok().flatten();
                    Ok((source.payload_prefix(id, want).await?, stamp))
                }
                // One zone per cell, named with it. At info with the rest: the
                // walk is the map's live payload path, so a capture that left
                // these out would show every frame and none of the reads the
                // frames are waiting on. A view change asks for the cells it
                // newly reaches and no more — the map holds the others and
                // this loop skips what is already on the wire — so the count
                // is a view's worth of zones, not a frame's.
                .instrument(info_span!("cell payload", cell = ?id)),
            ),
        );
    }
}

/// How many systems the marked sky holds, off the index and not off what
/// has landed
///
/// The share has to be the same figure in [`fetch`] and in [`reconcile`],
/// and it must not move as payloads arrive: a share struck over what is
/// held would rise while the map was still reading and every mark already
/// drawn would shift under it. A cell's slice length is known from the
/// index the moment the walk marks it.
fn population(planned: &galos_index::Needed) -> u64 {
    let slices: u64 =
        planned.marks.iter().map(|mark| u64::from(mark.slice)).sum();
    // And the sky the merged cells stand for, which is drawn without being
    // read. Counting it holds the share down over a region the frame is
    // already marking, so zooming out past a cell's merge does not brighten
    // what is left.
    let merged: u64 = planned.blobs.iter().map(|blob| blob.count).sum();
    slices + merged
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
pub(crate) fn collect(
    mut tasks: ResMut<BoundedTasks>,
    mut resident: ResMut<ResidentCells>,
    mut orders: ResMut<PointOrders>,
    mut republished: ResMut<Republished>,
    mut held: ResMut<crate::refresh::Held>,
) {
    tasks.0.retain(|&id, task| {
        let Some(result) = block_on(future::poll_once(task)) else {
            return true;
        };
        if let Ok((points, stamp)) = result {
            adopt(&mut resident, &mut orders, &mut republished, id, points);
            held.holding(id, stamp);
        }
        false
    });
}

/// How many payload points one frame may weigh against the filters
///
/// The verdicts are a walk of every point of every resident payload, and
/// the resident set at a wide zoom is measured at 152 million points over
/// 151,619 cells. At 22.7 ns a point under a 334-stop route filter that is
/// 3.4 seconds, which is what a cut used to spend in one frame and what was
/// reported as the map hanging at the end of a long plot.
///
/// Two hundred thousand is some four milliseconds of it — a frame's worth
/// of slack rather than a frame's whole budget, the walk itself already
/// costing 22–29 ms at this scale. A cell is taken whole or not at all,
/// its list being an all-or-nothing answer about that payload.
const VERDICT_BUDGET: usize = 200_000;

/// The orders a resident cell's points are drawn in, kept until what they are
/// worked out from moves
///
/// Two of them, both walks of every point of every resident payload and both
/// answers that hold still between the same few events, which is the whole
/// reason they are kept rather than asked afresh every frame:
///
/// - Which points the filters admit, since [`reconcile`] draws those before
///   the ones they exclude. It moves when a filter is asked or lifted, when a
///   span is re-cut against the clock, when the political table is replaced by
///   a refresh, and when the payload itself is. [`Cut`] counts the first three
///   and [`adopt`] drops a cell's lists with its payload for the last.
/// - Which points anybody lives in, busiest first, for the sky read as
///   populations. Empty systems are left out rather than ordered last: in
///   that mode they are not drawn at all
///   ([`crate::systems::visibility`]), so a slot of a cell's budget spent on
///   one buys nothing. It moves with the political table and the payload,
///   which is the same [`Cut`] and the same [`adopt`].
///
/// Indices into the cell's payload rather than addresses. The admitted list is
/// ascending, so the fill can walk the payload and it together and take what
/// is in one and not the other without a set to test against; the populated list
/// is in the order it is drawn in.
#[derive(Resource, Default)]
pub(crate) struct PointOrders {
    /// The cut being worked towards
    cut: u64,
    /// Which cut each cell's lists were last taken at
    ///
    /// **A cut no longer throws the lists away.** It used to, and what
    /// followed was one frame that walked every resident payload through
    /// the filters again — measured over `.index/full`, 152 million points
    /// at 22.7 ns apiece under a 334-stop route filter, which is **3.4
    /// seconds** of frozen map. Reported as a hang at the end of plotting
    /// a long route, and that is exactly when it fires: a route landing
    /// adds a filter, which cuts.
    ///
    /// So a stale list is kept and drawn from until this frame's budget
    /// reaches its cell. What that shows is the map filtering in over a
    /// second or two rather than stopping dead, which is what every other
    /// bounded thing here already does — see
    /// [`super::spawn::SPAWN_BUDGET`].
    at: HashMap<CellId, u64>,
    cells: HashMap<CellId, Vec<u32>>,
    populated: HashMap<CellId, Vec<u32>>,
}

impl PointOrders {
    /// Drop what a new cut, or a map with nothing asked of it, has invalidated
    ///
    /// A cut carries both lists off. Nothing asked of the filters carries only
    /// the verdicts, there being no order over them to keep — who lives where
    /// is no business of the filters.
    fn hold(&mut self, cut: u64, asking: bool) {
        if self.cut != cut {
            // Noted and not acted on: what each cell holds is stale from
            // here, and [`Self::walk`] brings it forward a budget at a
            // time. See [`Self::at`].
            self.cut = cut;
        } else if !asking {
            self.cells.clear();
            self.at.clear();
        }
    }

    /// Work out whatever this cut has not asked about this cell yet
    ///
    /// Both lists in one pass over the payload, so [`reconcile`] can read
    /// either or both of them afterwards without holding this borrow open.
    /// `by_population` says whether the busiest order is wanted at all: it is
    /// a walk of the payload against the political table, and there is no
    /// sense paying for one while the sky is not being read that way.
    fn walk(
        &mut self,
        id: CellId,
        points: &[Point],
        filters: &Prepared<'_>,
        populated: &Populated,
        now: DateTime<Utc>,
        by_population: bool,
        budget: &mut usize,
    ) {
        // Whether what is held about this cell was worked out against the
        // filters as they stand. A cell nothing is held about at all is
        // stale too, this being the first time it has been reached.
        let fresh = self.at.get(&id) == Some(&self.cut);
        // What the pass may still spend, in points. Nothing left is not a
        // reason to drop what is held: a list one cut behind draws a system
        // the filters no longer admit, or misses one they now do, which is
        // a frame or two of the wrong dimming — against a map that stops
        // for seconds.
        if !fresh && *budget < points.len() {
            return;
        }
        if !fresh {
            *budget -= points.len();
            self.at.insert(id, self.cut);
            self.cells.remove(&id);
            self.populated.remove(&id);
        }
        // Nothing asked of the filters admits everything, and there is no
        // order over them worth keeping; see [`Self::admits`].
        if filters.asking() {
            self.cells.entry(id).or_insert_with(|| {
                points
                    .iter()
                    .enumerate()
                    .filter(|(_, point)| {
                        filters.admits(&candidate(point, populated), now)
                    })
                    .map(|(index, _)| index as u32)
                    .collect()
            });
        }
        if by_population {
            self.populated.entry(id).or_insert_with(|| {
                let mut order: Vec<(u64, u32)> = points
                    .iter()
                    .enumerate()
                    .filter_map(|(index, point)| {
                        let count = populated
                            .get(point.id64 as i64)
                            .map(|system| system.population)
                            .filter(|count| *count > 0)?;
                        Some((count, index as u32))
                    })
                    .collect();
                // Busiest first, and by their place in the payload where two
                // hold the same number, so the order is the same answer every
                // time rather than whatever the sort happened to do.
                order.sort_unstable_by_key(|&(count, index)| {
                    (Reverse(count), index)
                });
                order.into_iter().map(|(_, index)| index).collect()
            });
        }
    }

    /// The indices of a cell's points the filters admit, ascending
    ///
    /// Empty where nothing is asked, since then every point is admitted and an
    /// order over them says nothing. [`reconcile`]'s fill draws the whole
    /// payload in that case, which is the pass this made before the filters
    /// had a say in it. Empty, too, for a cell [`Self::walk`] has not reached.
    fn admits(&self, id: CellId) -> &[u32] {
        self.cells.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The indices of a cell's points with a population, busiest first
    ///
    /// A *set* with an order over it, which is why it is not called after
    /// the order. What it answers is who lives in this cell, off the
    /// payload — and off the payload it can only answer about the prefix
    /// that has landed, which is why the population scale reads
    /// [`super::populated::PopulatedCells`] instead and this is left to the one
    /// case that cannot: a span, which only a payload point carries a
    /// moment for.
    fn populated(&self, id: CellId) -> &[u32] {
        self.populated.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Forget a cell, its payload having been freed
    pub(crate) fn forget(&mut self, id: CellId) {
        self.cells.remove(&id);
        self.populated.remove(&id);
        self.at.remove(&id);
    }
}

/// Take `points` as a cell's payload, dropping whatever was worked out about
/// the one it replaces
///
/// The two go together and must: [`PointOrders`] holds *indices into the
/// payload*, so a list kept across a replacement names whichever systems now
/// sit at those places. A republished cell is the case — see
/// [`crate::refresh`] — and a first read is the same call with nothing to
/// forget.
///
/// The systems already drawn out of the old payload are the third thing that
/// goes with it, and the one this cannot do itself: they are entities, and
/// which of them the walk still draws is not known until it walks. So the
/// cell is noted in [`Republished`] and [`reconcile`] rebuilds it. A first
/// read notes it too and nothing comes of that, the cell having nothing drawn
/// out of it yet.
pub(crate) fn adopt(
    resident: &mut ResidentCells,
    orders: &mut PointOrders,
    republished: &mut Republished,
    id: CellId,
    points: Vec<Point>,
) {
    resident.0.insert(id, points);
    orders.forget(id);
    republished.0.insert(id);
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

/// The order a cell's populated systems are drawn in: what the filters admit first,
/// then the rest to fill what is left
///
/// The list is already busiest first — [`super::populated::PopulatedCells`] sorts
/// it once, at startup — so this only weighs the filters over it, for the
/// reason [`busiest_first`] does: the excluded are the space the admitted
/// are read against, and a cell that spent its budget on excluded systems
/// because they happen to be the busiest would draw the background and
/// leave the thing asked for off the map.
///
/// A span is not among the filters that can reach here. The populated
/// table carries no moment, so a system drawn out of it is one
/// [`Filter::Recency`] has nothing to say about — which is why
/// [`reconcile`] falls back to the payload while one is asked. See
/// [`Filters::timed`].
fn populated_first<'a>(
    here: &'a [i64],
    filters: &'a Prepared<'_>,
    populated: &'a Populated,
    now: DateTime<Utc>,
    fill: bool,
) -> impl Iterator<Item = &'a i64> + 'a {
    let admitted = move |address: &&i64| {
        !filters.asking()
            || filters.admits(
                &Candidate {
                    address: **address,
                    factions: populated
                        .get(**address)
                        .map(|system| system.factions.as_slice())
                        .unwrap_or(&[]),
                    updated_at: None,
                },
                now,
            )
    };
    let lead = here.iter().filter(admitted);
    let rest = here.iter().filter(move |address| !admitted(address));
    lead.chain(rest.take(if fill { here.len() } else { 0 }))
}

/// The order a cell's points are drawn in while the map is reading the sky as
/// populations: the busiest first, and what the filters admit ahead of what
/// they exclude.
///
/// `busiest` is [`PointOrders::busiest`]'s list, so the empty systems are
/// already out of it — in that mode they are not drawn — and a cell's budget
/// is spent on the systems that have something to say. What is left of the
/// budget after the admitted is filled with the excluded, as [`drawn_first`]
/// fills it, and `fill` false stops that half outright for the same reason.
///
/// The filters lead the population for the reason they lead the pointer
/// (see [`crate::systems::pointing`]): the excluded are the space the
/// admitted are read against, and a cell that spent its whole budget on
/// excluded systems because they happen to be the busiest would draw the
/// background and leave the thing asked for off the map.
///
/// `asking` is what tells the two readings of an empty `admits` apart, and it
/// has to be handed in rather than inferred: [`PointOrders::walk`] keeps no
/// list at all while nothing is asked, and an empty one for a cell whose
/// points a filter excludes to the last. Read as "everything admitted", the
/// second becomes a whole cell offered as though it had been asked for — and
/// under a dim of zero every one of those is refused by
/// [`super::spawn::spawn_systems`], never becomes an entity, and is queued
/// again the next frame off a budget the admitted elsewhere needed.
fn busiest_first<'a>(
    busiest: &'a [u32],
    admits: &'a [u32],
    asking: bool,
    fill: bool,
) -> impl Iterator<Item = usize> + 'a {
    let admitted =
        move |index: &&u32| !asking || admits.binary_search(index).is_ok();
    let lead = busiest.iter().filter(admitted);
    let rest = busiest.iter().filter(move |index| !admitted(index));

    lead.chain(rest.take(if fill { busiest.len() } else { 0 }))
        .map(|&index| index as usize)
}

/// What the frame's marks came to, for the diagnostics panel
///
/// The marks' counterpart to [`super::glow::Laid`]: what the frontier asked
/// of every marked, resident, in-reach cell, how much of it the map has and
/// drew, and how many merged marks stand over the rest of the sky.
///
/// There is no share here and there is nothing to divide by. The walk's ask
/// is already one mark to every [`galos_index::MERGE_PX`] squared of the cell
/// covers, so what bounds the frame is the frame's own area and not a factor
/// struck across every cell — see [`galos_index::Index::walk_screen`]. What
/// this reports is therefore a fact about the view rather than a dial: if
/// `drawn` runs far under `wanted` the payloads have not landed, not that
/// the frame refused them.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct Sampled {
    /// Systems the frame is spread over: every marked, resident, in-reach
    /// cell's payload, plus what the merged cells stand for.
    pub population: u64,
    /// The share of them the frame draws.
    pub share: f32,
    /// Marks actually taken out of the payloads.
    pub drawn: usize,
    /// Cells drawn as one merged mark apiece.
    pub blobs: usize,
    /// How many of those are marks lighting a tile the rest of the frame
    /// left dark, rather than marks the share drew. See
    /// [`galos_index::screen::Empty`].
    pub lit: usize,
    /// How many systems those merged marks stand for.
    pub behind: u64,
}

/// Draw the prefix of every marked cell the frontier asks for, admitted
/// systems first, grown and shed per system as the camera moves
///
/// **The walk says how many, and there is nothing to divide.** A cell's
/// payload is magnitude-ordered and [`galos_index::MarkRef::wanted`] is how
/// many of it the cell's footprint holds apart at [`galos_index::MERGE_PX`]
/// to every merge distance squared of the patch of screen the cell's
/// contents cover. Drawing that many, and only that many, is what lets a
/// cell fill in and empty one system at a time rather than switching on
/// whole, and what keeps two neighbours at the same density however
/// differently their boxes fell.
///
/// What the sky *under* those marks comes to is the walk's too: everything
/// finer than one mark is merged into [`galos_index::BlobRef`]s, drawn by
/// [`super::field`] off the aggregates with no payload at all. So this pass
/// no longer has an overrun to spend. It used to: the per-cell rule bounds a
/// cell and not a frame, the marked prefixes of a wide view came to tens of
/// times what a frame could show, and every cell was thinned by one global
/// `fair_share` with each system holding a fixed place in it and the share
/// carried across cell faces to keep the density from stepping. All of that
/// is gone with the floor that made it necessary.
///
/// *Which* systems fill the count is the filters' to say. The count is a
/// budget of marks the screen can tell apart, worked out from the cell's own
/// footprint and not from which systems are chosen, so spending it on what
/// the filters admit draws exactly as many marks as before, no closer
/// together. Taking the brightest of the payload instead spends the budget
/// on whatever happens to be bright: a faction is a handful of systems in a
/// cell of thousands, so a filter on one used to draw nothing at all from
/// most cells while the marks the screen could carry went unused.
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
/// and not a rank range off the walk's ask, or the glow will double the
/// light of every system the filters promoted into the budget.
///
/// The prefix is pushed to the shared spawn queue, which builds only the
/// systems not already on the map, and everything outside every cell's prefix
/// is queued to drop.
///
/// Three things are spared: the system the camera is standing in, since its
/// `FloatingOrigin` hangs under it; a route's stops, which are how the way on
/// is found; and
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
pub(crate) fn reconcile(
    cameras: Query<(&OrbitCamera, &Camera)>,
    resident: Res<ResidentCells>,
    populated: Res<Populated>,
    names: Res<Names>,
    holding: Res<HeldSystem>,
    spyglass: Res<Spyglass>,
    view_mode: Res<View>,
    selection: Res<crate::systems::selection::Selection>,
    filtering: Filtering,
    cut: Res<Cut>,
    scale_population: Res<ScalePopulation>,
    mut worked: Worked,
    systems: Query<(Entity, &System, Has<crate::systems::route::Hop>)>,
    mut pending: ResMut<PendingSpawns>,
    mut evictions: ResMut<PendingEvictions>,
) {
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = crate::systems::aggregate::view(orbit, camera) else {
        return;
    };
    let Worked {
        ref populated_cells,
        ref standing,
        ref mut orders,
        ref mut republished,
        ref mut keeping,
        ref planned,
        ref mut drawn,
        ref mut sampled,
        ref mut blobs,
    } = worked;
    // Afresh each pass. A cell that has stopped being marked, been evicted or
    // fallen outside the bubble accounts for nothing now, and an account left
    // standing would go on subtracting marks that are no longer drawn — a hole
    // in the field exactly where the map has stopped drawing anything at all.
    drawn.0.clear();
    // What the frontier asks of each cell, for this pass and for the evictor
    // after it. Rebuilt only where the plan has moved, which a still camera
    // never does.
    //
    // The merge distance the ask was worked out at is the walk's: a mark's
    // own width in [`galos_index::Mode::Shell`] and the point spread in
    // [`galos_index::Mode::Real`], which is why a cluster stays a field of
    // stars in the sky where the map would collapse it to one mark.
    if planned.is_changed() {
        let _zone = info_span!("marked set").entered();
        keeping.marks(&planned.0.marks);
    }
    let now = Instant::now();
    // Clearing, the spyglass clamps the drawn set to a bubble about the camera:
    // the LOD is untouched inside it, only the far tail is shed.
    let bubble = reach(&spyglass);

    // The three phases of the pass, each its own zone: what it gathers about
    // what is already drawn, the walk of every resident cell, and the scan
    // that decides what goes. The system's own zone is all three together,
    // which is not enough to act on.
    //
    // The entity beside the address, not the address alone. What the walk
    // resolves is tens of thousands of points a frame and what is drawn is a
    // couple of thousand entities, so the pass used to hash every resolved
    // *address* into a wanted set to answer a question only the drawn ones
    // could be asked — measured at 196 ms of a 512 ms flight
    // ([`super::flight`]). Carrying the entity here means a resolved point
    // that is already drawn marks its entity ([`EntityHashSet`], bevy's own
    // numbering and a cheap hash) and one that is not goes to the queue,
    // which is one lookup a point rather than two.
    // Hashed quickly and not securely. This is asked once per point the
    // pass draws — tens of thousands a frame — and the key is a system
    // address, which is a bit-packed position. Measured over
    // `.index/full`, SipHash over it was more than half of what the whole
    // reconciliation pass cost: 1.31 ms a frame against 0.57 with the
    // lookup taken out entirely.
    let existing: rustc_hash::FxHashMap<i64, Entity> = {
        let _zone = info_span!("reconcile setup").entered();
        systems
            .iter()
            .map(|(entity, system, _)| (system.address, entity))
            .collect()
    };
    let picked: HashSet<i64> = selection.addresses().into_iter().collect();
    // Every stop of every route being shown. A line is only a line if it has
    // both ends of each leg to draw between, so these are wanted whatever the
    // walk resolves and wherever the bubble ends. See [`Filters::routed`].
    let routed = filtering.filters.routed();

    // With nothing asked every system is admitted, so there is no order to
    // impose: the admitted lists are dropped and the fill draws the payload in
    // its own order, which is what this did before the filters had a say.
    let asking = filtering.filters.asking();
    orders.hold(cut.0, asking);
    // Whether a cell's budget is spent on the systems with a population, which is
    // what the sky says while it is read that way. The cells are the walk's to
    // choose either way — that is the index's own business and it knows
    // nothing of who lives where — so what this settles is which of a held
    // cell's systems are drawn out of it.
    let by_population = by_population(&view_mode, &scale_population);
    // What this frame may spend working out afresh what the filters admit.
    // Spent down by the loop below and not refilled inside it: a cut leaves
    // every resident cell stale at once, and a pass that walked them all
    // would be the three-and-a-half-second hang this bounds. See
    // [`VERDICT_BUDGET`].
    let mut verdicts = VERDICT_BUDGET;
    // The addresses the filters name, gathered once for the pass rather
    // than walked per point: a route of three hundred stops is what made
    // having one on the map cost seven times what any other filter does.
    // See [`Filters::prepared`].
    let asked_for = filtering.filters.prepared();
    // Whether the excluded are wanted on screen at all. Below the dim they are
    // never spawned ([`super::spawn`]) and queued to drop by this pass, so
    // queueing them is a slot of the spawn budget spent on a system that
    // cannot land and rebuilt again next frame.
    let fill = !asking || filtering.excluded_are_drawn();
    // One clock for the pass, as the spawn batch takes one: a span's near edge
    // moves by a frame's worth in a frame.
    let wall = Utc::now();

    // What the frame has to spend and what it is spread over. A share of
    // the population is a share of every *other* marked cell's too, so the
    // whole has to be known before any of it is spent — which is a sum
    // over the plan's own counts and touches neither the index nor a
    // payload.
    let population = population(&planned.0);
    let share = share(population, frame_marks(&view));
    // Where this pass takes its systems from. Drawing by population draws
    // the systems anybody lives in, and every one of those is resident in
    // full — so the cell's own populated systems answer, exactly and the same however
    // the eye arrived, where a payload prefix answers with the busiest of
    // whatever happened to land. See [`super::populated::PopulatedCells`].
    //
    // Unless a span is asked. A moment is a fact only a payload point
    // carries, so a system taken off the populated table is one a span can
    // say nothing about; while one is on the map this falls back to the
    // payload, hysteresis and all. A moment per populated row would close
    // it.
    let from_the_table =
        by_population && !filtering.filters.asking_a_span();
    // And what it is spread over, where it draws the populated. A share
    // the *systems* in view is the wrong denominator for a mode that
    // of the *systems* in view is the wrong denominator for a mode
    // that draws none but the populated: it is thousandths where they
    // are tens, so a cell holding twenty colonies was asked for one and
    // the other nineteen went undrawn — measured over `.index/full` from
    // two hundred light years out, 31 of the 165 inhabited systems
    // within twenty-five light years of the camera, `ALPHA CENTAURI`
    // among them.
    //
    // Counted over the table rather than over the plan, because the
    // plan's cells nest: every cell from the root to the frontier holds
    // its whole subtree's populated systems, and summing those counts
    // the same
    // colony a dozen times. 148,199 rows and a distance apiece, on the
    // frames the plan moves.
    let populated_in_reach = |orbit: &OrbitCamera, bubble: Option<f64>| {
        populated
            .0
            .values()
            .filter(|system| {
                bubble.is_none_or(|radius| {
                    orbit.center().distance(DVec3::new(
                        f64::from(system.position[0]),
                        f64::from(system.position[1]),
                        f64::from(system.position[2]),
                    )) <= radius
                })
            })
            .count() as u64
    };
    let populated_share = match from_the_table {
        false => 0.,
        true => {
            let _zone = info_span!("the populated in reach").entered();
            crate::systems::bounded::share(
                populated_in_reach(orbit, bubble),
                frame_marks(&view),
            )
        }
    };
    // Which patches of sky the frame leaves dark, and the one mark each
    // of them lights. A pass of its own over the plan, before anything is
    // drawn, because the question is about the frame as a whole: a tile is
    // dark only once every mark and every merged mark has had its say. It
    // reads no payload and asks the filters nothing — a take off the
    // plan's own counts and a projection apiece. See [`Empty`].
    //
    // Cells are binned at their own centroid rather than at each mark they
    // draw. A cell above the frontier spreads its marks over the patch it
    // covers, so this can call a tile dark that one of them landed in —
    // which lights one mark that was not needed, of the at most one a tile
    // this may light at all.
    let lit: Vec<u32> = {
        let _zone = info_span!("dark tiles").entered();
        let mut lighting = Empty::over(&view);
        for (offer, mark) in planned.0.marks.iter().enumerate() {
            if !in_reach(mark.id, orbit, bubble) {
                continue;
            }
            match wanted(share, mark.slice as usize, mark.id) {
                0 => lighting.offered(
                    &view,
                    mark.at,
                    u64::from(mark.slice),
                    offer as u32,
                ),
                take => lighting.drew(&view, mark.at, take),
            }
        }
        for (offer, blob) in planned.0.blobs.iter().enumerate() {
            if !in_reach(blob.id, orbit, bubble) {
                continue;
            }
            let offer = (planned.0.marks.len() + offer) as u32;
            match wanted(share * blob.blend, blob.count as usize, blob.id) {
                0 => lighting.offered(&view, blob.at, blob.count, offer),
                _ => lighting.drew(&view, blob.at, 1),
            }
        }
        lighting.lit()
    };
    // Whether a plan entry is one of them, walked alongside the plan
    // rather than looked up: both are in the plan's own order.
    let mut lighting = lit.iter().copied().peekable();
    let mut is_lit = move |offer: u32| {
        while lighting.peek().is_some_and(|&lit| lit < offer) {
            lighting.next();
        }
        lighting.next_if_eq(&offer).is_some()
    };
    let mut took_all = 0usize;
    let mut behind = 0u64;

    // Marked, not merely held. A payload outlives the marking by [`KEEP`]
    // now (see [`evict_payloads`]), and drawing one the walk has stopped
    // marking would put the far, faint sky the walk just shed back on the
    // map — the level of detail comes from the marks and nowhere else.
    let mut wanted_by: EntityHashSet = EntityHashSet::default();
    // One buffer for every cell's take rather than one allocation apiece:
    // a wide view walks thousands of cells a frame, and the indices taken are
    // a share's worth each.
    // What a cell draws: an address, where it stands, and the payload
    // index where a payload is what named it. The two sources — a cell's
    // magnitude-ordered payload and the resident populated table — answer
    // in different terms and everything after this is the same for both.
    let mut taken: Vec<(i64, [f64; 3], Option<u32>)> = Vec::new();
    // Which of the populated the pass has already taken
    //
    // **A cell answers with its whole subtree's populated systems, and
    // the marked
    // cells nest.** Every cell from the root down to the frontier draws
    // its own marks, so without this each of them offers the same
    // busiest few over again: the budget goes on one handful drawn five
    // times and nothing deeper is ever reached. Measured over
    // `.index/full` from two hundred light years out, `ALPHA CENTAURI` —
    // a population of a hundred thousand, four light years from the
    // camera — was
    // not drawn at all, while the pass spent 2,496 marks where the
    // ordinary sky spent 2,562.
    //
    // Taken once, and the deeper cells fill in behind the shallower: a
    // cell takes the busiest of its own the pass has not reached yet,
    // which over the nest comes to the busiest first and then down.
    let mut took_populated: FxHashSet<i64> = FxHashSet::default();
    // The walk's offers are this pass's: what the last one offered and the
    // budget never reached is gone, and what is still wanted is offered again
    // below. See [`super::spawn::PendingSpawns`].
    pending.opening(now);
    // Whether the pass may still offer. Past the offer budget it goes on
    // marking what is wanted — the eviction scan reads that — and stops
    // looking for work the frame cannot do.
    let mut offering = true;
    // Over the plan's marks and not over everything the map holds. The
    // two differ by whatever [`KEEP`] is still holding onto and by
    // everything outside the bubble, and at a wide zoom that is twice the
    // set: measured over `.index/full`, 117,274 payloads held against
    // 60,229 the plan marks. A held cell the plan does not name draws
    // nothing, so walking it only to skip it is the pass done twice.
    let prefixes =
        info_span!("cell prefixes", cells = planned.0.marks.len()).entered();
    for (offer, mark) in planned.0.marks.iter().enumerate() {
        let id = mark.id;
        // **Asked before the payload is looked up.** Most marked cells
        // draw nothing at a wide zoom — the share is thousandths and a
        // cell's slice is hundreds — and the lookup is a random probe
        // into a hundred thousand entries, which is a cache miss and the
        // most expensive thing in the pass. Off the mark's own slice,
        // which the walk carried here for exactly this: measured over
        // `.index/full` at sixty thousand light years out, 77,773 marks
        // against 10,055 that draw.
        //
        // The whole slice and not what has landed, so what is drawn does
        // not grow as the read arrives: a share struck over the prefix in
        // hand would ask for less of a cell the moment less of it was
        // held, and every mark would shift as the payloads came in.
        // A cell the share draws nothing of still draws one mark where
        // nothing else in the frame reaches its patch of sky: the
        // brightest it holds, which is the head of its payload. See
        // [`Empty`] — and the payload is in hand for it, every marked cell
        // in reach being read to [`READ_LEAST`] whatever its share.
        // A share of the populated where those are what is drawn, and
        // a share of the cell's own slice otherwise.
        let asked = match from_the_table {
            true => wanted(
                populated_share,
                populated_cells.of(id).len(),
                id,
            ),
            false => wanted(share, mark.slice as usize, id),
        }
        .max(usize::from(is_lit(offer as u32)));
        if asked == 0 {
            continue;
        }
        // Taken rather than walked lazily, since the two sources are
        // different iterators and what follows is the same for both: an
        // address, where it stands, and the payload index where there is
        // one. A share's worth apiece, which is a few.
        taken.clear();
        // Whether this cell's payload is the one the drawn systems were
        // built from, or a later one; see [`Republished`]. Nothing to
        // settle where no payload was read.
        let mut refreshed = false;
        if from_the_table {
            // **Off the resident table and not off a payload.** This mode
            // draws the systems anybody lives in, and every one of those
            // is resident in full; a payload prefix holds one in
            // forty-four of them, scattered, so the busiest of a prefix is
            // not the busiest of the cell. See
            // [`super::populated::PopulatedCells`].
            let here = populated_cells.of(id);
            if here.is_empty() {
                continue;
            }
            let fresh: Vec<i64> =
                populated_first(here, &asked_for, &populated, wall, fill)
                    .filter(|address| !took_populated.contains(address))
                    .take(asked)
                    .copied()
                    .collect();
            for address in fresh {
                let Some(system) = populated.get(address) else { continue };
                took_populated.insert(address);
                taken.push((
                    address,
                    [
                        f64::from(system.position[0]),
                        f64::from(system.position[1]),
                        f64::from(system.position[2]),
                    ],
                    None,
                ));
            }
        } else {
            let Some(cell) = resident.0.cell(id) else { continue };
            let target = asked.min(cell.points.len());
            if target == 0 {
                continue;
            }
            refreshed = republished.holds(id);
            orders.walk(
                id,
                &cell.points,
                &asked_for,
                &populated,
                wall,
                by_population,
                &mut verdicts,
            );
            let admits = orders.admits(id);
            let order: Vec<usize> = if by_population {
                busiest_first(orders.populated(id), admits, asking, fill)
                    .take(target)
                    .collect()
            } else {
                drawn_first(&cell.points, admits, fill).take(target).collect()
            };
            for index in order {
                let point = &cell.points[index];
                taken.push((
                    point.id64 as i64,
                    point.pos,
                    Some(index as u32),
                ));
            }
        }
        // What this cell's marks account for, so [`super::glow`] can lay the
        // rest of it down and not the whole. Built here because here is the
        // only place the drawn set is known: it is not a rank range, the
        // filters having promoted systems out of magnitude order, and it is
        // cut again per point by the bubble just below.
        let mut took = Accounted::default();
        for &(address, pos, index) in &taken {
            // A cell straddling the bubble draws only the points inside it, so
            // the edge is a sphere about the camera, not the cell grid.
            if let Some(radius) = bubble
                && orbit.center().distance(DVec3::from(pos)) > radius
            {
                continue;
            }
            took_all += 1;
            // Counted before it is queued rather than after it is spawned: a
            // system the budget has not reached yet is one the field would
            // otherwise go on drawing for the frame or two it takes to land,
            // and a mark arriving over light that is already there reads as a
            // flash. Accounting for it now hands the light over on the frame
            // the walk decides, and the spawn catches up under it.
            took.took(
                pos,
                populated
                    .get(address)
                    .filter(|system| system.population > 0)
                    .map(|system| {
                        Inhabited::of_system(
                            pos,
                            system.allegiance,
                            system.government,
                            system.security,
                        )
                    }),
            );
            // Already drawn is already answered, except out of a cell that
            // has just been published again: then the system on the map was
            // built from the payload this one replaced, and what it says
            // about the moment, the magnitude and the politics is what the
            // index said last time. Queued either way, and `spawn_systems`
            // replaces it in place.
            // Which point of which cell where a payload named it: most of
            // what a walk offers is never drawn, and building it to queue
            // it is a name and a political join thrown away. See
            // [`super::spawn::Waiting`].
            //
            // A system off the populated table has no payload point to
            // name and is built here instead — the path a route's own
            // stops take. It costs what it costs because the set is
            // small: the whole galaxy holds 148,199 systems anybody lives
            // in, and a frame in this mode draws tens.
            //
            // Built from the row and not through `system_at`, which
            // refuses a system the names table has no row for. A name is
            // one thing a system may be missing and being drawn is
            // another: the payload path names an unnamed system by its
            // address ([`build_system`]) and draws it, and a mode that
            // silently dropped the same system would be a hole in the
            // sky wherever the two tables disagree.
            let queue = |pending: &mut PendingSpawns| match index {
                Some(index) => pending.offer(address, id, index),
                None => {
                    let raw = RawSystem {
                        address,
                        position: pos,
                        magnitude: None,
                        temp_bucket: None,
                        // A moment is a payload's to carry; see
                        // [`Filters::asking_a_span`].
                        updated_at: None,
                    };
                    let system = build_system(&raw, &populated, &names);
                    pending.push(system, false, true, now);
                    true
                }
            };
            match existing.get(&address) {
                Some(&entity) => {
                    wanted_by.insert(entity);
                    if refreshed && offering {
                        offering = queue(&mut pending);
                    }
                }
                None => {
                    if offering {
                        offering = queue(&mut pending);
                    }
                }
            }
        }
        drawn.0.insert(id, took);
        if refreshed {
            republished.settled(id);
        }
    }
    drop(prefixes);
    // And the cells the walk merged, which need nothing read. One mark
    // apiece and no more — a blob is a cell whose whole contents fall inside
    // one mark, so one is what it is worth — and it is drawn or not on the
    // same share every read cell is thinned by, so a merged region is no
    // denser or thinner on screen than a read one beside it.
    blobs.0.clear();
    let merged =
        info_span!("merged cells", cells = planned.0.blobs.len()).entered();
    for (offer, blob) in planned.0.blobs.iter().enumerate() {
        if !in_reach(blob.id, orbit, bubble) {
            continue;
        }
        // What it stands for is the weighing's to say: its whole subtree
        // ordinarily, and only the systems anybody lives in where the sky
        // is read as populations. A merged mark over a cell nobody lives
        // in stands for nothing in that mode and is not drawn.
        let (light, admitted, stands_for) = standing
            .of(offer)
            .unwrap_or((Vec3::splat(f32::NAN), 1., blob.count));
        let drawn = wanted(share * blob.blend, stands_for as usize, blob.id)
            > 0
            || is_lit((planned.0.marks.len() + offer) as u32);
        if !drawn || stands_for == 0 {
            continue;
        }
        // What the filters make of it: the share of its systems they
        // admit, spent exactly as those systems' own marks would spend
        // it. A cell with three of ten thousand admitted would draw three
        // marks at full and 9,997 at the dim if it split, so the one mark
        // it is drawn as is worth their average — which leaves it at the
        // dim, without ever claiming the cell is empty. See
        // [`super::merged::Standing`].
        let dim = match fill {
            true => filtering.dim.opacity(),
            // Below the dim an excluded system is not drawn at all, so
            // neither is the share of a mark that stands for one.
            false => 0.,
        };
        let fade = admitted + (1. - admitted) * dim;
        if fade <= 0. {
            continue;
        }
        behind += stands_for;
        blobs.0.push(Blob {
            light,
            fade,
            id: blob.id,
            count: blob.count,
            at: blob.at,
            m_min: blob.m_min,
        });
    }
    drop(merged);

    sampled.set_if_neq(Sampled {
        population,
        share: share as f32,
        drawn: took_all,
        blobs: blobs.0.len(),
        lit: lit.len(),
        behind,
    });

    // The route's own stops, which no cell prefix answers for. They lie
    // wherever the route goes rather than near the camera, so from far enough
    // out to see the whole of a route most of them fall outside every prefix
    // and outside the bubble both, and the walk would never build them.
    //
    // Wanted, which is the one thing said here: it is what builds the stops
    // the map has not got and, below, what keeps the ones it has. Read out of
    // the resident names table, as a searched system is, and pinned so the
    // queue does not weigh them against the reach and forget them unread.
    // Asked for, too: a stop is a system named by hand, and it is drawn
    // before the galaxy of marks this same walk has just offered.
    //
    // Being on the map is not being in view. A stop the spyglass does not
    // reach is hidden by [`crate::systems::visibility`] and the line is cut
    // back to it by [`crate::systems::route::trim`]; what this settles is
    // that the stop is there to be reached at all.
    for &address in &routed {
        match existing.get(&address) {
            Some(&entity) => {
                wanted_by.insert(entity);
            }
            None => {
                if let Some(system) = system_at(address, &populated, &names) {
                    pending.push(system, true, true, now);
                }
            }
        }
    }

    // Everything outside every prefix goes: the tail a cell sheds as it
    // recedes, the systems of a cell whose payload has been freed, and —
    // clearing — whatever fell outside the bubble above. Written whole, so a
    // system the walk has taken back is not still down for eviction.
    let _zone = info_span!("evict scan", wanted = wanted_by.len()).entered();
    evictions.0 = systems
        .iter()
        .filter(|(entity, system, hop)| {
            if Some(*entity) == holding.of()
                || *hop
                || picked.contains(&system.address)
            {
                return false;
            }
            // Unwanted now, not unwanted twice. A grace here was measured and
            // dropped: over one flight ([`super::flight`]) it changed the
            // despawn count by 588 in 401,857 — the turnover is the level of
            // detail moving, not systems flickering on the threshold — and
            // the frames it took to hold the extra systems cost 23% of the
            // frame at the median.
            !wanted_by.contains(entity)
        })
        .map(|(entity, ..)| entity)
        .collect();
}

/// Free the payloads the walk has stopped wanting, once it has stopped
/// wanting them for long enough
///
/// A payload is wanted where the walk marks its cell and the clamp still
/// reaches it. It used to be freed the moment either stopped being true,
/// which reads as the obvious rule and is the expensive one: a zoom marks a
/// different set every frame, so cells left and came back, and a payload
/// freed on one frame was read from disk again two frames later. Measured
/// over one flight ([`super::flight`]), seventy per cent of the reads were of
/// cells already read once, and each re-read rebuilt the systems in it.
///
/// So an unwanted payload is kept for [`KEEP`], and the held set is allowed
/// to run to [`SLACK`] times what the view marks. Past that ceiling the least
/// recently wanted go first, however new they are, which is what keeps a
/// grace from being a leak: the memory held stays proportional to the view,
/// as it was when the rule was immediate.
///
/// Their entities are not this system's business. [`reconcile`] draws the
/// marked cells alone, so a held-but-unmarked payload is one nothing draws
/// from — the systems in it are outside every prefix and queued to drop on
/// the frame the walk stops marking it, exactly as before. What is deferred
/// here is the reading, not the drawing.
pub(crate) fn evict_payloads(
    planned: Res<Planned>,
    spyglass: Res<Spyglass>,
    cameras: Query<&OrbitCamera>,
    time: Res<Time<Real>>,
    mut resident: ResMut<ResidentCells>,
    mut orders: ResMut<PointOrders>,
    mut held: ResMut<crate::refresh::Held>,
    mut keeping: ResMut<Keeping>,
    mut swept: Local<Option<Instant>>,
) {
    let now = time.last_update().unwrap_or_else(|| time.startup());
    // **Not every frame.** This is a pass over everything the map holds,
    // which at a wide zoom is a hundred thousand payloads and three
    // milliseconds — and what it decides is measured against [`KEEP`],
    // which is two seconds. Running it sixty times inside every one of
    // those is sixty answers to a question that can only change once.
    //
    // Swept on a plan change, since that is what moves a cell in or out
    // of the wanted set, and on a clock besides, since the grace expires
    // on its own while nothing at all is happening. A quarter of a second
    // is an eighth of the grace: the ceiling holds the memory either way,
    // and what this costs is the freeing running late by a frame or two.
    let due = swept.is_none_or(|last| {
        now.saturating_duration_since(last) >= SWEEPS
    });
    if !planned.is_changed() && !due {
        return;
    }
    *swept = Some(now);
    let bubble = reach(&spyglass).zip(cameras.single().ok());

    // One pass over what is held: stamp what is wanted now, take what has
    // been unwanted past the grace, and note the rest with the stamp the
    // ceiling sorts on. The marked set is [`Keeping`]'s, built once a plan by
    // [`fetch`], so this is a lookup a held cell and no set to build.
    let _zone = info_span!("keep or free", cells = resident.0.len()).entered();
    let mut freeing: Vec<CellId> = Vec::new();
    let mut spare: Vec<(Instant, CellId)> = Vec::new();
    for (id, _) in resident.0.iter() {
        let wanted = keeping.marks_it(id)
            && bubble.is_none_or(|(radius, camera)| {
                cell_in_reach(id, camera.center(), radius)
            });
        if wanted {
            keeping.wanted(id, now);
            continue;
        }
        let last = keeping.last(id, now);
        if now.saturating_duration_since(last) >= KEEP {
            freeing.push(id);
        } else {
            spare.push((last, id));
        }
    }

    // And the ceiling, over whatever the grace left standing: the oldest
    // stamps first, so what goes is what has gone longest without being asked
    // for.
    //
    // Measured off the marked set, which `MARK_LEAST` is what bounds: the
    // walk marks a cell only where it is worth a handful of marks, so the
    // set is a view's worth of cells rather than the sky. It is a wider set
    // than the share test it replaced — measured over `.index/full`, 10,343
    // cells against 2,432 from two thousand light years out — so the
    // residency this sizes is wider with it, deliberately: reading the thin
    // cells is what stops a region being drawn a box at a time.
    let budget = planned.0.marks.len().saturating_mul(SLACK);
    let holding = resident.0.len() - freeing.len();
    if holding > budget {
        spare.sort_unstable_by_key(|(last, _)| *last);
        freeing.extend(spare.iter().take(holding - budget).map(|(_, id)| *id));
    }

    for id in freeing {
        resident.0.remove(id);
        orders.forget(id);
        held.forget(id);
        keeping.forget(id);
    }
}

/// One payload point as a drawable system: placed where the payload puts it,
/// named and colored off the resident tables
///
/// The position comes straight from the payload, in light years — finer than
/// the names table's whole-light-year placement, and present for every system,
/// named or not. The name and the political columns are the same join a
/// system named by hand gets, keyed by the point's id.
///
/// The one place a point becomes a system, so the payload's
/// [`Point::updated_at`] is read into a moment here rather than at each
/// caller. Unix seconds on the wire and a moment on the map: the payload
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
    use crate::systems::route::graph::{Drive, Routing, Tuning};
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
            kind: galos_index::StarKind::G,
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
            kind: galos_index::StarKind::G,
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
            kind: galos_index::StarKind::G,
        }
    }

    /// A view of `wide` by `high` pixels, looking down `-z`
    /// Where a test system stands: spread around the camera rather than
    /// strung out along one ray from it, as a real sky is.
    fn placed(id: i64) -> [f64; 3] {
        let turn = id as f64 / 8. * std::f64::consts::TAU;
        [10. * turn.cos(), 10. * turn.sin(), 0.]
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
    /// At a dim of zero [`super::spawn`] refuses them and [`reconcile`]
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
        use crate::systems::filter::{Filter, Filters};
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

        let mut held = PointOrders::default();
        let mut budget = VERDICT_BUDGET;
        held.hold(1, true);
        held.walk(
            id,
            &points,
            &filters.prepared(),
            &populated,
            now,
            false,
            &mut budget,
        );
        assert_eq!(
            held.admits(id),
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
        held.walk(
            id,
            &points,
            &filters.prepared(),
            &populated,
            now,
            false,
            &mut budget,
        );
        assert_eq!(
            held.admits(id),
            &[0, 2, 3],
            "the faction's, and the two picked out by hand"
        );

        // Nothing asked admits everything, so there is no order to hold.
        held.hold(2, false);
        held.walk(
            id,
            &points,
            &Filters::default().prepared(),
            &populated,
            now,
            false,
            &mut budget,
        );
        assert!(held.admits(id).is_empty());
    }

    /// A cut spends a budget rather than a frame
    ///
    /// **The reported hang.** A cut used to throw every cell's verdicts
    /// away, and the next frame walked every resident payload through the
    /// filters again: measured over `.index/full`, 152 million points at
    /// 22.7 ns apiece under a 334-stop route filter, which is 3.4 seconds
    /// of a frozen map. It fired at the end of plotting a long route,
    /// because a route landing adds a filter and a filter added cuts.
    ///
    /// So a cell past the budget keeps the list it has and is brought
    /// forward on a later frame. What that costs is a frame or two of the
    /// wrong dimming on the cells at the back of the queue; what it buys
    /// is a map that filters in rather than stopping.
    #[test]
    fn a_cut_is_worked_through_a_budget_at_a_time() {
        use crate::systems::filter::{Filter, Filters};

        let points: Vec<Point> = (1..=10).map(point).collect();
        let cells = [CellId::ROOT, CellId { level: 1, x: 1, y: 0, z: 0 }];
        let populated = Populated::default();
        let now = Utc::now();
        let mut filters = Filters::default();
        filters.add(Filter::Systems { label: "one".into(), systems: vec![3] });

        // Room for one cell's payload and no more, which is the shape of
        // every frame after a cut: far more stale cells than budget.
        let mut held = PointOrders::default();
        held.hold(1, true);
        let mut budget = points.len();
        for id in cells {
            held.walk(
                id,
                &points,
                &filters.prepared(),
                &populated,
                now,
                false,
                &mut budget,
            );
        }
        assert_eq!(held.admits(cells[0]), &[2], "the first cell was not read");
        assert!(
            held.admits(cells[1]).is_empty(),
            "the second cell was read past the budget"
        );

        // And the next frame's budget reaches it, the cut not having moved.
        let mut budget = points.len();
        held.walk(
            cells[1],
            &points,
            &filters.prepared(),
            &populated,
            now,
            false,
            &mut budget,
        );
        assert_eq!(held.admits(cells[1]), &[2], "it never caught up");

        // A cut leaves what is held standing, so the map draws the last
        // answer while the new one is worked out. Nothing to spend here,
        // which is the frame a cut lands on.
        filters
            .add(Filter::Systems { label: "another".into(), systems: vec![5] });
        held.hold(2, true);
        let mut budget = 0;
        held.walk(
            cells[0],
            &points,
            &filters.prepared(),
            &populated,
            now,
            false,
            &mut budget,
        );
        assert_eq!(
            held.admits(cells[0]),
            &[2],
            "a cut threw the old verdicts away instead of keeping them"
        );
    }

    /// The clamp is the spyglass reach, and only while it is clearing
    ///
    /// Clearing, the walk is cut off at the reach; not clearing, it runs to the
    /// whole sky and the clamp stands down — the bound never switches the LOD
    /// off, only where it ends.
    #[test]
    fn the_clamp_is_the_reach_only_while_clearing() {
        let mut spyglass = Spyglass {
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

    /// A world with the walk holding nothing, so every system is out of reach
    /// of every prefix and only what is spared survives
    fn walking() -> App {
        let mut app = App::new();
        // The populated table gathered ahead of the draw, as the map
        // gathers it: what the population scale draws comes from there
        // and not from a payload. See [`super::populated`].
        app.add_systems(
            Update,
            (crate::systems::populated::gather, reconcile).chain(),
        );
        app.init_resource::<PendingEvictions>();
        app.init_resource::<PendingSpawns>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<HeldSystem>();
        app.init_resource::<crate::systems::selection::Selection>();
        app.init_resource::<crate::systems::filter::Filters>();
        app.init_resource::<crate::systems::filter::DimTo>();
        app.init_resource::<crate::systems::filter::Cut>();
        app.init_resource::<PointOrders>();
        app.init_resource::<Republished>();
        app.init_resource::<Keeping>();
        app.init_resource::<Sampled>();
        app.init_resource::<Blobs>();
        app.init_resource::<crate::systems::merged::Standing>();
        app.init_resource::<crate::systems::populated::PopulatedCells>();
        app.init_resource::<crate::systems::aggregate::Drawn>();
        app.insert_resource(crate::ResidentIndex(galos_index::Index::default()));
        app.insert_resource(Populated::default());
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.insert_resource(View::Map);
        app.insert_resource(ScalePopulation(false));
        app.insert_resource(Spyglass {
            radius: 50.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(Planned(galos_index::Needed {
            mode: galos_index::Mode::Shell,
            marks: Vec::new(),
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        app.world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()));
        app
    }

    /// Hold every payload a build published, and mark every cell it holds
    ///
    /// Both halves, because the walk draws the cells the plan marks and holds
    /// the payloads of those it has read: a test that filled one and not the
    /// other would be a map holding a galaxy nothing marks. See
    /// [`evict_payloads`].
    fn holding(app: &mut App, built: &galos_index::Snapshot) {
        let mut marks = Vec::new();
        {
            let mut resident = app.world_mut().resource_mut::<ResidentCells>();
            for cell in built.index.cells() {
                let points = built.payload(cell.id);
                if !points.is_empty() {
                    resident.0.insert(cell.id, points.to_vec());
                    marks.push(galos_index::MarkRef {
                        id: cell.id,
                        slice: points.len() as u32,
                        at: cell.id.bounds().center(),
                    });
                }
            }
        }
        app.insert_resource(Planned(galos_index::Needed {
            mode: galos_index::Mode::Shell,
            marks,
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
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
    /// for as long as the selection stood. Spared here, where the walk is what
    /// says whose star goes.
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

    /// Nor the system the camera is standing in, which carries the origin
    ///
    /// The `FloatingOrigin` hangs under it while the camera is inside, so a
    /// walk that dropped it would take the camera down with it and leave the
    /// map with nothing to draw from.
    #[test]
    fn the_walk_does_not_drop_the_system_it_stands_in() {
        use crate::systems::tests::system;

        let mut app = walking();
        let inside = app.world_mut().spawn(system(1)).id();
        app.world_mut().spawn(system(2));
        app.insert_resource(HeldSystem::holding(inside));

        app.update();

        assert_eq!(
            dropping(&mut app),
            vec![2],
            "the walk dropped the system the camera is standing in"
        );
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
                    name: format!("Stop {address}").into(),
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
            tune: Tuning::default(),
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
                position: placed(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::ResidentIndex(built.index.clone()));
        holding(&mut app, &built);
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
            drawn.position = placed(address);
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
        //
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

    /// Reading the sky as populations, a cell's budget goes on the systems
    /// have a population
    ///
    /// Most of the galaxy is empty, and in that mode an empty system is not
    /// drawn at all ([`crate::systems::visibility`]) — so a prefix taken
    /// brightest-first spent a cell's whole budget building systems that were
    /// never painted, and the populated ones behind them never arrived. The
    /// walk still chooses the cells: what this settles is which of a held
    /// cell's systems come out of it.
    #[test]
    fn the_walk_spends_a_cells_budget_on_the_populated_systems() {
        use galos_index::meta::PopulatedSystem;
        use galos_index::{BuildParams, Snapshot};

        let inputs: Vec<galos_index::System> = (1..=5)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: placed(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::ResidentIndex(built.index.clone()));
        holding(&mut app, &built);
        // Two of the five have anybody in them: a world and a hamlet.
        let lived_in = |address: i64, population: u64| {
            (
                address,
                PopulatedSystem {
                    address,
                    name: format!("Home {address}").into(),
                    // Where the tree put it. The populated table's own
                    // place is what the population scale draws a mark at
                    // — the same place `system_at` builds one at — so a
                    // fixture that disagrees with its index is testing
                    // two galaxies.
                    position: placed(address).map(|it| it as f32),
                    population,
                    security: None,
                    government: None,
                    allegiance: None,
                    primary_economy: None,
                    secondary_economy: None,
                    factions: Vec::new(),
                    body_count: None,
                    non_body_count: None,
                },
            )
        };
        app.insert_resource(Populated(std::sync::Arc::new(HashMap::from([
            lived_in(2, 10),
            lived_in(4, 1_000_000),
        ]))));

        // The ordinary sky: every system the cell resolves is built.
        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            5,
            "the walk left a resolvable system unbuilt"
        );

        // Read as populations, only the two anybody lives in are asked for.
        // The three empty ones would be built and then never painted.
        app.insert_resource(PendingSpawns::default());
        app.insert_resource(ScalePopulation(true));
        app.update();

        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            2,
            "the budget went on systems the mode does not draw"
        );
    }

    /// A cell that admits nothing offers nothing, rather than offering it all
    ///
    /// The reported trouble: [`PointOrders`] keeps no admitted list while
    /// nothing is asked, and an empty one for a cell a filter excludes to the
    /// last point — so the population order read the second as the first and
    /// led with every system in the cell. Below the dim
    /// ([`crate::systems::filter::DimTo`] at zero) `spawn_systems` refuses
    /// every one of them, so none became an entity, none was found already
    /// drawn, and the whole cell was queued again every frame off a budget the
    /// admitted in other cells needed.
    #[test]
    fn a_cell_the_filters_empty_offers_nothing_below_the_dim() {
        use crate::systems::filter::{DimTo, Filter, Filters};
        use galos_index::meta::PopulatedSystem;
        use galos_index::{BuildParams, Snapshot};

        let inputs: Vec<galos_index::System> = (1..=4)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: placed(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::ResidentIndex(built.index.clone()));
        holding(&mut app, &built);
        // Everybody lives somewhere, so the population order holds them all.
        let lived_in = |address: i64| {
            (
                address,
                PopulatedSystem {
                    address,
                    name: format!("Home {address}").into(),
                    position: [address as f32, 0., 0.],
                    population: 1_000 * address as u64,
                    security: None,
                    government: None,
                    allegiance: None,
                    primary_economy: None,
                    secondary_economy: None,
                    factions: Vec::new(),
                    body_count: None,
                    non_body_count: None,
                },
            )
        };
        app.insert_resource(Populated(std::sync::Arc::new(HashMap::from([
            lived_in(1),
            lived_in(2),
            lived_in(3),
            lived_in(4),
        ]))));
        app.insert_resource(ScalePopulation(true));
        // A faction nobody is in, and the excluded not drawn at all.
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Faction { id: 9_999, name: "Nobody".into() });
        app.insert_resource(DimTo(0.));

        app.update();

        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            0,
            "a cell that admits nothing was offered whole"
        );
    }

    /// A merged mark the filters exclude is dimmed, and dropped below the dim
    ///
    /// **A merged mark answers the filters as the marks it replaces do.**
    /// Before this it answered nothing: at galaxy scale nearly every mark
    /// on the map is a merged one, so a faction filter dimmed the handful
    /// of drawn stars and left the galaxy standing at full strength. What
    /// it can be asked is whether anything under it is admitted — see
    /// [`crate::systems::merged::Standing`] — and the verdict is spent here
    /// exactly as a system's is: dimmed while the excluded are drawn,
    /// dropped when they are not.
    #[test]
    fn a_merged_mark_the_filters_exclude_is_dimmed_then_dropped() {
        use crate::systems::filter::{DimTo, Filter, Filters};

        let merged = |id: CellId| galos_index::BlobRef {
            id,
            count: 4_000,
            blend: 1.,
            at: id.bounds().center(),
            aged: [500; galos_index::aggregate::AGE_BUCKETS],
            m_min: Some(2.),
        };
        let held = CellId::of_point([0., 0., 0.], 6);

        let standing = |app: &mut App, share: f32| {
            app.insert_resource(crate::systems::merged::Standing::weighed(
                vec![(Vec3::splat(0.25), share, 4_000)],
            ));
        };

        let mut app = walking();
        app.insert_resource(Planned(galos_index::Needed {
            mode: galos_index::Mode::Shell,
            marks: Vec::new(),
            blobs: vec![merged(held)],
            splats: Vec::new(),
        }));
        // A filter is being asked, and what it excludes is still drawn.
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Faction { id: 9_999, name: "Nobody".into() });
        app.insert_resource(DimTo(0.5));

        // Wholly admitted: drawn whole.
        standing(&mut app, 1.);
        app.update();
        let drawn = &app.world().resource::<Blobs>().0;
        assert_eq!(drawn.len(), 1, "an admitted merged mark was not drawn");
        assert_eq!(drawn[0].fade, 1., "an admitted mark was dimmed");

        // Nothing admitted: the dim, and not gone — the cell still holds
        // systems and a mark that vanished would say it did not.
        let dim = app.world().resource::<DimTo>().opacity();
        standing(&mut app, 0.);
        app.update();
        let drawn = &app.world().resource::<Blobs>().0;
        assert_eq!(drawn.len(), 1, "an excluded mark should still be drawn");
        assert_eq!(drawn[0].fade, dim, "an excluded mark was not dimmed");

        // And a share between the two lands between them, which is the
        // average of the marks it stands for: a tenth of them at full and
        // nine tenths at the dim.
        standing(&mut app, 0.1);
        app.update();
        let drawn = &app.world().resource::<Blobs>().0;
        let want = 0.1 + 0.9 * dim;
        assert!(
            (drawn[0].fade - want).abs() < 1e-6,
            "a tenth admitted drew at {} against {want}",
            drawn[0].fade,
        );

        // Below the dim, what is excluded is not drawn at all, so a mark
        // with nothing admitted goes with it.
        app.insert_resource(DimTo(0.));
        standing(&mut app, 0.);
        app.update();
        assert!(
            app.world().resource::<Blobs>().0.is_empty(),
            "an excluded merged mark was drawn below the dim",
        );
        // But a mark with a share of itself admitted stays, at that share.
        standing(&mut app, 0.25);
        app.update();
        let drawn = &app.world().resource::<Blobs>().0;
        assert_eq!(drawn.len(), 1, "a partly admitted mark was dropped");
        assert_eq!(drawn[0].fade, 0.25);
    }

    /// A cell published again rebuilds the systems already drawn out of it
    ///
    /// The reported trouble: the walk builds what is not already on the map,
    /// which is the right test for a cell arriving and exactly the wrong one
    /// for a cell arriving a second time. Every address was already there, so
    /// nothing was queued and every system kept the columns of the first read
    /// — the moment a span is cut against among them, which is the whole of
    /// what [`crate::refresh`] exists to keep current.
    #[test]
    fn a_republished_cell_rebuilds_the_systems_already_drawn() {
        use galos_index::{BuildParams, Snapshot};

        let at = |id: u64, when: u32| galos_index::System {
            id64: id,
            position: placed(id as i64),
            absolute_magnitude: id as f64,
            temperature: 5000.,
            age_bucket: 0,
            updated_at: when,
            kind: galos_index::StarKind::G,
        };
        let built =
            Snapshot::build(&[at(1, 1_700_000_000)], &BuildParams::default());
        let owner = built
            .index
            .cells()
            .map(|cell| cell.id)
            .find(|&id| !built.payload(id).is_empty())
            .expect("some cell owns the system");

        let mut app = walking();
        app.insert_resource(crate::ResidentIndex(built.index.clone()));
        app.world_mut()
            .resource_mut::<ResidentCells>()
            .0
            .insert(owner, built.payload(owner).to_vec());
        // Marked as well as held: the walk draws the cells the plan names.
        app.insert_resource(Planned(galos_index::Needed {
            mode: galos_index::Mode::Shell,
            marks: vec![galos_index::MarkRef {
                id: owner,
                slice: built.payload(owner).len() as u32,
                at: owner.bounds().center(),
            }],
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        // Drawn already, as it would be a frame after the first read.
        app.world_mut().spawn(crate::systems::tests::system(1));

        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            0,
            "a system already drawn was built again for nothing"
        );

        // The feed hears from it again, and the builder republishes the cell.
        let again =
            Snapshot::build(&[at(1, 1_700_000_600)], &BuildParams::default());
        {
            let payload = again.payload(owner).to_vec();
            let world = app.world_mut();
            world.resource_scope(|world, mut resident: Mut<ResidentCells>| {
                world.resource_scope(|world, mut orders: Mut<PointOrders>| {
                    let mut republished = world.resource_mut::<Republished>();
                    adopt(
                        &mut resident,
                        &mut orders,
                        &mut republished,
                        owner,
                        payload.clone(),
                    );
                });
            });
        }

        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            1,
            "the republished cell left its drawn system as first read"
        );

        // And once, not every frame after: the walk is done with the cell.
        app.insert_resource(PendingSpawns::default());
        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            0,
            "the cell went on being rebuilt after it was settled"
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
