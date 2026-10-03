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
//! too, and turned into an entity by [`crate::map::galaxy::spawn`]'s `drain_spawns`; an
//! evicted one goes onto [`PendingEvictions`] for `super`'s
//! `drain_evictions`. The rest of the map — visibility, sizing, pointing,
//! selection, labels — reads a [`System`] without caring where it came from.

use crate::map::bodies::spawn::Entered;
use crate::map::camera::OrbitCamera;
use crate::map::filter::{Candidate, Cut, Filtering, Prepared};
use crate::map::galaxy::plan::{Accounted, Planned};
use crate::map::galaxy::spawn::{ColorBy, PendingSpawns};
use crate::map::galaxy::{PendingEvictions, Spyglass, System};
use crate::map::index::{Names, Populated, Transport};
use crate::map::paint::sizing::{ScalePopulation, View, by_population};
use crate::map::schedule::MapSet;
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::system::SystemParam;
use bevy::log::tracing::Instrument;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, Task};
use chrono::{DateTime, Utc};
use galos_index::prelude::{CellId, CellSystem, Lit, Part, Stamp, StarKind};
use galos_index::read::inhabited::{Inhabited, Readings};
use galos_index::read::resident::{Resident, ResidentCell};
use galos_index::read::screen::{Crowded, Empty, Share};
use galos_photometry::{Distance, Magnitude};
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};
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
            .after(crate::map::filter::Marking),
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

/// `f` of every index under `n`, in order: in chunks across the compute
/// pool where there are enough of them to be worth the hand-off
fn across<T: Send + 'static>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    const CHUNK: usize = 8192;
    if n < CHUNK * 2 {
        return (0..n).map(f).collect();
    }
    let f = &f;
    let parts = bevy::tasks::ComputeTaskPool::get().scope(|scope| {
        for start in (0..n).step_by(CHUNK) {
            scope.spawn(async move {
                (start..(start + CHUNK).min(n)).map(f).collect::<Vec<_>>()
            });
        }
    });
    let mut out = Vec::with_capacity(n);
    for part in parts {
        out.extend(part);
    }
    out
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
/// the galaxy and back ([`crate::map::galaxy::flight`]), 2,615 payload reads of which
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
    fn marks(&mut self, marks: &[galos_index::read::walk::MarkRef]) {
        self.marked.clear();
        self.marked.extend(marks.iter().map(|mark| mark.id));
    }

    /// Whether the walk marks this cell
    ///
    /// The cells the plan marks, for a pass that asks about many of them.
    fn marked(&self) -> &FxHashSet<CellId> {
        &self.marked
    }

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
/// ([`crate::map::filter`]'s `mark` re-asks a system's own copy of those).
///
/// So a replaced payload is noted here and its cell is rebuilt whole on the
/// next walk, `spawn_systems` replacing each system in place. Noted rather
/// than acted on at once because the walk is where a cell's drawn prefix is
/// known, and rebuilding a system the prefix does not reach would spend the
/// spawn budget on something about to be evicted.
#[derive(Resource, Default)]
pub(crate) struct Republished(FxHashSet<CellId>);

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
/// [`crate::map::paint::field`]'s one mesh beside the stars.
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
    /// The light of the marks it stands for that the filters let through,
    /// and of those they exclude, both at full and in linear light, and the
    /// fade the realistic view dims it by: see
    /// [`crate::map::galaxy::blobs::Standing`].
    pub(crate) through: Vec3,
    pub(crate) dimmed: Vec3,
    pub(crate) fade: f32,
    /// The cell it stands for, which is what names it: the system a merged
    /// mark is pointed at is read out of this cell's own payload. See
    /// [`crate::map::galaxy::blobs`].
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
    /// from; see [`crate::map::galaxy::populated::PopulatedOrder`].
    populated_order: Res<'w, crate::map::galaxy::populated::PopulatedOrder>,
    /// What each merged mark stands for and what the filters leave of it;
    /// see [`crate::map::galaxy::blobs::Standing`].
    standing: Res<'w, crate::map::galaxy::blobs::Standing>,
    orders: ResMut<'w, PointOrders>,
    republished: ResMut<'w, Republished>,
    /// The marked set, which this pass is the one to take from the plan: it
    /// is the first of the three to read it and the only one that must not
    /// read it a frame late.
    keeping: ResMut<'w, Keeping>,
    planned: Res<'w, Planned>,
    /// What each marked cell's marks account for, which the field subtracts
    /// from the aggregate it lays down so the two never draw one system twice.
    drawn: ResMut<'w, crate::map::galaxy::plan::Drawn>,
    /// What the frame's marks came to, for the panel to read.
    sampled: ResMut<'w, Sampled>,
    /// The merged marks the frame draws, which the merge above settles.
    blobs: ResMut<'w, Blobs>,
    /// Which entity draws each system.
    addresses: Res<'w, crate::map::galaxy::Addresses>,
    /// What the map is colored by, star class spending each cell's budget by
    /// class; see [`strata`].
    color_by: Res<'w, ColorBy>,
}

/// One cell's read as it lands: the payload and the [`Stamp`] it was read
/// under.
type Landed = (CellId, io::Result<(Vec<CellSystem>, Vec<Lit>, Option<Stamp>)>);

/// The payload reads the walk wants, queued and on the wire
///
/// **Read by a few workers, most wanted first, between frames as well as
/// during them.** Opening a payload file is the whole cost of a read —
/// measured over `.index/full`, some twelve microseconds an open against one
/// and a half for a `stat`, and no faster on eighteen threads than on four —
/// so a wide view of seventy thousand cells is most of a second however it
/// is spread. What the spreading decides is what lands first and what else
/// waits. One task a cell put tens of thousands on the compute pool at once,
/// landed them in whatever order the walk named them, held every thread in
/// the kernel and was polled one by one every frame.
///
/// So the asks queue here, in the order [`fetch`] ranks them, and
/// [`IN_FLIGHT`] workers on the IO pool take [`BATCH`] cells at a time off
/// it until it is empty. **The workers pull; the frame does not push.** A
/// batch handed out once a frame per worker tied the reads to the frame
/// rate: 256 cells a frame, which over a wide view at the map's own frame
/// times was ten seconds to fill what the disk reads in under one. A queued
/// cell the plan stops wanting is dropped unread.
///
/// Each read carries the payload and the [`Stamp`] the transport gave for
/// it, so a refresh knows what it is holding and asks whether that has moved
/// rather than reading every resident cell again. Stamped before the read,
/// so a payload rewritten between the two is held under the older stamp and
/// read again on the next poll — the safe way round.
#[derive(Resource, Default)]
pub(crate) struct BoundedTasks {
    /// What the workers and the frame share.
    shared: Arc<Mutex<Reads>>,
    /// The workers, each running until the queue is empty.
    workers: Vec<Task<()>>,
}

/// The queue and what has come off it; see [`BoundedTasks`].
#[derive(Default)]
struct Reads {
    /// Asked and not yet taken, the most wanted last so a batch pops off the
    /// end.
    queued: Vec<(CellId, usize)>,
    /// Taken by a worker and not yet landed, so a plan asking again does not
    /// queue them twice.
    reading: FxHashSet<CellId>,
    /// Read and waiting for the frame to take them in.
    landed: Vec<Landed>,
}

/// How many cells a worker takes off the queue at once: some three quarters
/// of a millisecond of opens, few enough that what the frame asks for next
/// is not long behind them.
const BATCH: usize = 64;

/// How many workers read at once. Past four threads the opens go no faster
/// (see [`BoundedTasks`]); more would only take threads from whatever else is
/// waiting on the pool.
const IN_FLIGHT: usize = 4;

impl BoundedTasks {
    /// Queue `asking`, ranked most wanted first, in place of whatever was
    /// queued before; what is already on the wire is left to land.
    fn ask(&mut self, mut asking: Vec<(CellId, usize)>) {
        let mut reads = self.shared.lock().expect("the reads lock");
        asking.retain(|(id, _)| !reads.reading.contains(id));
        asking.reverse();
        reads.queued = asking;
    }

    /// Keep [`IN_FLIGHT`] workers reading while anything is queued.
    fn send(&mut self, transport: &Transport) {
        self.workers.retain(|worker| !worker.is_finished());
        if self.shared.lock().expect("the reads lock").queued.is_empty() {
            return;
        }
        let pool = IoTaskPool::get();
        while self.workers.len() < IN_FLIGHT {
            let shared = Arc::clone(&self.shared);
            let source = transport.0.clone();
            self.workers.push(pool.spawn(async move {
                loop {
                    let batch: Vec<(CellId, usize)> = {
                        let mut reads = shared.lock().expect("the reads lock");
                        let from = reads.queued.len().saturating_sub(BATCH);
                        let batch: Vec<_> =
                            reads.queued.drain(from..).rev().collect();
                        reads.reading.extend(batch.iter().map(|&(id, _)| id));
                        batch
                    };
                    if batch.is_empty() {
                        return;
                    }
                    // One zone a batch. At info with the rest: the walk is
                    // the map's live payload path, so a capture that left
                    // these out would show every frame and none of the
                    // reads the frames are waiting on.
                    let cells = batch.len();
                    let landed = async {
                        let mut landed = Vec::with_capacity(batch.len());
                        for (id, want) in batch {
                            // The stamp first: a payload republished between
                            // the two is then held under the older stamp and
                            // re-read by the next refresh, where the other
                            // order would hold a stamp for contents the map
                            // does not have.
                            let stamp = source
                                .stamp(Part::Cell(id))
                                .await
                                .ok()
                                .flatten();
                            // And the light after the payload: it is written
                            // before the payload it stands beside, so what
                            // is read here is at least as new as the points.
                            let read = async {
                                let points =
                                    source.payload_prefix(id, want).await?;
                                let lit = source.lit(id, points.len()).await?;
                                Ok((points, lit, stamp))
                            }
                            .await;
                            landed.push((id, read));
                        }
                        landed
                    }
                    .instrument(info_span!("cell payloads", cells))
                    .await;
                    shared
                        .lock()
                        .expect("the reads lock")
                        .landed
                        .extend(landed);
                }
            }));
        }
    }

    /// What has landed since the last frame took it.
    fn landed(&mut self) -> Vec<Landed> {
        let mut reads = self.shared.lock().expect("the reads lock");
        let landed = std::mem::take(&mut reads.landed);
        for (id, _) in &landed {
            reads.reading.remove(id);
        }
        landed
    }
}

#[cfg(test)]
impl BoundedTasks {
    /// The cells asked and not yet landed, queued or on the wire, for a
    /// caller that wants to know what a frame put to the transport
    ///
    /// Read by the flight guard ([`crate::map::galaxy::flight`]), which counts a cell read
    /// twice over one flight as work paid for twice.
    pub(crate) fn cells(&self) -> Vec<CellId> {
        let reads = self.shared.lock().expect("the reads lock");
        reads
            .queued
            .iter()
            .map(|&(id, _)| id)
            .chain(reads.reading.iter().copied())
            .collect()
    }

    /// Whether nothing is queued, on the wire or waiting to be taken in
    pub(crate) fn is_empty(&self) -> bool {
        let reads = self.shared.lock().expect("the reads lock");
        reads.queued.is_empty()
            && reads.reading.is_empty()
            && reads.landed.is_empty()
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

/// Whether the draw spends each cell's budget by class ([`strata`]): along
/// star class, on the map — not in the sky, whose marks are the stars that
/// clear the eye's floor, nor among who lives where, which star class is
/// not offered for
fn by_class(
    color_by: ColorBy,
    mode: &galos_index::prelude::Mode,
    by_population: bool,
) -> bool {
    matches!(color_by, ColorBy::StarClass)
        && matches!(mode, galos_index::prelude::Mode::Shell)
        && !by_population
}

/// Ask for the payloads of the marks cells the map does not hold enough of
///
/// **A prefix and not the payload.** A cell's payload is in standing order,
/// an even sample of the cell, and the draw takes a share of it
/// ([`wanted`]), so the rest is bytes
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
/// Where a filter is asked, or star class draws each cell by class
/// ([`strata`]), marked cells are held whole. The filters promote systems
/// out of the payload's order — a faction is a handful of systems anywhere in
/// a payload — and a class sample is exact to the cell's proportions only
/// over the cell, so a prefix cannot answer the first and answers the
/// second only to within its own sampling. Under a filter admitting nothing but rows of the populated table
/// it is only the cells holding a row; see [`Whole::Rows`]. A cell the
/// frame draws from is still read to its prefix first, so the frame fills
/// in evenly; see [`reads`].
pub(crate) fn fetch(
    planned: Res<Planned>,
    resident: Res<ResidentCells>,
    transport: Res<Transport>,
    filters: Res<crate::map::filter::Filters>,
    view_mode: Res<View>,
    scale_population: Res<ScalePopulation>,
    color_by: Res<ColorBy>,
    populated_order: Res<crate::map::galaxy::populated::PopulatedOrder>,
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
        && !color_by.is_changed()
        && !populated_order.is_changed()
    {
        return;
    }
    // **Nothing at all while the sky is read as populations.** That mode
    // draws the systems anybody lives in and takes them from the resident
    // table ([`crate::map::galaxy::populated::PopulatedOrder`]), so a payload answers nothing it
    // asks — and a payload read for nothing is the whole galaxy faulted in
    // to draw a few hundred marks. What is already held is left to
    // [`evict_payloads`] to let go of on its own grace.
    //
    // Unless a span is asked, a moment being a payload's to carry; see
    // [`Filters::asking_a_span`] and [`reconcile`].
    let by_population =
        crate::map::paint::sizing::by_population(&view_mode, &scale_population);
    if by_population && !filters.asking_a_span() {
        return;
    }
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = crate::map::galaxy::plan::view(orbit, camera) else {
        return;
    };
    // Which marked cells are wanted whole: every one under a filter
    // narrowing the map, the systems it admits standing anywhere in a
    // payload's standing order and [`reconcile`] drawing them first out of
    // whatever of it is held — or where all it admits is rows of the
    // populated table, every one holding a row; and the ones that draw
    // along star class, drawing every class in its proportion. Not under a
    // mask that only thins the sky, which admits what the unfiltered map
    // draws with some of it taken out: a cell's brightest less a few
    // colonies, or a class sample less its hidden classes; see
    // [`Filters::only_thins`].
    let by_class = by_class(*color_by, &planned.0.mode, by_population);
    let whole = if filters.asking() && !filters.only_thins() {
        match filters.admits_only_rows() {
            true => Whole::Rows { by_class },
            false => Whole::Every,
        }
    } else if by_class {
        Whole::Drawing
    } else {
        Whole::None
    };
    // What the draw will spread over. Off the plan alone — a mark carries
    // its own slice — which is what lets the share be struck here as well
    // as in [`reconcile`] and lets the two agree without either of them
    // touching the index.
    let share = Share::of(population(&planned.0), view.marks());
    // Whether this is the photometric sky, whose reads are whole cells.
    let real =
        matches!(planned.0.mode, galos_index::prelude::Mode::Real { .. });
    // The two halves of the frame's flat cost, measured apart: the set
    // arithmetic over every marked cell, and the asking that follows it. A
    // still view asks for nothing and pays the first of them anyway, which is
    // what a capture has to be able to see.
    let asking = {
        let _zone = info_span!("missing cells").entered();
        reads(
            &planned.0.marks,
            |slice, id| share.wanted(slice, id),
            |id| resident.0.cell(id).map_or(0, |held| held.points.len()),
            whole,
            |id| populated_order.holds_a_row(id),
            real,
        )
    };
    let _zone = info_span!("cell tasks", missing = asking.len()).entered();
    tasks.ask(asking);
    tasks.send(&transport);
}

/// What [`fetch`] asks for: each marked cell not held to what the frame
/// wants, and how much of it, most wanted first
///
/// `wanted` is how many of a cell's `slice` the frame draws, `held` how many
/// of its points the map holds, `whole` which marked cells the draw wants
/// held whole, `rows` whether a cell holds a row of the populated table
/// ([`Whole::Rows`]), and `real` whether this is the photometric sky.
fn reads(
    marks: &[galos_index::read::walk::MarkRef],
    wanted: impl Fn(usize, CellId) -> usize,
    held: impl Fn(CellId) -> usize,
    whole: Whole,
    rows: impl Fn(CellId) -> bool,
    real: bool,
) -> Vec<(CellId, usize)> {
    let mut asking = Vec::new();
    for mark in marks {
        // Past the clamp there is nothing to test for: the walk is clamped
        // to the reach itself, so a cell it marks is a cell the bubble
        // touches. See [`galos_index::Reach`].
        let id = mark.id;
        let slice = mark.slice as usize;
        // **The sky reads a cell whole.** Which of a cell draws is decided
        // per star against the exposure's floor ([`reconcile`]), and a
        // star's magnitude is a fact only its payload point carries — so a
        // prefix sized off a share is a cell whose fainter half cannot be
        // weighed at all. It is what left 33 of the 184 naked-eye stars in
        // a frustum undrawn: not rationed away, never read. The walk has
        // already dropped every subtree that cannot clear the floor, so the
        // cells reaching here are few and what they hold is what the sky is
        // made of.
        let drawn = wanted(slice, id);
        let draws = drawn > 0 || real;
        let prefix = (drawn * READ_SLACK).max(READ_LEAST).min(slice);
        let sampled = draws && slice <= prefix * WHOLE_REACH;
        let whole = match whole {
            Whole::None => false,
            Whole::Drawing => sampled,
            Whole::Rows { by_class } => (by_class && sampled) || rows(id),
            Whole::Every => true,
        };
        // **Held whole, a cell that draws is read whole, but not first.** It is
        // read to the prefix an unfiltered frame would read, and only once
        // every drawing cell has its prefix is any of them read the rest of
        // the way. Read whole from the first, the reads were thousands of
        // points apiece and each cell drew nothing until its own landed, so
        // the view filled in a tile at a time where unfiltered it fills
        // evenly all over.
        //
        // **And one that draws nothing is read whole at once.** It has no
        // share to fill in evenly, and a prefix would be a second open of
        // the same file for the few points [`Empty`] lights a dark patch of
        // sky from — at a wide zoom most marked cells, and an open is the
        // whole cost of a read (see [`BoundedTasks`]).
        let first = if real || (whole && !draws) { slice } else { prefix };
        let held = held(id);
        let (want, stage) = if held < first {
            (first, Stage::First)
        } else if whole && held < slice {
            (slice, Stage::Rest)
        } else {
            continue;
        };
        let (idle, level, scatter) = rank(id, draws);
        asking.push(((idle, stage, level, scatter), id, want));
    }
    asking.sort_unstable_by_key(|&(rank, _, _)| rank);
    asking.into_iter().map(|(_, id, want)| (id, want)).collect()
}

/// Which marked cells the draw wants held whole
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Whole {
    /// None: a prefix is all any of them is drawn from.
    None,
    /// The cells that draw this frame, along star class, and of those only
    /// the ones a whole read costs at most [`WHOLE_REACH`] prefixes: a class
    /// sample reaches as far down a payload as its faintest class, and a
    /// cell that draws nothing samples nothing.
    Drawing,
    /// Every one holding a row of the populated table, under a filter that
    /// admits nothing else ([`Filters::admits_only_rows`]), and along star
    /// class the ones [`Self::Drawing`] reads whole besides.
    ///
    /// **A cell with no row holds nothing to find.** Read whole under every
    /// filter, the uninhabited hidden at nine thousand light years back
    /// read 16.2 million points over `.index/full` to draw 6,431 colonies,
    /// 13.2 million of them out of the 12,799 cells of 15,431 without one
    /// colony in them, and the view took eighty frames of the verdict
    /// budget to weigh them.
    ///
    /// [`Filters::admits_only_rows`]: crate::map::filter::Filters::admits_only_rows
    Rows { by_class: bool },
    /// Every one, under a filter: see [`fetch`].
    Every,
}

/// How many of its prefixes a drawing cell is read whole for along star
/// class, past which its class sample is drawn from the prefix alone
///
/// **A sample of a handful is not worth a payload.** Zoomed out, a cell's
/// share is a few marks of hundreds or thousands of systems, and every
/// drawing cell read whole held nineteen times the points at five thousand
/// light years out — measured over `.index/full`, 7.4 million against
/// 381 thousand — to spend most of it on a few marks apiece. Eight
/// prefixes is a share of a thirty-second, which reaches from Sol's own
/// neighbourhood out to some fourteen hundred light years back, where the
/// sample still has marks enough to say something; beyond it the marks are
/// the prefix's brightest, drawn by class among themselves.
const WHOLE_REACH: usize = 8;

/// Which of a cell's reads is being asked for, in the order the queue takes
/// them among the cells that draw alike
///
/// **Every drawing cell's prefix before any cell's rest.** A filtered frame
/// holds every marked cell whole, and a whole read is thousands of points;
/// asked for first, they kept the prefixes that draw the frame waiting
/// behind them, and the view filled in a cell at a time.
///
/// **But never ahead of whether the cell draws.** A cell that draws nothing
/// this frame waits behind every read of one that does, its rest included:
/// ranked by stage first, every such cell's read went ahead of the rest of
/// the cells the filter's systems are drawn out of, and under a filter
/// admitting a handful of systems a payload the map drew nothing it asked
/// for until tens of thousands of reads had landed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    /// The cell's first read: what an unfiltered frame reads of it, the
    /// share it draws with [`READ_SLACK`] over it and [`READ_LEAST`] under
    /// it, or the whole of it where that is all it is read in (see
    /// [`reads`])
    First,
    /// The rest of a cell already read to its prefix, which a filter
    /// narrowing the map wants held
    Rest,
}

/// Where a cell stands in the queue, lowest first, with its [`Stage`] ranked
/// between the first of these and the rest
///
/// **The cells that draw a mark this frame before the ones that do not.**
/// Every marked cell in reach is read to [`READ_LEAST`] whatever its share,
/// so [`Empty`] can light a dark patch of sky from it; at a wide zoom most
/// of them draw nothing, and read in the walk's order they held the marks
/// that do draw behind them.
///
/// Then coarse before fine, a coarse cell being the brightest of a wider
/// patch of sky, and within a level scattered by address, so the frame
/// fills in evenly all over rather than sweeping across it in the walk's
/// order.
fn rank(id: CellId, draws: bool) -> (bool, u8, u64) {
    (!draws, id.level, id.morton().wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// A system a marked cell would draw and the map has not got, waiting its
/// round
///
/// **Offered in rounds across the cells, not a cell at a time.** The pass
/// used to offer each cell's undrawn systems as it reached the cell, in the
/// plan's order, until the frame's offers ran out. Zooming in, every cell of
/// the finer level coming into view wants dozens of marks, so the offers ran
/// out a few cells into the plan and the view filled in a square at a time,
/// one neighbourhood of the walk after the next. Zooming out, each cell wants
/// a mark or two, the same offers reached across the whole frame, and the
/// sky filled in evenly — which is why the two directions looked nothing
/// alike.
///
/// So every cell's first undrawn system goes before any cell's second, and
/// within a round coarse before fine and scattered by address, which is
/// [`rank`] and the order the reads land in too. The frame fills in all over
/// at once and gains density as it goes, whichever way the camera moved.
struct Unspawned {
    /// Its place among its cell's undrawn systems, in the order they draw.
    round: u32,
    rank: (bool, u8, u64),
    address: i64,
    cell: CellId,
    /// Which point of the cell's payload it is.
    index: u32,
}

/// A marked cell the pass draws out of, asked down the plan: where it
/// stands in the plan, how far down its order it draws, and whether its
/// payload is newer than the systems drawn from it
#[derive(Clone, Copy)]
struct Job {
    offer: usize,
    id: CellId,
    target: usize,
    refreshed: bool,
}

/// What a cell came to once taken: its account, the drawn systems it still
/// wants, and the ones it would draw that are not drawn yet
#[derive(Default)]
struct Took {
    count: usize,
    accounted: Accounted,
    wanted: Vec<Entity>,
    unspawned: Vec<Unspawned>,
}

/// How many cells one thread takes: a cell is a share's worth of points,
/// a few apiece with the galaxy seen whole and dozens close in
const TAKE_CHUNK: usize = 512;

/// Offer what the frame can take of `unspawned`, round by round
///
/// Only the offers the pass has room for are ordered, the rest being dropped
/// unsorted: the pass runs again next frame and gathers whatever is still
/// wanted. Answers whether there is room left.
fn offer_in_rounds(
    pending: &mut PendingSpawns,
    mut unspawned: Vec<Unspawned>,
) -> bool {
    let key = |it: &Unspawned| (it.round, it.rank);
    let room = pending.offers_left();
    if unspawned.len() > room {
        unspawned.select_nth_unstable_by_key(room, key);
        unspawned.truncate(room);
    }
    unspawned.sort_unstable_by_key(key);
    for it in unspawned {
        if !pending.offer(it.address, it.cell, it.index) {
            return false;
        }
    }
    pending.offers_left() > 0
}

/// How many systems the marked sky holds, off the index and not off what
/// has landed
///
/// The share has to be the same figure in [`fetch`] and in [`reconcile`],
/// and it must not move as payloads arrive: a share struck over what is
/// held would rise while the map was still reading and every mark already
/// drawn would shift under it. A cell's slice length is known from the
/// index the moment the walk marks it.
fn population(planned: &galos_index::prelude::Needed) -> u64 {
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
/// [`crate::map::index::refresh`] ask whether the cell has been republished since instead
/// of reading every resident payload on every poll.
pub(crate) fn collect(
    mut tasks: ResMut<BoundedTasks>,
    transport: Res<Transport>,
    mut resident: ResMut<ResidentCells>,
    mut orders: ResMut<PointOrders>,
    mut republished: ResMut<Republished>,
    mut held: ResMut<crate::map::index::refresh::Stamps>,
) {
    for (id, read) in tasks.landed() {
        if let Ok((points, lit, stamp)) = read {
            // The same payload read further down — the rest of a prefix, or a
            // share that has outgrown it — holds nothing the systems already
            // drawn out of it do not, so they are left standing. Anything
            // else may be a republished cell, and rebuilds them.
            let rebuild = !held.unchanged(id, stamp);
            adopt(
                &mut resident,
                &mut orders,
                rebuild.then_some(&mut *republished),
                id,
                points,
                lit,
            );
            held.holding(id, stamp);
        }
    }
    // And the next batches, now there is room on the wire for them.
    tasks.send(&transport);
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
/// Three of them, all walks of every point of every resident payload and all
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
///   ([`crate::map::galaxy::visibility`]), so a slot of a cell's budget spent on
///   one buys nothing. It moves with the political table and the payload,
///   which is the same [`Cut`] and the same [`adopt`].
/// - Along star class, every point in an order that spends a cell's budget
///   on each class in proportion to how many of it the cell holds, brightest
///   first within each; see [`strata`]. It moves only with the payload, so
///   [`adopt`] is all that drops it.
///
/// Indices into the cell's payload rather than addresses. The admitted list is
/// ascending, so the fill can walk the payload and it together and take what
/// is in one and not the other without a set to test against; the populated
/// list and the strata are in the order they are drawn in.
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
    /// [`crate::map::galaxy::spawn::SPAWN_BUDGET`].
    at: FxHashMap<CellId, u64>,
    cells: FxHashMap<CellId, Vec<u32>>,
    populated: FxHashMap<CellId, Vec<u32>>,
    strata: FxHashMap<CellId, Vec<u32>>,
    /// What the strata were worked out along: the axis the map is colored
    /// by, and for a political one the cut, the populated table being what
    /// a point's value along it is read from. See [`Along`].
    strata_along: Option<(ColorBy, u64)>,
    /// Each cell's points brightest first, for the sky; see [`bright`].
    bright: FxHashMap<CellId, Vec<u32>>,
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

    /// Work out whatever this cut has not asked about this cell yet, and say
    /// whether that was done or left for a later budget
    ///
    /// Both lists in one pass over the payload, so [`reconcile`] can read
    /// either or both of them afterwards without holding this borrow open.
    /// `by_population` says whether the busiest order is wanted at all: it is
    /// a walk of the payload against the political table, and there is no
    /// sense paying for one while the sky is not being read that way.
    fn walk(
        &mut self,
        id: CellId,
        points: &[CellSystem],
        filters: &Prepared<'_>,
        populated: &Populated,
        now: DateTime<Utc>,
        by_population: bool,
        budget: &mut usize,
    ) -> bool {
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
            return false;
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
        true
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
    /// [`crate::map::galaxy::populated::PopulatedOrder`] instead and this is left to the one
    /// case that cannot: a span, which only a payload point carries a
    /// moment for.
    fn populated(&self, id: CellId) -> &[u32] {
        self.populated.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Work out the order the map draws this cell in along the axis it is
    /// colored by, where it has none
    ///
    /// Off the same budget as the verdicts, it being a walk and a sort of the
    /// whole payload: a cell the budget does not reach draws in its standing
    /// order this frame and in its values' proportions once it does.
    fn stratify(
        &mut self,
        id: CellId,
        points: &[CellSystem],
        along: &Along<'_>,
        budget: &mut usize,
    ) -> bool {
        if self.strata.contains_key(&id) {
            return true;
        }
        if *budget < points.len() {
            return false;
        }
        *budget -= points.len();
        self.strata.insert(id, strata(points, along));
        true
    }

    /// The order the map draws a cell in along its axis, where
    /// [`Self::stratify`] has worked one out
    fn strata(&self, id: CellId) -> Option<&[u32]> {
        self.strata.get(&id).map(Vec::as_slice)
    }

    /// Hold the strata to the axis they are drawn along, dropping every
    /// cell's where it has moved — or where nothing is drawn by them
    fn stratify_along(&mut self, along: Option<(ColorBy, u64)>) {
        if self.strata_along != along {
            self.strata_along = along;
            if !self.strata.is_empty() {
                self.strata = FxHashMap::default();
            }
        }
    }

    /// Work out a cell's brightest-first order for the sky, where it has none
    ///
    /// Off the same budget as the verdicts, it being a sort of the whole
    /// payload's light. A cell the budget does not reach is weighed point by
    /// point this frame, which is what the sky did before it had an order to
    /// cut short.
    fn brighten(
        &mut self,
        id: CellId,
        cell: &ResidentCell,
        budget: &mut usize,
    ) -> bool {
        if self.bright.contains_key(&id) {
            return true;
        }
        if *budget < cell.points.len() {
            return false;
        }
        *budget -= cell.points.len();
        self.bright.insert(id, bright(cell));
        true
    }

    /// A cell's points brightest first, where [`Self::brighten`] has worked
    /// the order out
    fn bright(&self, id: CellId) -> Option<&[u32]> {
        self.bright.get(&id).map(Vec::as_slice)
    }

    /// Drop every cell's brightest-first order, the sky no longer being
    /// drawn
    fn unbrighten(&mut self) {
        if !self.bright.is_empty() {
            self.bright = FxHashMap::default();
        }
    }

    /// Forget a cell, its payload having been freed
    pub(crate) fn forget(&mut self, id: CellId) {
        self.cells.remove(&id);
        self.populated.remove(&id);
        self.strata.remove(&id);
        self.bright.remove(&id);
        self.at.remove(&id);
    }
}

/// What a point's value is along the axis the map is colored by, which is
/// what [`strata`] draws a cell's values in proportion to
///
/// **Every view orders a cell by its own values.** The payload is in
/// standing order, which is no view's; each view then draws a cell's points
/// so that any share of them holds each of its values in the proportion the
/// cell does. Star class reads a point's kind off the point itself. A
/// political axis reads it off the populated table, a system nobody lives
/// in standing in the axis's unreported bucket — so the strata go stale with
/// the table, and [`PointOrders::stratify_along`] is told the cut.
pub(crate) struct Along<'a> {
    axis: ColorBy,
    populated: &'a Populated,
}

impl<'a> Along<'a> {
    pub(crate) fn of(axis: ColorBy, populated: &'a Populated) -> Along<'a> {
        Along { axis, populated }
    }

    /// The bucket `point` stands in along the axis: what the walk strata
    /// are drawn in proportion to, and what an enhanced picture colors by.
    pub(crate) fn bucket(&self, point: &CellSystem) -> usize {
        match self.axis {
            ColorBy::StarClass => usize::from(point.kind.code()),
            axis => self
                .populated
                .get(point.id64 as i64)
                .map_or(0, |row| axis.bucket(&Readings::of(row))),
        }
    }

    /// Whether the axis is star class, whose values are the points' own.
    fn is_star_class(&self) -> bool {
        matches!(self.axis, ColorBy::StarClass)
    }

    /// How many buckets the axis has.
    fn buckets(&self) -> usize {
        self.axis.buckets()
    }

    /// What the strata along this axis go stale with: nothing along star
    /// class, whose values are the points', and the cut along a political
    /// axis, whose values are the populated table's.
    fn key(&self, cut: u64) -> (ColorBy, u64) {
        match self.axis {
            ColorBy::StarClass => (self.axis, 0),
            axis => (axis, cut),
        }
    }
}

/// A cell's points brightest first, by the light the sidecar holds for each:
/// what the sky is drawn in, and what the floor cuts short
///
/// **The realistic view's order and no other's.** The payload is in
/// standing order, an even sample of the cell, so the stars that clear the
/// eye's floor stand anywhere in it; this is the order the payload used to
/// be in, worked out of the light where it is read rather than built into
/// the tree every view reads. A point with no light on record stands at the
/// default class. Ties go to the earlier point, so the order is the same
/// answer every time.
fn bright(cell: &ResidentCell) -> Vec<u32> {
    let mut order: Vec<(f32, u32)> = (0..cell.points.len())
        .map(|at| (magnitude_at(cell, at), at as u32))
        .collect();
    order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    order.into_iter().map(|(_, at)| at).collect()
}

/// The absolute magnitude of a resident cell's `at`th point: its light where
/// the sidecar had one, the default class where it did not.
fn magnitude_at(cell: &ResidentCell, at: usize) -> f32 {
    cell.lit_at(at).map_or(
        galos_photometry::ClassLight::DEFAULT.absolute_magnitude.0 as f32,
        |lit| lit.magnitude,
    )
}

/// A cell's points in the order the map draws them along the axis it is
/// colored by: every value in proportion to how many of it the cell holds,
/// in standing order within each
///
/// **A view's marks are a sample of what it colors.** Drawn in any order of
/// its own — brightness was the one the payload used to be in — a cell spent
/// the budget its rarer values earned on its commoner ones wherever in the
/// cell they stood: brown dwarfs, last in every payload at an absolute
/// magnitude of sixteen and a half, were never reached, and a slab of them
/// twenty light years thick drew as a thin strip between two bands packed
/// with the M, K and G stars of the same cells — measured over `.index/full`
/// at four hundred light years out, 603 marks in it against 1,399 in the
/// layer above, holding two and a half times as many systems. The payload is
/// an even sample now ([`galos_index::core::standing`]); this makes any
/// prefix of a cell exact to its own proportions as well.
///
/// The `j`th of a value's `n` points is due at `(j + ½) / n`, and the points
/// are taken in the order they fall due, so any prefix holds each value in
/// the proportion the cell does to within one point. Ties go to the earlier,
/// so the order is the same answer every time.
fn strata(points: &[CellSystem], along: &Along<'_>) -> Vec<u32> {
    let buckets: Vec<usize> =
        points.iter().map(|point| along.bucket(point)).collect();
    let mut of = vec![0u32; along.buckets()];
    for &bucket in &buckets {
        of[bucket] += 1;
    }
    let mut seen = vec![0u32; along.buckets()];
    let mut due: Vec<(f64, u32)> = buckets
        .iter()
        .enumerate()
        .map(|(index, &bucket)| {
            let j = seen[bucket];
            seen[bucket] += 1;
            ((f64::from(j) + 0.5) / f64::from(of[bucket]), index as u32)
        })
        .collect();
    due.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    due.into_iter().map(|(_, index)| index).collect()
}

/// Take `points` as a cell's payload, dropping whatever was worked out about
/// the one it replaces
///
/// The two go together and must: [`PointOrders`] holds *indices into the
/// payload*, so a list kept across a replacement names whichever systems now
/// sit at those places. A republished cell is the case — see
/// [`crate::map::index::refresh`] — and a first read is the same call with nothing to
/// forget.
///
/// The systems already drawn out of the old payload are the third thing that
/// goes with it, and the one this cannot do itself: they are entities, and
/// which of them the walk still draws is not known until it walks. So the
/// cell is noted in `republished` and [`reconcile`] rebuilds it. A first
/// read notes it too and nothing comes of that, the cell having nothing drawn
/// out of it yet. [`None`] where `points` is known to be the held payload
/// read further, whose drawn systems are already what it says.
pub(crate) fn adopt(
    resident: &mut ResidentCells,
    orders: &mut PointOrders,
    republished: Option<&mut Republished>,
    id: CellId,
    points: Vec<CellSystem>,
    lit: Vec<Lit>,
) {
    resident.0.insert(id, points, lit);
    orders.forget(id);
    if let Some(republished) = republished {
        republished.0.insert(id);
    }
}

/// What the filters ask about a payload point: its address, what the resident
/// table says of it, and the moment the payload carries.
///
/// The same facts a [`System`] answers, so a point is weighed by the one
/// predicate a drawn system is, and without building a system to ask —
/// [`System::of`] clones a name and reads a reach, work worth avoiding
/// for a point that is not going to be drawn.
pub(crate) fn candidate<'a>(
    point: &CellSystem,
    populated: &'a Populated,
) -> Candidate<'a> {
    Candidate::off_the_table(
        point.id64 as i64,
        populated,
        DateTime::from_timestamp(point.updated_at as i64, 0),
        point.kind,
    )
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
    points: &'a [CellSystem],
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

/// [`drawn_first`] along an order of the cell's own: what the filters admit
/// and then the rest, each in the cell's [`strata`] or, in the sky, its
/// [`bright`] order, rather than the payload's
///
/// `admits` is ascending, so a point's admission is a binary search; both
/// halves walk the strata, the second only where `fill` asks for it.
fn stratified_first<'a>(
    strata: &'a [u32],
    admits: &'a [u32],
    fill: bool,
) -> impl Iterator<Item = usize> + 'a {
    let admitted = move |index: &&u32| admits.binary_search(index).is_ok();
    let first = strata.iter().filter(admitted);
    let rest = strata
        .iter()
        .filter(move |index| !admitted(index))
        .take(if fill { strata.len() } else { 0 });
    first.chain(rest).map(|&index| index as usize)
}

/// What each narrowed cell draws of what the filters admit: the payload
/// indices, by the cell's offer in the plan, that won a patch of sky
///
/// Each admitted point in the bubble claims its patch ([`Crowded::claim`])
/// until the frame has claimed `ceiling`; one landing on a patch already
/// claimed is left to the field. So a sparse arm draws every colony that
/// stands apart, and the core is one mark to a patch however many stand in
/// it — where a share of each cell's count drew the arm's colonies a mark a
/// cell at best and piled the core's up into a white disc.
///
/// **Offered in proportion across the cells**, as the strata are within
/// one ([`strata`]): the `j`th of a cell's `n` admitted falls due at
/// `(j + d) / n`, and they are claimed as they fall due, coarse before fine
/// and scattered by address among those due together ([`rank`]). Where the
/// ceiling cuts, every cell has drawn the same share of what it admits,
/// in its own order, so the marks thin evenly and stand as dense as the
/// field under them says the colonies are. Where it does not — a view wide
/// enough that the lattice is what thins — every colony that stands apart
/// is drawn, and the order only settles which of two in one patch it is.
///
/// `d` is the cell's own place in `0..1`, as the share's dither is
/// ([`Share::wanted`]): a patch of small cells each due a third of a mark
/// draws one in a third of them rather than one in none.
///
/// The other rules each drew a gap. Offered cell by cell in the plan's
/// order, the ceiling ran out partway through the walk and the cells after
/// it drew nothing, a whole side of the view at once. Offered a round
/// apiece, a big cell drew no more than a small one, and one holding
/// hundreds over a wide patch stood among small ones drawn whole as a false
/// gap — measured over `.index/full` at eight hundred light years back,
/// columns of sky drawing anywhere from 8% to 32% of their colonies, where
/// in proportion they draw 12% to 16%. Offered by density, what each cell's
/// box made of its colonies decided it, and clusters of small cells drew
/// thinner than the sky about them.
///
/// `weigh` brings a cell's verdicts (and its strata, along star class)
/// forward, answering whether they are current; a cell it puts off offers
/// nothing this pass, and the pass runs again. The cells are weighed in
/// `weighing`'s order, so the budget reaches them all over the view at once
/// ([`weighing`]).
///
/// **What the map draws already claims first, while the view holds still.**
/// `held` says whether a system is on the map, and answers only while the
/// lattice, the bubble and the filters are what they were the pass before
/// ([`ClaimedUnder`]). A view filling in has more cells weighed each pass
/// than the last, and ranked afresh a colony on the map lost its patch to
/// whichever newly weighed one fell due before it — measured over
/// `.index/full` at nine thousand light years back, 5,590 colonies built
/// and dropped again on the way to 6,431. Held across a change of view it
/// kept the last view's picks standing in the next, the zoom that moved
/// them never reaching the screen; so a view that moves claims afresh.
fn claim_admitted(
    marks: &[galos_index::read::walk::MarkRef],
    weighing: &[u32],
    resident: &ResidentCells,
    orders: &mut PointOrders,
    mut weigh: impl FnMut(&mut PointOrders, CellId, &[CellSystem]) -> bool,
    reaches: impl Fn(CellId) -> bool,
    held: impl Fn(i64) -> bool,
    center: DVec3,
    bubble: Option<f64>,
    lattice: impl Fn() -> Crowded,
    ceiling: usize,
) -> Vec<Vec<u32>> {
    let _zone = info_span!("claiming the admitted").entered();
    // Weighed first, every cell the budget reaches, so the orders are read
    // below without holding them open across a weighing.
    let mut weighed: Vec<(u32, &[CellSystem])> = Vec::new();
    for &offer in weighing {
        let mark = &marks[offer as usize];
        if !reaches(mark.id) {
            continue;
        }
        let Some(cell) = resident.0.cell(mark.id) else { continue };
        if weigh(orders, mark.id, &cell.points) {
            weighed.push((offer, &cell.points));
        }
    }
    // Each weighed cell's admitted in the order it draws them, in the
    // weighing's order, which is [`rank`]'s. Along the colored axis that is
    // the strata's order and has to be worked out; otherwise it is the
    // admitted list itself, in the payload's order.
    //
    // And only what stands inside the bubble, so a cell the edge cuts is
    // weighed for what it can draw: counted whole, its outside took places
    // in its order and was then passed over, and the bubble's rim drew
    // thinner than the sky within it.
    let inside = |at: [f64; 3]| {
        !bubble.is_some_and(|radius| center.distance(DVec3::from(at)) > radius)
    };
    let cells: Vec<_> = weighed
        .into_iter()
        .filter_map(|(offer, points)| {
            let id = marks[offer as usize].id;
            let admits = orders.admits(id);
            let bounds = id.bounds();
            let whole = bubble.is_none_or(|radius| {
                center.distance(DVec3::from(bounds.center()))
                    + id.edge_ly() * 3f64.sqrt() / 2.
                    <= radius
            });
            let kept = |index: &u32| inside(points[*index as usize].position);
            let order = match orders.strata(id) {
                Some(strata) => Cow::Owned(
                    strata
                        .iter()
                        .copied()
                        .filter(|index| admits.binary_search(index).is_ok())
                        .filter(|index| whole || kept(index))
                        .collect(),
                ),
                None if whole => Cow::Borrowed(admits),
                None => {
                    Cow::Owned(admits.iter().copied().filter(kept).collect())
                }
            };
            (!order.is_empty()).then_some((offer, points, order))
        })
        .collect();
    // Claimed as they fall due, in `DUE` steps: a counting sort of every
    // candidate by its step, the cells in rank order within one. Linear in
    // the candidates, where sorting them outright took a zooming frame's
    // pass from 9.4 ms to 14.8 at the median at nine thousand light years
    // back, measured over `.index/full` with the uninhabited hidden.
    const DUE: usize = 1024;
    let dither: Vec<f32> = cells
        .iter()
        .map(|(offer, _, _)| {
            let id = marks[*offer as usize].id;
            let mixed = id.morton().wrapping_mul(0xbf58_476d_1ce4_e5b9);
            (mixed >> 40) as f32 / (1u64 << 24) as f32
        })
        .collect();
    // Every candidate, cell by cell in rank order: which it is, when it
    // falls due, and whether the map draws it already.
    let mut whose: Vec<(u32, u32)> = Vec::new();
    let mut dues: Vec<u16> = Vec::new();
    let mut holds: Vec<bool> = Vec::new();
    for (cell, (_, points, order)) in cells.iter().enumerate() {
        let n = order.len() as f32;
        for (j, &index) in order.iter().enumerate() {
            whose.push((cell as u32, j as u32));
            let due = ((j as f32 + dither[cell]) / n * DUE as f32) as usize;
            dues.push(due.min(DUE - 1) as u16);
            holds.push(held(points[index as usize].id64 as i64));
        }
    }
    // The candidates by `step`, in `steps` of them, stably.
    let queue = |step: &dyn Fn(usize) -> usize, steps: usize| {
        let mut starts = vec![0u32; steps + 1];
        for candidate in 0..whose.len() {
            starts[step(candidate) + 1] += 1;
        }
        for at in 0..steps {
            starts[at + 1] += starts[at];
        }
        let mut queued = vec![0u32; whose.len()];
        for candidate in 0..whose.len() {
            let at = &mut starts[step(candidate)];
            queued[*at as usize] = candidate as u32;
            *at += 1;
        }
        queued
    };
    // Claim down `queued` on a fresh lattice, answering the claims and the
    // step the ceiling cut at, where it did.
    let claim = |queued: &[u32]| {
        let mut crowded = lattice();
        let mut claims = vec![Vec::new(); marks.len()];
        let mut claimed = 0usize;
        let mut cut = None;
        for &candidate in queued {
            if claimed >= ceiling {
                break;
            }
            let (cell, j) = whose[candidate as usize];
            let (offer, points, order) = &cells[cell as usize];
            let index = order[j as usize];
            if crowded.claim(points[index as usize].position) {
                claims[*offer as usize].push(index);
                claimed += 1;
                if claimed == ceiling {
                    cut = Some(usize::from(dues[candidate as usize]));
                }
            }
        }
        (claims, cut)
    };
    let fair = queue(&|candidate| usize::from(dues[candidate]), DUE);
    if !holds.contains(&true) {
        return claim(&fair).0;
    }
    // **Held, but only to its share.** What the map draws claims first
    // where it falls due before the ceiling would cut a view claimed
    // afresh, and in its turn past that. Held whole, the cells weighed
    // first kept more than their share of a ceiling the later ones then
    // found spent, and columns of dense sky drew as little as 4% of their
    // colonies beside others at 11%.
    let cut = claim(&fair).1.unwrap_or(DUE);
    let first = |candidate: usize| {
        let due = usize::from(dues[candidate]);
        match holds[candidate] && due <= cut {
            true => due,
            false => DUE + due,
        }
    };
    claim(&queue(&first, 2 * DUE)).0
}

/// What a populated choice was made against
///
/// Remade when any of this moves, and neither a still view nor a turning
/// one moves any of it. The centre is in it twice over: the lattice
/// sizes its cells by how far off the galaxy is and bins its lines of
/// sight about that point, so travelling changes the answer where
/// turning does not. The plan is *not* in it — which cells are marked
/// decides only what each mark is booked against. See [`Crowded::about`]
/// and [`book_populated`].
///
/// Whether the excluded are drawn at all is in it too: below the dim
/// they are not chosen, and a choice left standing across the slider
/// held them in the draw where nothing would build or evict them — the
/// pass offering them again every frame and never settling — or, the
/// other way, left them off the map until the camera moved. The table
/// the choice is made from is not a value worth keeping here; a new one
/// is caught by its change mark instead (see [`reconcile`]).
#[derive(Default, PartialEq)]
pub(crate) struct Against {
    filters: u32,
    fill: bool,
    bubble: u64,
    about: [u64; 3],
    pitch: u64,
}

/// What the population scale is drawing, and what it was settled
/// against: kept between frames, since nothing a frame does changes it.
#[derive(Default)]
pub(crate) struct Chosen {
    against: Against,
    /// Every mark the mode will make, with the deepest cell of the tree
    /// that holds it.
    chosen: Vec<(i64, [f64; 3], CellId)>,
    /// And the cell of the plan each is accounted against.
    booked: Vec<(CellId, i64, [f64; 3])>,
}

/// What one pass of [`reconcile`] keeps for the next
#[derive(Default)]
pub(crate) struct Pass {
    /// What the population scale settled on; see [`Chosen`].
    chosen: Chosen,
    /// Where the camera stood when the last pass found nothing left to do,
    /// or [`None`] where it did not; see [`Settled`].
    settled: Option<Settled>,
    /// The plan's marks in the order their verdicts are weighed, each by
    /// its place in the plan; see [`weighing`].
    weighing: Vec<u32>,
    /// What the last pass's claims were made under; see [`ClaimedUnder`].
    claimed_under: Option<ClaimedUnder>,
}

/// What a filtered view's claims turn on, besides which cells are weighed
/// ([`claim_admitted`]): the lattice — reckoned about the centre, at the
/// octave of the eye's distance, and as fine as the pixel pitch — the
/// bubble and the filters. While none of it moves, what is drawn holds its
/// patch; a turn of the camera moves none of it.
#[derive(PartialEq)]
pub(crate) struct ClaimedUnder {
    against: Against,
    back: i32,
}

/// The plan's marks in the order the verdict budget reaches them: coarse
/// before fine and scattered by address within a level, as the reads are
/// ([`rank`])
///
/// **Not the plan's own order.** The walk hands its marks over a subtree
/// at a time, so a budget spent down the plan brought the filters in a
/// patch of sky after the next — reported as a filter loading left to
/// right and top to bottom, every other load filling in all over at once.
///
/// Sorted on keys worked out once a mark, each packed with its place:
/// [`rank`] interleaves an address's bits, and asked of the comparator it was
/// worked out twice a comparison — four milliseconds of every moving frame
/// over the forty-odd thousand marks of the galaxy seen whole.
fn weighing(marks: &[galos_index::read::walk::MarkRef]) -> Vec<u32> {
    let mut keyed: Vec<u128> = marks
        .iter()
        .enumerate()
        .map(|(offer, mark)| {
            let (_, level, scattered) = rank(mark.id, true);
            u128::from(level) << 96
                | u128::from(scattered) << 32
                | offer as u128
        })
        .collect();
    keyed.sort_unstable();
    keyed.into_iter().map(|key| key as u32).collect()
}

/// What a finished pass was worked out from, besides the resources it reads
///
/// **A pass that found nothing to do is not run again until something it
/// reads moves.** The walk used to be worked through every frame, a still
/// view included: measured over `.index/full` at a wide zoom, 8–10 ms of
/// every settled frame re-deciding the tens of thousands of marks it had
/// decided the frame before. What it reads is the resources it is handed,
/// whose change marks say whether they moved, and the camera, which is
/// written every frame whether it moves or not — so the camera's part is
/// kept by value and compared.
///
/// Finished means it offered nothing, pushed nothing and put no cell off
/// for a later budget: a pass that is still filling a view in or bringing
/// the filters' verdicts forward a budget at a time runs again next frame
/// whatever moved. Budget left over is not the test. A cell bigger than
/// what is left is put off with most of it unspent, and a pass that settled
/// on that left the cell unweighed for as long as the camera stood still —
/// reported as half the colonies on screen standing as a blur, the uninhabited
/// hidden, until the view was turned a hair.
#[derive(PartialEq)]
struct Settled {
    view: galos_index::prelude::View,
    center: DVec3,
    eye: DVec3,
    bubble: Option<f64>,
}

/// What the populated draw settles on: every mark it will make
///
/// **Once a view, and not once a frame.** What this answers turns on
/// the filters, the dim, the table and where the view is centred — see
/// [`Against`] — not on the plan, and not on which way the camera is
/// pointed, the lattice being reckoned about the centre (see
/// [`Crowded::about`]). On a still view none of
/// it moves, and on a turning one none of it moves either, while the
/// answer itself is a hundred and fifty thousand systems weighed
/// against a lattice: measured over `.index/full` with the reach at
/// five hundred light years, doing that per frame cost **69 ms of every
/// settled frame**.
///
/// The cell each mark is booked against is *not* settled here, that one
/// being a question about the plan; see [`book_populated`].
///
/// **What the filters admit claims the sky first.** Taken busiest first
/// whatever the filters said, the excluded — busier, in the core, than
/// the faction asked for — took the patches and the ceiling, and the
/// admitted there were left to the field while dimmed marks of what
/// nobody asked for stood in their place. So the admitted are offered
/// the lattice and the ceiling first, and the excluded, still busiest
/// first, only what they leave — and only while they are drawn at all
/// (`fill`). The same order [`busiest_first`] and [`claim_admitted`]
/// keep, and for the same reason.
fn choose_populated(
    cells: &crate::map::galaxy::populated::PopulatedOrder,
    populated: &Populated,
    filters: &Prepared<'_>,
    now: DateTime<Utc>,
    fill: bool,
    view: &galos_index::prelude::View,
    about: DVec3,
    bubble: Option<f64>,
) -> Vec<(i64, [f64; 3], CellId)> {
    let _zone = info_span!("choosing the populated").entered();
    let mut chosen = Vec::new();
    // One mark to a mark's worth of sky, wherever in the galaxy that
    // sky stands; see [`Crowded::about`].
    let mut crowded = Crowded::about(view, about.to_array());
    // And no more of them than the frame can carry and still be read;
    // see [`View::crowded_marks`](galos_index::read::walk::View::crowded_marks). Spent busiest first, which is the order
    // the table is in.
    let ceiling = view.crowded_marks() as usize;

    // **One pass over the galaxy's own order, busiest first.** Asked
    // cell by cell this walked the same systems once per cell on their
    // path — 1,559,152 entries scanned over 831 cells to choose 25,744
    // marks, measured over `.index/full` with the reach at five hundred
    // light years. The lattice is what thins the answer and the lattice
    // has nothing to do with cells, so there is nothing for the cells to
    // decide: the busiest are offered first and the sky settles what
    // fits.
    //
    // The excluded are put by as they are met, in that same order, for a
    // second turn at what the admitted leave.
    let mut excluded = Vec::new();
    for stands in cells.order() {
        if chosen.len() >= ceiling {
            break;
        }
        // The bubble before the lattice, a system the frame will not
        // draw being one that must not claim sky and leave it empty.
        if bubble.is_some_and(|radius| {
            about.distance(DVec3::from(stands.at)) > radius
        }) {
            continue;
        }
        if filters.asking()
            && !filters.admits(
                &Candidate::off_the_table(
                    stands.address,
                    populated,
                    None,
                    StarKind::Unknown,
                ),
                now,
            )
        {
            if fill {
                excluded.push(stands);
            }
            continue;
        }
        if !crowded.claim(stands.at) {
            continue;
        }
        chosen.push((stands.address, stands.at, stands.deepest));
    }
    for stands in excluded {
        if chosen.len() >= ceiling {
            break;
        }
        if !crowded.claim(stands.at) {
            continue;
        }
        chosen.push((stands.address, stands.at, stands.deepest));
    }
    chosen
}

/// Which cell each chosen mark is accounted against
///
/// A mark has to be booked against a cell the plan marks: the field
/// subtracts what a cell has drawn from the light it lays for that cell,
/// so a mark nobody accounts for is a mark drawn twice. Walked up from
/// the deepest cell the tree has for it, which is at most a dozen steps
/// and only for what is drawn — cheap enough to redo whenever the plan
/// moves, which is what it turns on, where the choice itself is not.
fn book_populated(
    chosen: &[(i64, [f64; 3], CellId)],
    marked: &FxHashSet<CellId>,
) -> Vec<(CellId, i64, [f64; 3])> {
    let _zone = info_span!("booking the populated").entered();
    chosen
        .iter()
        .filter_map(|&(address, at, deepest)| {
            let mut id = deepest;
            loop {
                if marked.contains(&id) {
                    break Some((id, address, at));
                }
                id = id.parent()?;
            }
        })
        .collect()
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
/// (see [`crate::map::pointing`]): the excluded are the space the
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
/// [`crate::map::galaxy::spawn::spawn_systems`], never becomes an entity, and is queued
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
/// The marks' counterpart to [`crate::map::paint::glow::Laid`]: what the frontier asked
/// of every marked, resident, in-reach cell, how much of it the map has and
/// drew, and how many merged marks stand over the rest of the sky.
///
/// There is no share here and there is nothing to divide by. The walk's ask
/// is already one mark to every [`galos_index::MERGE_PX`] squared of the cell
/// covers, so what bounds the frame is the frame's own area and not a factor
/// struck across every cell — see [`galos_index::prelude::Index::walk_screen`].
/// What this reports is therefore a fact about the view rather than a dial: if
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
    /// [`galos_index::read::screen::Empty`].
    pub lit: usize,
    /// How many systems those merged marks stand for.
    pub behind: u64,
}

/// Draw the prefix of every marked cell the frontier asks for, admitted
/// systems first, grown and shed per system as the camera moves
///
/// **The walk says how many, and there is nothing to divide.** A cell's
/// payload is in standing order and [`galos_index::read::walk::MarkRef::wanted`] is how
/// many of it the cell's footprint holds apart at [`galos_index::MERGE_PX`]
/// to every merge distance squared of the patch of screen the cell's
/// contents cover. Drawing that many, and only that many, is what lets a
/// cell fill in and empty one system at a time rather than switching on
/// whole, and what keeps two neighbours at the same density however
/// differently their boxes fell.
///
/// What the sky *under* those marks comes to is the walk's too: everything
/// finer than one mark is merged into [`galos_index::read::walk::BlobRef`]s, drawn by
/// [`crate::map::paint::field`] off the aggregates with no payload at all. So this pass
/// no longer has an overrun to spend. It used to: the per-cell rule bounds a
/// cell and not a frame, the marked prefixes of a wide view came to tens of
/// times what a frame could show, and every cell was thinned by one global
/// `fair_share` with each system holding a fixed place in it and the share
/// carried across cell faces to keep the density from stepping. All of that
/// is gone with the floor that made it necessary.
///
/// *Which* systems fill the count is the filters' to say. The count is a
/// budget of marks the screen can tell apart, a share of the frame's
/// capacity. Taking the brightest of the payload instead spends the budget
/// on whatever happens to be bright: a faction is a handful of systems in a
/// cell of thousands, so a filter on one used to draw nothing at all from
/// most cells while the marks the screen could carry went unused. And what
/// the filters admit is not held to that count at all: it is drawn wherever
/// it stands apart on the sky ([`claim_admitted`]), so a map narrowed to the
/// colonies draws every colony of an arm and one to a patch of the core.
///
/// So the order is: what the filters admit, and then — only where
/// [`crate::map::filter::DimTo`] still draws the excluded — the rest, to
/// fill whatever the admitted left. The excluded are what a short budget sheds
/// first, which is what they are for: the space a faction is read against
/// gives way to the faction. Where the admitted alone overrun the budget they
/// decimate among themselves in the cell's order. Where nothing is asked
/// every system is admitted, and the order is the cell's own.
///
/// **And the cell's own order is the view's.** On the map each half is drawn
/// along the axis the map is colored by, every value of the cell taking its
/// proportion of the budget ([`strata`]): the coloring is of the systems, so
/// its marks are a sample of them. In the sky each half is drawn brightest
/// first, off the light beside the payload ([`bright`]), which is what the
/// floor cuts short. The payload's own order, standing, is no view's and is
/// what a cell not yet ordered draws in.
///
/// What this gives up is that the drawn set is no longer a prefix of the
/// payload: it is a subset chosen by admission, in the cell's order within
/// each half. Nothing reads it as a prefix today.
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
/// on naming it, so [`crate::map::galaxy::fetch`]'s `fetch_selected` builds the star again
/// the moment [`crate::map::selection`]'s `follow_selection` rewrites the row, and
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
    holding: Res<Entered>,
    spyglass: Res<Spyglass>,
    view_mode: Res<View>,
    selection: Res<crate::map::selection::Selection>,
    filtering: Filtering,
    cut: Res<Cut>,
    scale_population: Res<ScalePopulation>,
    mut worked: Worked,
    systems: Query<(Entity, &System, Has<crate::map::route::Hop>)>,
    mut pending: ResMut<PendingSpawns>,
    mut evictions: ResMut<PendingEvictions>,
    // What the population scale settled on, and what it was settled
    // against; see [`choose_populated`].
    mut pass: Local<Pass>,
) {
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = crate::map::galaxy::plan::view(orbit, camera) else {
        return;
    };
    let Worked {
        ref populated_order,
        ref standing,
        ref mut orders,
        ref mut republished,
        ref mut keeping,
        ref planned,
        ref mut drawn,
        ref mut sampled,
        ref mut blobs,
        ref addresses,
        ref color_by,
    } = worked;
    // Nothing to do where the last pass finished and nothing it reads has
    // moved since; see [`Settled`]. Clearing, the spyglass clamps the drawn
    // set to a bubble about the camera: the LOD is untouched inside it, only
    // the far tail is shed.
    let bubble = reach(&spyglass);
    let here =
        Settled { view, center: orbit.center(), eye: orbit.eye(), bubble };
    let moved = resident.is_changed()
        || populated.is_changed()
        || names.is_changed()
        || holding.is_changed()
        || spyglass.is_changed()
        || view_mode.is_changed()
        || selection.is_changed()
        || filtering.filters.is_changed()
        || filtering.dim.is_changed()
        || cut.is_changed()
        || scale_population.is_changed()
        || populated_order.is_changed()
        || standing.is_changed()
        || republished.is_changed()
        || planned.is_changed()
        || addresses.is_changed()
        || color_by.is_changed();
    if !moved && pass.settled.as_ref() == Some(&here) {
        return;
    }
    // The order the verdicts are weighed in, remade with the plan; see
    // [`weighing`]. Taken for the pass and put back at its end.
    let mut weighed_in = std::mem::take(&mut pass.weighing);
    if planned.is_changed() || weighed_in.len() != planned.0.marks.len() {
        let _zone = info_span!("weighing order").entered();
        weighed_in = weighing(&planned.0.marks);
    }
    // What the last pass's claims were made under, taken for the same
    // reason; see [`ClaimedUnder`].
    let claimed_before = pass.claimed_under.take();
    let choice = &mut pass.chosen;
    // Whether this pass asked for anything to be built; see [`Settled`].
    let mut wanting = false;
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
    // own width in [`galos_index::prelude::Mode::Shell`] and the point spread
    // in [`galos_index::prelude::Mode::Real`], which is why a cluster stays a
    // field of stars in the sky where the map would collapse it to one mark.
    if planned.is_changed() {
        let _zone = info_span!("marked set").entered();
        keeping.marks(&planned.0.marks);
    }
    let now = Instant::now();

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
    // ([`crate::map::galaxy::flight`]). Looking the entity up here means a
    // resolved point that is already drawn marks its entity
    // ([`EntityHashSet`], bevy's own numbering and a cheap hash) and one that
    // is not goes to the queue, which is one lookup a point rather than two.
    // Out of the index the systems keep themselves
    // ([`crate::map::galaxy::Addresses`]) rather than a map built here over
    // every drawn system every frame.
    let existing = &**addresses;
    let picked: HashSet<i64> = selection.addresses().into_iter().collect();
    // Every stop of every route being shown. A line is only a line if it has
    // both ends of each leg to draw between, so these are wanted whatever the
    // walk resolves and wherever the bubble ends. See [`Filters::routed`].
    let routed = filtering.filters.routed();

    // With nothing asked every system is admitted, so there is no order to
    // impose: the admitted lists are dropped and the fill draws the payload in
    // its own order, which is what this did before the filters had a say.
    let asking = filtering.filters.asking();
    // Whether the excluded are wanted on screen at all. Below the dim they are
    // never spawned ([`crate::map::galaxy::spawn`]) and queued to drop by this pass, so
    // queueing them is a slot of the spawn budget spent on a system that
    // cannot land and rebuilt again next frame.
    let fill = !asking || filtering.excluded_are_drawn();
    // Whether the mask only thins the sky, which is then drawn as it is
    // unfiltered rather than claimed; see [`Filters::only_thins`].
    let thins = filtering.filters.only_thins();
    // And where what it thins is still drawn, dimmed, the order is the
    // unfiltered draw's own — the payload's, or star class's strata — and
    // there is nothing to weigh: which of the drawn are dimmed is asked of
    // the drawn alone ([`crate::map::filter`]). Weighed, every resident
    // point was walked for a list admitting most of it.
    let unweighed = thins && fill;
    orders.hold(cut.0, asking && !unweighed);
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
    // Whether a cell was put off for a later budget; see [`Settled`].
    let mut deferred = false;
    // The addresses the filters name, gathered once for the pass rather
    // than walked per point: a route of three hundred stops is what made
    // having one on the map cost seven times what any other filter does.
    // See [`Filters::prepared`].
    let asked_for = filtering.filters.prepared();
    // One clock for the pass, as the spawn batch takes one: a span's near edge
    // moves by a frame's worth in a frame.
    let wall = Utc::now();

    // **The sky needs no budget, and the map's would be wrong for it.** A
    // share of the population is how the political map says density: there
    // every mark is the same size, so how many there are is the only thing
    // that can carry it. In the photometric sky the density says itself —
    // a crowded region deposits a thousand stars' flux — and the cut that
    // bounds the frame is the one the eye already has: apparent magnitude
    // against the exposure's floor. Rationing on top of it drew a uniform
    // sample of the galaxy instead of the sky, spending its marks on the
    // dwarfs of crowded cells while the naked-eye stars of coarse ones went
    // undrawn: measured over `.index/full` standing at Sol at a thousand
    // light years of reach, 47,670 stars of median apparent magnitude
    // **10.55**, 250 of them naked-eye, and not one star of the Big Dipper.
    //
    // So the share is the Shell mode's alone, and [`Mode::Real`] carries
    // the limit each star is weighed against instead.
    let limit = match planned.0.mode {
        galos_index::prelude::Mode::Real { limit } => Some(limit),
        galos_index::prelude::Mode::Shell => None,
    };
    // Whether each cell's budget is spent by its values along the axis the
    // map is colored by; see [`strata`]. Every view on the map is: the sky
    // spends it brightest first instead, and the populations busiest first.
    let along = Along::of(**color_by, &populated);
    let stratified = limit.is_none() && !by_population;
    let along_key = stratified.then(|| along.key(orders.cut));
    orders.stratify_along(along_key);
    if limit.is_none() {
        orders.unbrighten();
    }
    // What the frame has to spend and what it is spread over. A share of
    // the population is a share of every *other* marked cell's too, so the
    // whole has to be known before any of it is spent — which is a sum
    // over the plan's own counts and touches neither the index nor a
    // payload.
    let population = population(&planned.0);
    let share = Share::of(population, view.marks());
    // Whether each mark and each merged mark stands in reach, and what the
    // share draws of each mark, worked out once a pass. The dark tiles, the
    // strata, the draw and the merged cells each asked them again of every
    // one of the hundred and forty thousand with the galaxy seen whole: a
    // box and a distance, and a hash of the address, apiece.
    let marks_reached = across(planned.0.marks.len(), |offer| {
        in_reach(planned.0.marks[offer].id, orbit, bubble)
    });
    let marks_wanted = across(planned.0.marks.len(), |offer| {
        let mark = &planned.0.marks[offer];
        share.wanted(mark.slice as usize, mark.id)
    });
    let blobs_reached = across(planned.0.blobs.len(), |offer| {
        in_reach(planned.0.blobs[offer].id, orbit, bubble)
    });
    // **Under a filter, what it admits is drawn where it stands apart.**
    // The share above is struck over every system the marked cells hold,
    // and a cell's budget off it is a share of its whole slice: with the
    // uninhabited hidden, the colonies a hundredth of the systems about
    // them, that left each cell a fraction of a mark for its dozens of
    // colonies, the dither deciding cell by cell which drew one and which
    // stood as a blur, and a turn of the camera moving the population
    // enough to decide it again. Measured over `.index/full` at nine
    // thousand light years back, the cells' budgets reached 3,783 of the
    // 142,414 colonies they held. A share struck over the admitted instead
    // spreads the screen in proportion to them, and the core, where most
    // of them are, took most of it and washed out.
    //
    // So the admitted are drawn as the populated draw draws them: each
    // claims a mark's patch of sky, and one that would land on a patch
    // already claimed is left to the field; see [`claim_admitted`]. An
    // arm's colonies stand apart and are drawn every one; the core's pile
    // up and are one mark to a patch. The excluded, where the dim draws
    // them, fill what the cell's share leaves, as before. Not where the mask
    // only thins the sky: what it admits is most of what the share draws,
    // and claimed, every point of every cell was a candidate.
    let narrowed = asking && !thins && limit.is_none() && !by_population;
    // Whether what is on the map claims first: only while the view the
    // last claims were made under still stands; see [`claim_admitted`].
    let under = ClaimedUnder {
        against: Against {
            filters: filtering.filters.revision(),
            fill,
            bubble: bubble.unwrap_or(f64::INFINITY).to_bits(),
            about: orbit.center().to_array().map(f64::to_bits),
            pitch: view.pixels_per_radian().to_bits(),
        },
        back: orbit.center().distance(DVec3::from(view.eye)).log2().round()
            as i32,
    };
    let keep_claims = narrowed && claimed_before.as_ref() == Some(&under);
    let claims = match narrowed {
        false => Vec::new(),
        true => claim_admitted(
            &planned.0.marks,
            &weighed_in,
            &resident,
            orders,
            |orders, id, points| {
                let weighed = orders.walk(
                    id,
                    points,
                    &asked_for,
                    &populated,
                    wall,
                    by_population,
                    &mut verdicts,
                );
                // Along star class only: a claim draws what the filters
                // admit wherever it stands apart, and the strata only say
                // which of two in one patch it is — which along a political
                // axis is not worth a second walk of the payload in the
                // frame's budget.
                let stratified = !(stratified && along.is_star_class())
                    || orders.stratify(id, points, &along, &mut verdicts);
                deferred |= !(weighed && stratified);
                weighed
            },
            |id| in_reach(id, orbit, bubble),
            |address| keep_claims && existing.get(address).is_some(),
            orbit.center(),
            bubble,
            || Crowded::about(&view, orbit.center().to_array()),
            view.crowded_marks() as usize,
        ),
    };
    // And the strata, off the same budget and in the same order, for the
    // cells whose share draws; the draw below then finds them worked out.
    // Left to the draw, they came in down the plan's order and the values
    // filled in a patch at a time.
    if stratified && !narrowed {
        let _zone = info_span!("strata").entered();
        for &offer in &weighed_in {
            let offer = offer as usize;
            if !marks_reached[offer] || marks_wanted[offer] == 0 {
                continue;
            }
            let mark = &planned.0.marks[offer];
            if let Some(cell) = resident.0.cell(mark.id) {
                deferred |= !orders.stratify(
                    mark.id,
                    &cell.points,
                    &along,
                    &mut verdicts,
                );
            }
        }
    }
    // Where this pass takes its systems from. Drawing by population draws
    // the systems anybody lives in, and every one of those is resident in
    // full — so the cell's own populated systems answer, exactly and the same however
    // the eye arrived, where a payload prefix answers with the busiest of
    // whatever happened to land. See [`crate::map::galaxy::populated::PopulatedOrder`].
    //
    // Unless a span is asked. A moment is a fact only a payload point
    // carries, so a system taken off the populated table is one a span can
    // say nothing about; while one is on the map this falls back to the
    // payload, hysteresis and all. A moment per populated row would close
    // it.
    let from_the_table = by_population && !filtering.filters.asking_a_span();
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
    // Nothing to light in the sky: a tile is dark there because nothing in
    // it clears the floor, which is the honest answer and not a hole to
    // fill. The promotion is also a *screen* lattice, so its tiles sweep
    // the sky as the camera turns — see [`Crowded::about`].
    let lit: Vec<u32> = if limit.is_some() {
        Vec::new()
    } else {
        let _zone = info_span!("dark tiles").entered();
        let mut lighting = Empty::over(&view);
        for (offer, mark) in planned.0.marks.iter().enumerate() {
            if !marks_reached[offer] {
                continue;
            }
            match marks_wanted[offer] {
                0 => lighting.offered(
                    mark.at,
                    u64::from(mark.slice),
                    offer as u32,
                ),
                take => lighting.drew(mark.at, take),
            }
        }
        for (offer, blob) in planned.0.blobs.iter().enumerate() {
            if !blobs_reached[offer] {
                continue;
            }
            let offer = (planned.0.marks.len() + offer) as u32;
            match share.scaled(blob.blend).wanted(blob.count as usize, blob.id)
            {
                0 => lighting.offered(blob.at, blob.count, offer),
                _ => lighting.drew(blob.at, 1),
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
    // The walk's offers are this pass's: what the last one offered and the
    // budget never reached is gone, and what is still wanted is offered again
    // below. See [`crate::map::galaxy::spawn::PendingSpawns`].
    pending.opening(now);
    // What every cell would have drawn and is not drawn yet, gathered over
    // the whole plan before any of it is offered; see [`Unspawned`].
    let mut unspawned: Vec<Unspawned> = Vec::new();
    // Over the plan's marks and not over everything the map holds. The
    // two differ by whatever [`KEEP`] is still holding onto and by
    // everything outside the bubble, and at a wide zoom that is twice the
    // set: measured over `.index/full`, 117,274 payloads held against
    // 60,229 the plan marks. A held cell the plan does not name draws
    // nothing, so walking it only to skip it is the pass done twice.
    let prefixes =
        info_span!("cell prefixes", cells = planned.0.marks.len()).entered();
    // And nothing out of the payloads while the sky is read as
    // populations: that mode draws the systems with a population, takes
    // every one of them off the resident table, and is the pass below.
    // See [`choose_populated`].
    let by_payload = match from_the_table {
        true => &[][..],
        false => &planned.0.marks[..],
    };
    // **Asked down the plan, taken across the pool.** Everything that
    // writes to the orders — the brightness, the filters' verdicts and the
    // strata, each off a budget spent in the plan's order — is worked out
    // here, a cell after the next as it always was, and what is left of a
    // cell is a read: its order taken to its target, each point held to the
    // bubble, looked up as drawn or not and summed into its account. That is
    // the bulk of the pass, a few lookups a point over tens of thousands of
    // points, and it was four to seven milliseconds of every frame of a pan
    // with the galaxy seen whole on one thread. Taken in chunks across the
    // pool and folded back in the plan's order, so what is offered, and in
    // what order, is what it was.
    let mut jobs: Vec<Job> = Vec::new();
    for (offer, mark) in by_payload.iter().enumerate() {
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
        // first it draws, which is the head of its order. See
        // [`Empty`] — and the payload is in hand for it, every marked cell
        // in reach being read to [`READ_LEAST`] whatever its share.
        // In the sky the ask is the cell's whole slice: which of it draws is
        // per star, against the floor, just below. The walk has already
        // dropped every subtree whose brightest cannot clear it
        // (`node_visible`), so a cell reaching here holds at least one.
        let asked = match limit {
            Some(_) => mark.slice as usize,
            None => marks_wanted[offer].max(usize::from(is_lit(offer as u32))),
        };
        // A narrowed cell draws what won its patches of sky, share or no
        // share; see [`claim_admitted`]. One that won none and whose share
        // draws nothing is skipped here as any other, before its payload is
        // looked up.
        let won = narrowed && !claims[offer].is_empty();
        if asked == 0 && !won {
            continue;
        }
        let Some(cell) = resident.0.cell(id) else { continue };
        // In the sky, the cell's points brightest first, off its light;
        // see [`bright`].
        if limit.is_some() {
            deferred |= !orders.brighten(id, cell, &mut verdicts);
        }
        // How far down the cell's brightest-first order the floor
        // reaches. The stars that can clear the floor are a prefix of
        // it: measured at the *nearest* face of the cell, which is the
        // most generous distance modulus anything in it can have, so the
        // prefix never cuts a star the exact test below would have kept.
        // Without it a wide view would walk every point of every marked
        // cell — tens of millions — to find the few thousand that draw.
        // A cell whose order the budget has not reached yet is weighed
        // whole, point by point.
        let target = match limit {
            None => asked.min(cell.points.len()),
            Some(limit) => {
                let near = id.bounds().distance_to(orbit.eye().to_array());
                let faintest = match near > 0.0 {
                    true => {
                        limit
                            - Magnitude(0.0)
                                .apparent(Distance::light_years(near))
                                .0
                    }
                    false => f64::INFINITY,
                };
                match orders.bright(id) {
                    Some(bright) => bright.partition_point(|&at| {
                        f64::from(magnitude_at(cell, at as usize)) <= faintest
                    }),
                    None => cell.points.len(),
                }
            }
        };
        if target == 0 && !won {
            continue;
        }
        // Whether this cell's payload is the one the drawn systems were
        // built from, or a later one; see [`Republished`].
        let refreshed = republished.holds(id);
        // Nothing asked of the filters has no verdict to weigh, and the
        // orders hold none ([`PointOrders::hold`] clears them every pass
        // while nothing is asked): weighed anyway, every drawing cell came
        // back stale every pass, stamped, and charged its whole payload to
        // the budget the strata are worked out on, which went on putting off
        // the strata of cells just come into view.
        if !unweighed && asking {
            deferred |= !orders.walk(
                id,
                &cell.points,
                &asked_for,
                &populated,
                wall,
                by_population,
                &mut verdicts,
            );
        }
        if stratified {
            deferred |=
                !orders.stratify(id, &cell.points, &along, &mut verdicts);
        }
        jobs.push(Job { offer, id, target, refreshed });
    }

    // And each cell taken, which reads and writes nothing shared.
    let orders: &PointOrders = orders;
    let take = |job: &Job| -> Took {
        let Job { offer, id, target, refreshed } = *job;
        let mut took = Took::default();
        let Some(cell) = resident.0.cell(id) else { return took };
        let admits = orders.admits(id);
        let order: Vec<usize> = if by_population {
            busiest_first(orders.populated(id), admits, asking, fill)
                .take(target)
                .collect()
        } else if narrowed {
            // What won its patch of sky, and the excluded after it to
            // fill the cell's share, where the dim draws them.
            let claimed = &claims[offer];
            let left = target.saturating_sub(claimed.len());
            let won = claimed.iter().map(|&index| index as usize);
            match orders.strata(id) {
                Some(strata) => won
                    .chain(
                        stratified_first(strata, admits, fill)
                            .skip(admits.len())
                            .take(left),
                    )
                    .collect(),
                None => won
                    .chain(
                        drawn_first(&cell.points, admits, fill)
                            .skip(admits.len())
                            .take(left),
                    )
                    .collect(),
            }
        } else if let Some(bright) = limit.and(orders.bright(id)) {
            // The sky, brightest first: the same walk as the strata's,
            // in the order the floor was measured down.
            stratified_first(bright, admits, fill).take(target).collect()
        } else if let Some(strata) = orders.strata(id) {
            stratified_first(strata, admits, fill).take(target).collect()
        } else {
            drawn_first(&cell.points, admits, fill).take(target).collect()
        };
        let ranked = rank(id, true);
        let mut round = 0u32;
        for index in order {
            let point = &cell.points[index];
            let (address, pos, kind) =
                (point.id64 as i64, point.position, point.kind);
            // And in the sky, a star that does not clear the floor from
            // where the eye stands is not drawn at all. The prefix above
            // is the cell's best case; this is the star's own.
            if let Some(limit) = limit
                && Magnitude(f64::from(magnitude_at(cell, index)))
                    .apparent(Distance::light_years(
                        orbit.eye().distance(DVec3::from(pos)),
                    ))
                    .0
                    > limit
            {
                continue;
            }
            // What this cell's marks account for, so
            // [`crate::map::paint::glow`] can lay the rest of it down and not
            // the whole. Built here because here is the only place the drawn
            // set is known: it is not a rank range, the filters having
            // promoted systems out of the payload's order, and it is cut
            // again per point by the bubble just below.
            // A cell straddling the bubble draws only the points inside it, so
            // the edge is a sphere about the camera, not the cell grid.
            if let Some(radius) = bubble
                && orbit.center().distance(DVec3::from(pos)) > radius
            {
                continue;
            }

            took.count += 1;
            // Counted before it is queued rather than after it is spawned: a
            // system the budget has not reached yet is one the field would
            // otherwise go on drawing for the frame or two it takes to land,
            // and a mark arriving over light that is already there reads as a
            // flash. Accounting for it now hands the light over on the frame
            // the walk decides, and the spawn catches up under it.
            took.accounted.took(
                pos,
                kind,
                populated
                    .get(address)
                    .filter(|system| system.population > 0)
                    .map(|system| {
                        Inhabited::of_system(pos, Readings::of(system))
                    }),
            );
            // Already drawn is already answered, except out of a cell that
            // has just been published again: then the system on the map was
            // built from the payload this one replaced, and what it says
            // about the moment, the magnitude and the politics is what the
            // index said last time. Gathered either way, and `spawn_systems`
            // replaces it in place.
            //
            // Which point of which cell, and nothing built: most of what a
            // walk gathers is never drawn, and building it to queue it is a
            // name and a political join thrown away. See
            // [`crate::map::galaxy::spawn::Waiting`]. Its place among this
            // cell's own undrawn is the round it is offered in; see
            // [`Unspawned`].
            let on_map = existing.get(address);
            if let Some(entity) = on_map {
                took.wanted.push(entity);
            }
            if on_map.is_none() || refreshed {
                took.unspawned.push(Unspawned {
                    round,
                    rank: ranked,
                    address,
                    cell: id,
                    index: index as u32,
                });
                round += 1;
            }
        }
        took
    };
    let taking_zone = info_span!("taking", cells = jobs.len()).entered();
    // Folded a chunk at a time, as they come back: an account is a few
    // hundred bytes of histograms, and flattened into one list first every
    // one was copied once more for nothing.
    let taking: Vec<Vec<Took>> = if jobs.len() < TAKE_CHUNK * 2 {
        vec![jobs.iter().map(take).collect()]
    } else {
        let take = &take;
        bevy::tasks::ComputeTaskPool::get().scope(|scope| {
            for chunk in jobs.chunks(TAKE_CHUNK) {
                scope.spawn(async move {
                    chunk.iter().map(take).collect::<Vec<_>>()
                });
            }
        })
    };
    drop(taking_zone);
    drawn.0.reserve(jobs.len());
    for (job, took) in jobs.iter().zip(taking.into_iter().flatten()) {
        took_all += took.count;
        wanted_by.extend(took.wanted);
        unspawned.extend(took.unspawned);
        drawn.0.insert(job.id, took.accounted);
        if job.refreshed {
            republished.settled(job.id);
        }
    }
    drop(prefixes);
    // Whether there is room left to offer once they are in, for the
    // populated pass below.
    let offering = {
        let _zone =
            info_span!("offering", unspawned = unspawned.len()).entered();
        wanting |= !unspawned.is_empty();
        offer_in_rounds(&mut pending, unspawned)
    };

    // And the systems the population scale draws, which come off the
    // resident table rather than out of any payload.
    //
    // **Chosen once a plan.** What is chosen turns on the plan, the
    // filters, the dim, the table and where the eye stands — see
    // [`Against`] — and a still view moves none of them; what a frame owes
    // is to mark them wanted and offer whatever is not drawn yet. Measured
    // over `.index/full` with the reach at five hundred light years,
    // choosing afresh every frame was 69 ms of every settled frame.
    if from_the_table {
        let key = Against {
            filters: filtering.filters.revision(),
            fill,
            bubble: bubble.unwrap_or(f64::INFINITY).to_bits(),
            about: orbit.center().to_array().map(f64::to_bits),
            pitch: view.pixels_per_radian().to_bits(),
        };
        let afresh = choice.against != key
            || populated.is_changed()
            || populated_order.is_changed();
        if afresh {
            choice.against = key;
            choice.chosen = choose_populated(
                populated_order,
                &populated,
                &asked_for,
                wall,
                fill,
                &view,
                orbit.center(),
                bubble,
            );
        }
        // And which cell each is booked against, which is the one thing
        // here the plan decides.
        if afresh || planned.is_changed() {
            choice.booked = book_populated(&choice.chosen, keeping.marked());
        }
        let _zone =
            info_span!("the populated drawn", marks = choice.booked.len())
                .entered();
        for &(id, address, at) in &choice.booked {
            took_all += 1;
            // No payload point behind a pick off the table, so no star: the
            // star axis is not offered while the population scale draws
            // these; see `follow_color_by`.
            drawn.0.entry(id).or_default().took(
                at,
                StarKind::Unknown,
                populated.get(address).map(|system| {
                    Inhabited::of_system(at, Readings::of(system))
                }),
            );
            match existing.get(address) {
                Some(entity) => {
                    wanted_by.insert(entity);
                }
                None => {
                    if offering {
                        // Built from the row: a system the names table
                        // cannot name is still a system with a
                        // population, and the payload path names one by
                        // its address. See [`System::build`].
                        let system = System::build(
                            address, at, None, None, &populated, &names,
                        );
                        pending.push(system, false, true, now);
                        wanting = true;
                    }
                }
            }
        }
    }
    // And the cells the walk merged, which need nothing read. One mark
    // apiece and no more — a blob is a cell whose whole contents fall inside
    // one mark, so one is what it is worth — and it is drawn or not on the
    // same share every read cell is thinned by, so a merged region is no
    // denser or thinner on screen than a read one beside it.
    blobs.0.clear();
    let merged =
        info_span!("merged cells", cells = planned.0.blobs.len()).entered();
    for (offer, blob) in planned.0.blobs.iter().enumerate() {
        if !blobs_reached[offer] {
            continue;
        }
        // What it stands for is the weighing's to say: its whole subtree
        // ordinarily, and only the systems anybody lives in where the sky
        // is read as populations. A merged mark over a cell nobody lives
        // in stands for nothing in that mode and is not drawn.
        let mark = standing.of(offer).unwrap_or_else(|| {
            crate::map::galaxy::blobs::Mark::unweighed(blob.count)
        });
        let stands_for = mark.stands_for;
        let drawn =
            share.scaled(blob.blend).wanted(stands_for as usize, blob.id) > 0
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
        // [`crate::map::galaxy::blobs::Mark::fade`] and
        // [`crate::map::galaxy::blobs::Mark::split`].
        let dim = match fill {
            true => filtering.dim.opacity(),
            // Below the dim an excluded system is not drawn at all, so
            // neither is the share of a mark that stands for one.
            false => 0.,
        };
        let fade = mark.fade(dim);
        if fade <= 0. {
            continue;
        }
        let (through, dimmed) = mark.split(dim > 0.);
        behind += stands_for;
        blobs.0.push(Blob {
            through,
            dimmed,
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
        share: share.fraction() as f32,
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
    // reach is hidden by [`crate::map::galaxy::visibility`] and the line is cut
    // back to it by [`crate::map::route::trim`]; what this settles is
    // that the stop is there to be reached at all.
    //
    // **Below the dim, a stop the filters exclude is not wanted either.**
    // The mask and a span narrow a route's stops as they narrow anything,
    // and a stop built from its name has no update time for a recency to
    // admit. [`crate::map::galaxy::spawn::spawn_systems`] refuses every one
    // of those, so offered anyway each came back the next pass, kept the
    // pass from settling and ran it whole every frame the route was up —
    // and one already standing was held here against the dim's own rule
    // that the excluded leave the map. Asked as spawn asks it, of the
    // system as it would be built or as it stands.
    for &address in &routed {
        match existing.get(address) {
            Some(entity) => {
                let admitted = || {
                    systems.get(entity).is_ok_and(|(_, system, _)| {
                        asked_for.admit(system, wall)
                    })
                };
                if fill || admitted() {
                    wanted_by.insert(entity);
                }
            }
            None => {
                if let Some(system) = System::find(address, &populated, &names)
                    && (fill || asked_for.admit(&system, wall))
                {
                    pending.push(system, true, true, now);
                    wanting = true;
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
            // dropped: over one flight ([`crate::map::galaxy::flight`]) it changed the
            // despawn count by 588 in 401,857 — the turnover is the level of
            // detail moving, not systems flickering on the threshold — and
            // the frames it took to hold the extra systems cost 23% of the
            // frame at the median.
            !wanted_by.contains(entity)
        })
        .map(|(entity, ..)| entity)
        .collect();
    drop(_zone);

    // Finished, or not: see [`Settled`].
    pass.settled = (!wanting && !deferred).then_some(here);
    pass.weighing = weighed_in;
    pass.claimed_under = narrowed.then_some(under);
}

/// Free the payloads the walk has stopped wanting, once it has stopped
/// wanting them for long enough
///
/// A payload is wanted where the walk marks its cell and the clamp still
/// reaches it. It used to be freed the moment either stopped being true,
/// which reads as the obvious rule and is the expensive one: a zoom marks a
/// different set every frame, so cells left and came back, and a payload
/// freed on one frame was read from disk again two frames later. Measured
/// over one flight ([`crate::map::galaxy::flight`]), seventy per cent of the reads were of
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
    mut held: ResMut<crate::map::index::refresh::Stamps>,
    mut keeping: ResMut<Keeping>,
    mut swept: Local<Option<Instant>>,
) {
    let now = time.last_update().unwrap_or_else(|| time.startup());
    // **Not every frame.** This is a pass over everything the map holds,
    // which at a wide zoom is a hundred thousand payloads and two or three
    // milliseconds — and what it decides is measured against [`KEEP`],
    // which is two seconds. Running it sixty times inside every one of
    // those is sixty answers to a question that can only change once.
    //
    // On a clock alone, and not on a plan change as well: a moving camera
    // changes the plan every frame, so it ran every frame of a pan. A
    // quarter of a second is an eighth of the grace: what is marked when it
    // sweeps is stamped and kept, a cell held but never stamped counts from
    // the sweep that first sees it, the ceiling holds the memory either
    // way, and what this costs is the freeing running a few frames late.
    let due =
        swept.is_none_or(|last| now.saturating_duration_since(last) >= SWEEPS);
    if !due {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use galos_route::graph::{Drive, Routing, Tuning};
    use std::collections::HashMap;

    /// A payload point becomes a system placed exactly at its own position,
    /// named by its id where the resident tables hold nothing on it.
    #[test]
    fn a_point_becomes_a_placed_system() {
        let at = [1234.5, -678.25, 90123.75];
        let point = CellSystem {
            id64: 7,
            position: at,
            updated_at: 0,
            kind: galos_index::prelude::StarKind::G,
        };

        let system = System::of(
            &point,
            None,
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
        use crate::map::filter::{Filter, Filters};
        use chrono::{Duration as Span, Utc};

        let now = Utc::now();
        let point = |id: u64, ago: i64| CellSystem {
            id64: id,
            position: [0.; 3],
            updated_at: (now - Span::seconds(ago)).timestamp() as u32,
            kind: galos_index::prelude::StarKind::G,
        };
        let built = |point: &CellSystem| {
            System::of(
                point,
                None,
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

    /// A payload point at `id`
    fn point(id: u64) -> CellSystem {
        CellSystem {
            id64: id,
            position: [0.; 3],
            updated_at: 0,
            kind: galos_index::prelude::StarKind::G,
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
        points: &[CellSystem],
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
        let points: Vec<CellSystem> = (1..=6).map(point).collect();
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
        let points: Vec<CellSystem> = (1..=6).map(point).collect();
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
        let points: Vec<CellSystem> = (1..=6).map(point).collect();
        let admits = [3u32];

        assert_eq!(
            drawn(&points, &admits, false, 4),
            vec![4],
            "the admitted alone, though the budget has room"
        );
    }

    /// Along star class every class takes its share of a cell's budget,
    /// wherever in the payload it stands
    ///
    /// Brightest first, a cell of ten G stars and twenty brown dwarfs spent
    /// ten marks on the G stars and none on the dwarfs, and a slab of them
    /// drew as a gap between the layers its budget went to.
    #[test]
    fn each_class_takes_its_share_of_a_cells_budget() {
        use galos_index::prelude::StarKind;
        // Ten G stars, then twenty brown dwarfs.
        let points: Vec<CellSystem> = (1..=30)
            .map(|id| CellSystem {
                kind: if id <= 10 { StarKind::G } else { StarKind::BrownDwarf },
                ..point(id)
            })
            .collect();
        let populated = Populated::default();
        let order = strata(&points, &Along::of(ColorBy::StarClass, &populated));

        for take in 1..=points.len() {
            let dwarfs = order[..take]
                .iter()
                .filter(|&&index| {
                    points[index as usize].kind == StarKind::BrownDwarf
                })
                .count() as f64;
            let fair = take as f64 * 20. / 30.;
            assert!(
                (dwarfs - fair).abs() <= 1.,
                "{dwarfs} dwarfs in the first {take}, against {fair}"
            );
        }
        let ids = |kind| -> Vec<u64> {
            order
                .iter()
                .map(|&index| &points[index as usize])
                .filter(|point| point.kind == kind)
                .map(|point| point.id64)
                .collect()
        };
        assert_eq!(
            ids(StarKind::G),
            (1..=10).collect::<Vec<_>>(),
            "not brightest first"
        );
        assert_eq!(ids(StarKind::BrownDwarf), (11..=30).collect::<Vec<_>>());

        // What the filters admit still goes first, in the cell's strata, and
        // the excluded fill after.
        let admits = [0u32, 1, 10, 11];
        let taken: Vec<u64> = stratified_first(&order, &admits, true)
            .take(5)
            .map(|index| points[index].id64)
            .collect();
        assert_eq!(&taken[..4], &[11, 1, 12, 2], "the admitted, by class");
        assert!(!admits.contains(&((taken[4] - 1) as u32)));
        assert_eq!(
            stratified_first(&order, &admits, false).count(),
            admits.len(),
            "the excluded were offered below the dim"
        );
    }

    /// Along a political axis every value takes its share of a cell's
    /// budget too, off the populated table, and nobody living anywhere is a
    /// value with a share of its own
    ///
    /// A view orders a cell by its own values, whichever axis it colors by:
    /// a fifth of these systems are imperial colonies and the rest empty, and
    /// any first few drawn hold them a fifth to within one.
    #[test]
    fn a_political_axis_takes_its_share_by_its_own_values() {
        use elite_journal::prelude::Allegiance;
        use galos_index::records::PopulatedSystem;

        let points: Vec<CellSystem> = (1..=40).map(point).collect();
        let colony = |address: i64| PopulatedSystem {
            address,
            name: "Colony".into(),
            position: [0.; 3],
            population: 1,
            security: None,
            government: None,
            allegiance: Some(Allegiance::Empire),
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            state: None,
            power: None,
            powerplay_state: None,
        };
        // The last eight, so the payload's own order would draw none of them
        // in its first thirty.
        let populated = Populated(std::sync::Arc::new(
            (33..=40).map(|address| (address, colony(address))).collect(),
        ));
        let order =
            strata(&points, &Along::of(ColorBy::Allegiance, &populated));
        for take in 1..=points.len() {
            let colonies = order[..take]
                .iter()
                .filter(|&&index| points[index as usize].id64 > 32)
                .count() as f64;
            let fair = take as f64 * 8. / 40.;
            assert!(
                (colonies - fair).abs() <= 1.,
                "{colonies} colonies in the first {take}, against {fair}"
            );
        }
    }

    /// The sky draws a cell brightest first, off the light beside its
    /// payload, wherever in the payload the brightest stands
    ///
    /// The payload is in standing order, which is no order of brightness;
    /// the stars that clear the eye's floor are a prefix of this order and
    /// not of the payload. A point with no light on record stands at the
    /// default class.
    #[test]
    fn the_sky_draws_a_cell_brightest_first() {
        let lit = |magnitude| galos_index::prelude::Lit {
            magnitude,
            temp_bucket: galos_index::prelude::TempBucket::new(0),
        };
        let mut resident = Resident::default();
        let id = CellId::of_point([0.; 3], 4);
        // Five points and light for four: the fifth stands at the default
        // class, between the second and the third.
        resident.insert(
            id,
            (1..=5).map(point).collect(),
            vec![lit(12.), lit(-4.), lit(18.), lit(4.8)],
        );
        let cell = resident.cell(id).unwrap();
        assert_eq!(bright(cell), vec![1, 3, 4, 0, 2]);
        assert_eq!(
            magnitude_at(cell, 4),
            galos_photometry::ClassLight::DEFAULT.absolute_magnitude.0 as f32,
        );
    }

    /// A filter dense enough to overrun the budget decimates by magnitude
    ///
    /// The admitted are ordered among themselves as the whole payload used to
    /// be, so a filter admitting everything draws exactly what no filter draws.
    #[test]
    fn a_dense_filter_decimates_by_magnitude() {
        let points: Vec<CellSystem> = (1..=6).map(point).collect();
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
        use crate::map::filter::{Filter, Filters};
        use galos_index::records::PopulatedSystem;

        let points: Vec<CellSystem> = (1..=4).map(point).collect();
        let id = CellId::of_point([0.; 3], 4);
        // The third point is the only one a faction is present in.
        let populated = Populated(std::sync::Arc::new([(
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
                state: None,
                power: None,
                powerplay_state: None,
            },
        )].into_iter().collect()));

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
        use crate::map::filter::{Filter, Filters};

        let points: Vec<CellSystem> = (1..=10).map(point).collect();
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

    /// The verdicts come in all over the view, not down the plan
    ///
    /// The walk hands its marks over a subtree at a time, so a budget spent
    /// in its order weighed one side of the sky before the other: a filter
    /// loading left to right, top to bottom.
    #[test]
    fn the_verdicts_come_in_all_over_the_view() {
        // A row of cells, in the plan's order from one end to the other.
        let marks: Vec<galos_index::read::walk::MarkRef> = (0..64)
            .map(|n| {
                let id = CellId::of_point([n as f64 * 40., 0., 0.], 12);
                galos_index::read::walk::MarkRef {
                    id,
                    slice: 100,
                    at: id.bounds().center(),
                }
            })
            .collect();
        // Whatever half a budget reaches first holds both ends of the row.
        let first = &weighing(&marks)[..32];
        let near_end = first.iter().filter(|&&offer| offer < 32).count();
        assert!(
            (12..=20).contains(&near_end),
            "{near_end} of the first 32 weighed are the row's first half"
        );
    }

    /// A pass that left a cell for a later budget runs again, whatever moved
    ///
    /// The reported trouble: with the uninhabited hidden, half the colonies
    /// on screen stood as a blur for as long as the camera stood still, and
    /// turning it a hair resolved them. A pass settled whenever it ended with
    /// any budget left, and a cell bigger than what was left had been
    /// passed over — so the pass settled on a view with cells it never
    /// weighed, and nothing brought them forward until something moved.
    #[test]
    fn a_pass_the_budget_ran_short_of_runs_again() {
        use crate::map::filter::{DimTo, Filter, Filters};

        // Two cells that do not both fit in one frame's budget, whichever
        // is weighed first, each holding one system asked for — either side
        // of the origin, so they are two patches of sky at any zoom.
        let first = CellId::of_point([-20., 0., 0.], 12);
        let second = CellId::of_point([20., 0., 0.], 12);
        let each = VERDICT_BUDGET as u64 * 3 / 4;
        let at =
            |id: u64, x: f64| CellSystem { position: [x, 0., 0.], ..point(id) };
        let one: Vec<CellSystem> = (1..=each).map(|id| at(id, -20.)).collect();
        let two: Vec<CellSystem> =
            (1..=each).map(|id| at(1_000_000 + id, 20.)).collect();

        let mut app = walking();
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks: [(first, &one), (second, &two)]
                .into_iter()
                .map(|(id, points)| galos_index::read::walk::MarkRef {
                    id,
                    slice: points.len() as u32,
                    at: id.bounds().center(),
                })
                .collect(),
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        {
            let mut resident = app.world_mut().resource_mut::<ResidentCells>();
            resident.0.insert(first, one, Vec::new());
            resident.0.insert(second, two, Vec::new());
        }
        app.world_mut().resource_mut::<Filters>().add(Filter::Systems {
            label: "one apiece".into(),
            systems: vec![each as i64, 1_000_000 + each as i64],
        });
        app.insert_resource(DimTo(0.));

        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            1,
            "both cells were weighed off one frame's budget"
        );
        // Nothing moves, and the next frame's budget reaches the other.
        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            2,
            "the pass settled with a cell it never weighed"
        );
    }

    /// Under a filter, what it admits is drawn wherever it stands apart, and
    /// one to a patch of sky where it piles up
    ///
    /// The reported trouble: with the uninhabited hidden, the colonies of an
    /// arm stood as blur, their cells' budgets a share of every system about
    /// them, while the core's piled up into a white disc. Two colonies alone
    /// in their cells are drawn both; fifty on one spot are one mark.
    #[test]
    fn the_admitted_are_drawn_where_they_stand_apart() {
        use crate::map::filter::{DimTo, Filter, Filters};

        let at = |id: u64, place: [f64; 3]| CellSystem {
            position: place,
            ..point(id)
        };
        let mut marks = Vec::new();
        let mut asked = Vec::new();
        let mut app = walking();
        for (n, place) in [[20., 0., 0.], [0., 20., 0.], [-20., 0., 0.]]
            .into_iter()
            .enumerate()
        {
            let id = CellId::of_point(place, 12);
            // A thousand the filter excludes, the brightest of the cell.
            let mut points: Vec<CellSystem> =
                (1..=1_000).map(|k| at(n as u64 * 10_000 + k, place)).collect();
            // Then the admitted: one alone in each of the first two, fifty
            // on one spot in the third.
            let admitted = if n < 2 { 1 } else { 50 };
            for k in 0..admitted {
                let address = 5_000_000 + n as u64 * 100 + k;
                points.push(at(address, place));
                asked.push(address as i64);
            }
            marks.push(galos_index::read::walk::MarkRef {
                id,
                slice: points.len() as u32,
                at: place,
            });
            app.world_mut().resource_mut::<ResidentCells>().0.insert(
                id,
                points,
                Vec::new(),
            );
        }
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks,
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Systems { label: "colonies".into(), systems: asked });
        app.insert_resource(DimTo(0.));

        app.update();
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            3,
            "not every colony that stands apart, or more than one of a pile"
        );
    }

    /// A key hiding colonies draws the sky the unfiltered map draws, the
    /// hidden left out of it only below the dim
    ///
    /// The reported trouble: hiding No state was taken for a filter, so every
    /// marked cell was read whole and every point of it, nearly the whole
    /// sky, claimed a patch of it. Frames went from 25 ms to over 450. Here
    /// each cell is a pile on one spot, which claimed would be one mark.
    #[test]
    fn a_key_hiding_colonies_draws_the_unfiltered_sky() {
        use crate::map::filter::{DimTo, Filters};
        use elite_journal::prelude::Allegiance;
        use galos_index::read::inhabited::Bucketed;
        use galos_index::records::PopulatedSystem;

        let colony = |address: i64, allegiance| {
            (
                address,
                PopulatedSystem {
                    address,
                    name: format!("Home {address}").into(),
                    position: [0.; 3],
                    population: 1_000,
                    security: None,
                    government: None,
                    allegiance: Some(allegiance),
                    primary_economy: None,
                    secondary_economy: None,
                    factions: Vec::new(),
                    body_count: None,
                    non_body_count: None,
                    state: None,
                    power: None,
                    powerplay_state: None,
                },
            )
        };
        let drawing = |hide: bool, dim: f32| {
            let mut app = walking();
            let mut marks = Vec::new();
            let mut colonies = HashMap::new();
            for (n, place) in [[20., 0., 0.], [0., 20., 0.], [-20., 0., 0.]]
                .into_iter()
                .enumerate()
            {
                let id = CellId::of_point(place, 12);
                let points: Vec<CellSystem> = (1..=100)
                    .map(|k| CellSystem {
                        position: place,
                        ..point(n as u64 * 1_000 + k)
                    })
                    .collect();
                // The brightest ten are colonies, every other one Federation.
                for (k, at) in points.iter().take(10).enumerate() {
                    let allegiance = match k % 2 {
                        0 => Allegiance::Federation,
                        _ => Allegiance::Empire,
                    };
                    let (address, row) = colony(at.id64 as i64, allegiance);
                    colonies.insert(address, row);
                }
                marks.push(galos_index::read::walk::MarkRef {
                    id,
                    slice: points.len() as u32,
                    at: place,
                });
                app.world_mut().resource_mut::<ResidentCells>().0.insert(
                    id,
                    points,
                    Vec::new(),
                );
            }
            app.insert_resource(Planned(galos_index::prelude::Needed {
                mode: galos_index::prelude::Mode::Shell,
                marks,
                blobs: Vec::new(),
                splats: Vec::new(),
            }));
            app.insert_resource(Populated(std::sync::Arc::new(
                colonies.into_iter().collect(),
            )));
            app.insert_resource(DimTo(dim));
            if hide {
                app.world_mut().resource_mut::<Filters>().edit_mask(|mask| {
                    mask.set(
                        ColorBy::Allegiance,
                        [Allegiance::bucket(Some(Allegiance::Federation))],
                        true,
                    )
                });
            }
            app.update();
            app.world().resource::<PendingSpawns>().queued()
        };

        let sky = drawing(false, 0.5);
        assert_eq!(sky, 300, "the unfiltered sky was not drawn whole");
        assert_eq!(
            drawing(true, 0.5),
            sky,
            "dimmed, the hidden colonies are drawn where the sky has them"
        );
        assert_eq!(
            drawing(true, 0.),
            sky - 15,
            "below the dim, only the hidden colonies are left out"
        );
    }

    /// A key hiding star classes, the map colored by star class, draws the
    /// class sample the unfiltered map draws, the hidden left out of it only
    /// below the dim
    ///
    /// The reported trouble: hiding the unknown stars was taken for a filter,
    /// every marked cell read whole and every point of the sky claimed a
    /// patch of it, and the frames sank and the view never finished loading.
    /// Here each cell is a pile on one spot, which claimed would be one mark.
    #[test]
    fn a_key_hiding_star_classes_draws_the_unfiltered_sample() {
        use crate::map::filter::{DimTo, Filters};

        let drawing = |hide: bool, dim: f32| {
            let mut app = walking();
            let mut marks = Vec::new();
            for (n, place) in [[20., 0., 0.], [0., 20., 0.], [-20., 0., 0.]]
                .into_iter()
                .enumerate()
            {
                let id = CellId::of_point(place, 12);
                // Every other one unscanned, the rest M stars.
                let points: Vec<CellSystem> = (1..=100)
                    .map(|k| CellSystem {
                        position: place,
                        kind: match k % 2 {
                            0 => StarKind::Unknown,
                            _ => StarKind::M,
                        },
                        ..point(n as u64 * 1_000 + k)
                    })
                    .collect();
                marks.push(galos_index::read::walk::MarkRef {
                    id,
                    slice: points.len() as u32,
                    at: place,
                });
                app.world_mut().resource_mut::<ResidentCells>().0.insert(
                    id,
                    points,
                    Vec::new(),
                );
            }
            app.insert_resource(Planned(galos_index::prelude::Needed {
                mode: galos_index::prelude::Mode::Shell,
                marks,
                blobs: Vec::new(),
                splats: Vec::new(),
            }));
            app.insert_resource(ColorBy::StarClass);
            app.insert_resource(DimTo(dim));
            app.world_mut().resource_mut::<Filters>().edit_mask(|mask| {
                mask.draw(Some(ColorBy::StarClass));
                if hide {
                    mask.set(
                        ColorBy::StarClass,
                        [usize::from(StarKind::Unknown.code())],
                        true,
                    );
                }
            });
            app.update();
            app.world().resource::<PendingSpawns>().queued()
        };

        let sky = drawing(false, 0.5);
        assert_eq!(sky, 300, "the unfiltered sample was not drawn whole");
        assert_eq!(
            drawing(true, 0.5),
            sky,
            "dimmed, the hidden class is drawn where the sample has it"
        );
        assert_eq!(
            drawing(true, 0.),
            sky / 2,
            "below the dim, only the hidden class is left out"
        );
    }

    /// Where the ceiling cuts, every cell draws the same share of what it
    /// admits, whatever its size
    ///
    /// The reported trouble: a false gap in a filtered view. Offered a round
    /// apiece, a cell holding hundreds drew no more than one holding a
    /// dozen, and stood among small ones drawn whole as a thin patch in its
    /// own glow. A hundred and ten colonies under a ceiling of eleven draw a
    /// tenth of each cell.
    #[test]
    fn the_ceiling_takes_every_cell_alike() {
        use crate::map::filter::{Filter, Filters};

        let big = CellId::of_point([0., 0., 0.], 9);
        let small = CellId::of_point([0., 0., 0.], 12);
        let row = |from: u64, count: u64, z: f64| -> Vec<CellSystem> {
            (0..count)
                .map(|k| CellSystem {
                    position: [k as f64 * 2., 0., z],
                    ..point(from + k)
                })
                .collect()
        };
        let mut resident = ResidentCells::default();
        resident.0.insert(big, row(1, 100, 0.), Vec::new());
        resident.0.insert(small, row(1_000, 10, 7.), Vec::new());
        let marks: Vec<galos_index::read::walk::MarkRef> = [big, small]
            .into_iter()
            .map(|id| galos_index::read::walk::MarkRef {
                id,
                slice: 1,
                at: [0.; 3],
            })
            .collect();
        let mut filters = Filters::default();
        filters.add(Filter::Systems {
            label: "every one".into(),
            systems: (1..=100).chain(1_000..1_010).collect(),
        });
        let prepared = filters.prepared();
        let populated = Populated::default();
        // Close enough that a patch of sky is well under the two light
        // years between any two of them.
        let mut orbit = OrbitCamera::stood_back(100.);
        orbit.looks_at(DVec3::ZERO);
        orbit.stands_at(DVec3::new(0., 0., 100.));
        let view = crate::map::galaxy::plan::view(
            &orbit,
            &crate::map::galaxy::tests::seeing(),
        )
        .expect("a view");
        let mut budget = VERDICT_BUDGET;
        let claims = claim_admitted(
            &marks,
            &weighing(&marks),
            &resident,
            &mut PointOrders::default(),
            |orders, id, points| {
                orders.walk(
                    id,
                    points,
                    &prepared,
                    &populated,
                    Utc::now(),
                    false,
                    &mut budget,
                )
            },
            |_| true,
            |_| false,
            DVec3::ZERO,
            None,
            || Crowded::about(&view, [0.; 3]),
            11,
        );
        assert_eq!(
            [claims[0].len(), claims[1].len()],
            [10, 1],
            "the ceiling was not shared in proportion"
        );
    }

    /// A colony drawn keeps its patch while the view holds still, and the
    /// view claims afresh once it moves
    ///
    /// Two reports. A filtered view filling in built colonies and dropped
    /// them again as each newly weighed cell took patches from what was on
    /// the map — 5,590 on the way to 6,431 over `.index/full`. And held
    /// across a zoom, the last view's picks stood in the next and it never
    /// looked updated. So a colony keeps its patch against a rival that
    /// arrives while nothing moves, and a moved view (here, its bubble) is
    /// claimed as a fresh load would claim it.
    #[test]
    fn a_drawn_colony_keeps_its_patch_until_the_view_moves() {
        use crate::map::filter::{DimTo, Filter, Filters};

        let spot = [5., 0., 0.];
        let cells = [
            (CellId::of_point(spot, 12), 7_000_001i64),
            (CellId::of_point(spot, 13), 7_000_002i64),
        ];
        let payload = |address: i64| {
            vec![CellSystem { position: spot, ..point(address as u64) }]
        };
        let viewing = |held: &[(CellId, i64)]| {
            let mut app = walking();
            app.insert_resource(Planned(galos_index::prelude::Needed {
                mode: galos_index::prelude::Mode::Shell,
                marks: cells
                    .iter()
                    .map(|&(id, _)| galos_index::read::walk::MarkRef {
                        id,
                        slice: 1,
                        at: spot,
                    })
                    .collect(),
                blobs: Vec::new(),
                splats: Vec::new(),
            }));
            for &(id, address) in held {
                app.world_mut().resource_mut::<ResidentCells>().0.insert(
                    id,
                    payload(address),
                    Vec::new(),
                );
            }
            app.world_mut().resource_mut::<Filters>().add(Filter::Systems {
                label: "rivals".into(),
                systems: cells.iter().map(|&(_, address)| address).collect(),
            });
            app.insert_resource(DimTo(0.));
            app
        };
        let queued =
            |app: &App| app.world().resource::<PendingSpawns>().queued();
        for (first, then) in [(0, 1), (1, 0)] {
            let (_, drawn) = cells[first];
            // The first cell alone, and its colony built.
            let mut app = viewing(&cells[first..=first]);
            app.update();
            app.world_mut().spawn(crate::map::galaxy::tests::system(drawn));
            app.insert_resource(PendingSpawns::default());
            app.update();
            // Its rival arrives, and nothing else moves.
            let (id, address) = cells[then];
            app.world_mut().resource_mut::<ResidentCells>().0.insert(
                id,
                payload(address),
                Vec::new(),
            );
            app.update();
            assert_eq!(queued(&app), 0, "{address} was built over {drawn}");
            assert!(!dropping(&mut app).contains(&drawn), "{drawn} dropped");

            // The view moves, and is claimed as it would be loaded whole.
            app.world_mut().resource_mut::<Spyglass>().radius = 40.;
            app.insert_resource(PendingSpawns::default());
            app.update();
            let whole_drew_it = {
                let mut again = viewing(&cells);
                again
                    .world_mut()
                    .spawn(crate::map::galaxy::tests::system(drawn));
                again.update();
                queued(&again) == 0
            };
            assert_eq!(
                queued(&app) == 0,
                whole_drew_it,
                "a moved view kept the picks of the one before it"
            );
        }
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
            (crate::map::galaxy::populated::gather, reconcile).chain(),
        );
        app.init_resource::<PendingEvictions>();
        app.init_resource::<PendingSpawns>();
        app.init_resource::<crate::map::galaxy::Addresses>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<Entered>();
        app.init_resource::<crate::map::selection::Selection>();
        app.init_resource::<crate::map::filter::Filters>();
        app.init_resource::<crate::map::filter::DimTo>();
        app.init_resource::<crate::map::filter::Cut>();
        app.init_resource::<PointOrders>();
        app.init_resource::<Republished>();
        app.init_resource::<Keeping>();
        app.init_resource::<Sampled>();
        app.init_resource::<Blobs>();
        app.init_resource::<crate::map::galaxy::blobs::Standing>();
        app.init_resource::<crate::map::galaxy::populated::PopulatedOrder>();
        app.init_resource::<crate::map::galaxy::plan::Drawn>();
        app.insert_resource(crate::map::index::ResidentIndex(
            galos_index::prelude::Index::default(),
        ));
        app.insert_resource(Populated::default());
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.insert_resource(View::Map);
        app.insert_resource(ColorBy::Allegiance);
        app.insert_resource(ScalePopulation(false));
        app.insert_resource(Spyglass {
            radius: 50.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks: Vec::new(),
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        app.world_mut().spawn((
            OrbitCamera::default(),
            crate::map::galaxy::tests::seeing(),
        ));
        app
    }

    /// Hold every payload a build published, and mark every cell it holds
    ///
    /// Both halves, because the walk draws the cells the plan marks and holds
    /// the payloads of those it has read: a test that filled one and not the
    /// other would be a map holding a galaxy nothing marks. See
    /// [`evict_payloads`].
    fn holding(app: &mut App, built: &galos_index::prelude::Snapshot) {
        let mut marks = Vec::new();
        {
            let mut resident = app.world_mut().resource_mut::<ResidentCells>();
            for cell in built.index.cells() {
                let points = built.payload(cell.id);
                if !points.is_empty() {
                    resident.0.insert(
                        cell.id,
                        points.to_vec(),
                        built.lit(cell.id).to_vec(),
                    );
                    marks.push(galos_index::read::walk::MarkRef {
                        id: cell.id,
                        slice: points.len() as u32,
                        at: cell.id.bounds().center(),
                    });
                }
            }
        }
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
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

    /// A filtered frame reads every marked cell's prefix before any cell the
    /// rest of the way, and still ends with every cell held whole
    ///
    /// Read whole from the first, a coarse cell's thousands of points went
    /// ahead of a fine cell's few dozen, and the view filled in a cell at a
    /// time.
    #[test]
    fn a_filtered_frame_reads_every_prefix_before_any_cell_whole() {
        let coarse = CellId::of_point([0.0, 0.0, 0.0], 3);
        let fine = CellId::of_point([900.0, 0.0, 900.0], 5);
        let mark = |id: CellId, slice: u32| galos_index::read::walk::MarkRef {
            id,
            slice,
            at: id.bounds().center(),
        };
        let marks = [mark(coarse, 5_000), mark(fine, 4_000)];
        // Ten drawn apiece, so a prefix of forty.
        let wanted = |_: usize, _: CellId| 10;

        let coarse_at_prefix = |id: CellId| if id == coarse { 40 } else { 0 };
        assert_eq!(
            reads(
                &marks,
                wanted,
                coarse_at_prefix,
                Whole::Every,
                |_| true,
                false
            ),
            vec![(fine, 40), (coarse, 5_000)],
            "a cell was read whole while another's prefix waited"
        );
        assert_eq!(
            reads(
                &marks,
                wanted,
                coarse_at_prefix,
                Whole::None,
                |_| true,
                false
            ),
            vec![(fine, 40)],
            "unfiltered, a prefix is all a cell is read to"
        );

        // The way there changed and the loaded set did not: once every
        // prefix is in, every marked cell is read whole, and then nothing.
        assert_eq!(
            reads(&marks, wanted, |_| 40, Whole::Every, |_| true, false),
            vec![(coarse, 5_000), (fine, 4_000)]
        );
        let whole = |id: CellId| if id == coarse { 5_000 } else { 4_000 };
        assert!(
            reads(&marks, wanted, whole, Whole::Every, |_| true, false)
                .is_empty()
        );
    }

    /// Filtered, the cells that draw are read the whole way before a cell
    /// that draws nothing is read at all, and that one is read in one go
    ///
    /// Ranked by stage first, every idle cell's prefix went ahead of the rest
    /// of the drawing cells, and under a filter admitting a handful of
    /// systems a payload the map drew nothing it asked for until they landed.
    #[test]
    fn a_filtered_frame_reads_what_draws_whole_before_what_does_not() {
        // The idle cell coarser, so coarse-before-fine alone would read it
        // first.
        let draws = CellId::of_point([900.0, 0.0, 900.0], 6);
        let idle = CellId::of_point([0.0, 0.0, 0.0], 3);
        let mark = |id: CellId, slice: u32| galos_index::read::walk::MarkRef {
            id,
            slice,
            at: id.bounds().center(),
        };
        let marks = [mark(idle, 800), mark(draws, 5_000)];
        let wanted = |_: usize, id: CellId| if id == draws { 10 } else { 0 };

        assert_eq!(
            reads(&marks, wanted, |_| 0, Whole::Every, |_| true, false),
            vec![(draws, 40), (idle, 800)],
            "a cell that draws nothing was read to a prefix, or read first"
        );
        let draws_at_prefix = |id: CellId| if id == draws { 40 } else { 0 };
        assert_eq!(
            reads(
                &marks,
                wanted,
                draws_at_prefix,
                Whole::Every,
                |_| true,
                false
            ),
            vec![(draws, 5_000), (idle, 800)],
            "the rest of a drawing cell waited on a cell that draws nothing"
        );
        // Unfiltered, the idle cell is read to the least a cell is read to.
        assert_eq!(
            reads(&marks, wanted, |_| 0, Whole::None, |_| true, false),
            vec![(draws, 40), (idle, READ_LEAST)],
        );
        // Along star class, a drawing cell is read whole the same way where
        // its share is big enough to sample, and only there; the idle one is
        // read to its prefix, there being no class sample to draw.
        let sampling = |_: usize, id: CellId| if id == draws { 200 } else { 0 };
        let at_its_prefix = |id: CellId| if id == draws { 800 } else { 0 };
        assert_eq!(
            reads(
                &marks,
                sampling,
                at_its_prefix,
                Whole::Drawing,
                |_| true,
                false
            ),
            vec![(draws, 5_000), (idle, READ_LEAST)],
        );
        assert_eq!(
            reads(
                &marks,
                wanted,
                draws_at_prefix,
                Whole::Drawing,
                |_| true,
                false
            ),
            vec![(idle, READ_LEAST)],
            "ten marks of five thousand were read whole"
        );
    }

    /// Under a filter admitting only rows of the populated table, a cell
    /// holding none is read as an unfiltered frame reads it, and one holding
    /// any is read whole
    ///
    /// Read whole regardless, the uninhabited hidden at nine thousand light
    /// years back held 16.2 million points over `.index/full` where 3.4
    /// million held every colony, and took eighty frames to weigh them. Read
    /// to a prefix where a row stands, a colony past it is never drawn.
    #[test]
    fn a_filter_of_rows_reads_whole_only_what_holds_a_row() {
        let at = |x: f64, level: u8| CellId::of_point([x, 0.0, 0.0], level);
        let (draws, idle) = (at(1_000.0, 6), at(-1_000.0, 6));
        let (draws_rowless, idle_rowless) = (at(9_000.0, 6), at(-9_000.0, 6));
        let mark = |id: CellId| galos_index::read::walk::MarkRef {
            id,
            slice: 5_000,
            at: id.bounds().center(),
        };
        let marks = [draws, idle, draws_rowless, idle_rowless].map(mark);
        let wanted =
            |_: usize, id: CellId| match id == draws || id == draws_rowless {
                true => 200,
                false => 0,
            };
        let rows = |id: CellId| id == draws || id == idle;
        // Every prefix in hand, so what is asked is what is read whole.
        let prefix = |id: CellId| match id == draws || id == draws_rowless {
            true => 800,
            false => READ_LEAST,
        };
        let asked = |by_class| {
            let mut asked = reads(
                &marks,
                wanted,
                prefix,
                Whole::Rows { by_class },
                rows,
                false,
            );
            asked.sort();
            asked
        };
        let mut holding = vec![(draws, 5_000), (idle, 5_000)];
        holding.sort();
        assert_eq!(asked(false), holding);
        // Along star class a drawing cell is sampled whole besides, row or
        // none, as unfiltered.
        holding.push((draws_rowless, 5_000));
        holding.sort();
        assert_eq!(asked(true), holding);
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
        use crate::map::galaxy::tests::system;
        use crate::map::selection::{Picked, Selection};

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
        use crate::map::galaxy::tests::system;
        use crate::map::route::Hop;

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
        use crate::map::galaxy::tests::system;

        let mut app = walking();
        let inside = app.world_mut().spawn(system(1)).id();
        app.world_mut().spawn(system(2));
        app.insert_resource(Entered::holding(inside));

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
    /// walk never built them, and [`crate::map::route::trim`] cuts the line
    /// at a stop the map does not hold exactly as it cuts one the spyglass
    /// has put away. [`Hop`] was all that was spared, and that marks two
    /// stops — the one behind and the one ahead — so the rest of the line
    /// went.
    ///
    /// Two halves to it: a stop already on the map is not dropped, and one
    /// the walk never built is asked for.
    #[test]
    fn the_walk_holds_every_stop_of_a_route() {
        use crate::map::filter::{Filter, Filters};
        use crate::map::galaxy::tests::system;
        use galos_index::records::NameEntry;

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
        use crate::map::filter::{Filter, Filters};
        use crate::map::galaxy::tests::system;
        use galos_index::prelude::{BuildParams, Snapshot};
        use galos_index::records::PopulatedSystem;

        // Five systems a few light years apart, faintest last, and the faction
        // is in that faintest one.
        let held = 5i64;
        let inputs: Vec<galos_index::prelude::System> = (1..=5)
            .map(|id| galos_index::prelude::System {
                id64: id as u64,
                position: placed(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::prelude::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
        holding(&mut app, &built);
        app.insert_resource(Populated(std::sync::Arc::new([(
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
                state: None,
                power: None,
                powerplay_state: None,
            },
        )].into_iter().collect())));
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
        app.insert_resource(crate::map::filter::DimTo(0.));
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
    /// drawn at all ([`crate::map::galaxy::visibility`]) — so a prefix taken
    /// brightest-first spent a cell's whole budget building systems that were
    /// never painted, and the populated ones behind them never arrived. The
    /// walk still chooses the cells: what this settles is which of a held
    /// cell's systems come out of it.
    #[test]
    fn the_walk_spends_a_cells_budget_on_the_populated_systems() {
        use galos_index::prelude::{BuildParams, Snapshot};
        use galos_index::records::PopulatedSystem;

        // In front of the camera, where [`placed`] puts the rest of the
        // fixtures level with it. The populated draw takes one mark to a
        // mark's worth of *screen* ([`Crowded`]), so a system the camera
        // cannot see claims nothing and is not drawn — which is right,
        // and makes a fixture sitting on the eye's own plane a fixture
        // nothing is drawn from.
        let in_view = |id: i64| {
            let at = placed(id);
            [at[0], at[1], -25.]
        };

        let inputs: Vec<galos_index::prelude::System> = (1..=5)
            .map(|id| galos_index::prelude::System {
                id64: id as u64,
                position: in_view(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::prelude::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
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
                    // — the same place `System::find` builds one at — so a
                    // fixture that disagrees with its index is testing
                    // two galaxies.
                    position: in_view(address).map(|it| it as f32),
                    population,
                    security: None,
                    government: None,
                    allegiance: None,
                    primary_economy: None,
                    secondary_economy: None,
                    factions: Vec::new(),
                    body_count: None,
                    non_body_count: None,
                    state: None,
                    power: None,
                    powerplay_state: None,
                },
            )
        };
        app.insert_resource(Populated(std::sync::Arc::new([
            lived_in(2, 10),
            lived_in(4, 1_000_000),
        ].into_iter().collect())));

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
    /// ([`crate::map::filter::DimTo`] at zero) `spawn_systems` refuses
    /// every one of them, so none became an entity, none was found already
    /// drawn, and the whole cell was queued again every frame off a budget the
    /// admitted in other cells needed.
    #[test]
    fn a_cell_the_filters_empty_offers_nothing_below_the_dim() {
        use crate::map::filter::{DimTo, Filter, Filters};
        use galos_index::prelude::{BuildParams, Snapshot};
        use galos_index::records::PopulatedSystem;

        let inputs: Vec<galos_index::prelude::System> = (1..=4)
            .map(|id| galos_index::prelude::System {
                id64: id as u64,
                position: placed(id as i64),
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::prelude::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = walking();
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
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
                    state: None,
                    power: None,
                    powerplay_state: None,
                },
            )
        };
        app.insert_resource(Populated(std::sync::Arc::new([
            lived_in(1),
            lived_in(2),
            lived_in(3),
            lived_in(4),
        ].into_iter().collect())));
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

    /// A populated table of `(address, at, population, factions)` rows
    fn peopled_table(rows: &[(i64, [f64; 3], u64, Vec<i32>)]) -> Populated {
        use galos_index::records::PopulatedSystem;

        Populated(std::sync::Arc::new(
            rows.iter()
                .map(|(address, at, population, factions)| {
                    (
                        *address,
                        PopulatedSystem {
                            address: *address,
                            name: format!("Home {address}").into(),
                            position: at.map(|v| v as f32),
                            population: *population,
                            security: None,
                            government: None,
                            allegiance: None,
                            primary_economy: None,
                            secondary_economy: None,
                            factions: factions.clone(),
                            body_count: None,
                            non_body_count: None,
                            state: None,
                            power: None,
                            powerplay_state: None,
                        },
                    )
                })
                .collect(),
        ))
    }

    /// A world read as populations over the rows of [`peopled_table`],
    /// indexed where they stand and every cell of the index held and marked,
    /// with one entity already standing for each row
    fn peopled(rows: &[(i64, [f64; 3], u64, Vec<i32>)]) -> App {
        use galos_index::prelude::{BuildParams, Snapshot};

        let inputs: Vec<galos_index::prelude::System> = rows
            .iter()
            .map(|(address, at, ..)| galos_index::prelude::System {
                id64: *address as u64,
                position: *at,
                absolute_magnitude: *address as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::prelude::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());
        let mut app = walking();
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
        holding(&mut app, &built);
        app.insert_resource(peopled_table(rows));
        app.insert_resource(ScalePopulation(true));
        for (address, ..) in rows {
            app.world_mut().spawn(crate::map::galaxy::tests::system(*address));
        }
        app
    }

    /// The populated choice is made again when the dim crosses zero or the
    /// table changes, the camera standing still
    ///
    /// The reported trouble: the choice was keyed on the filters and the
    /// view alone. Taken to zero, the excluded colonies chosen at the dim
    /// stayed chosen, so the ones standing were held against the dim's own
    /// rule and never left; brought back up, the excluded never came back
    /// until the camera moved. A new table, with a colony the filter now
    /// admits, was not read either.
    #[test]
    fn the_populated_choice_follows_the_dim_and_the_table() {
        use crate::map::filter::{DimTo, Filter, Filters};

        let rows = |admitting: i64| -> Vec<(i64, [f64; 3], u64, Vec<i32>)> {
            (1..=4)
                .map(|address| {
                    let factions = match address == admitting {
                        true => vec![9_999],
                        false => Vec::new(),
                    };
                    (address, placed(address), 1_000 * address as u64, factions)
                })
                .collect()
        };
        let mut app = peopled(&rows(0));
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Faction { id: 9_999, name: "Nobody".into() });
        app.insert_resource(DimTo(0.5));
        app.update();
        let dimmed = dropping(&mut app);
        assert!(dimmed.len() < 4, "nothing excluded was drawn at the dim");

        app.insert_resource(DimTo(0.));
        app.update();
        assert_eq!(
            dropping(&mut app),
            vec![1, 2, 3, 4],
            "below the dim, the excluded chosen above it were kept"
        );

        app.insert_resource(DimTo(0.5));
        app.update();
        assert_eq!(
            dropping(&mut app),
            dimmed,
            "back above the dim, the excluded were not drawn again"
        );

        app.insert_resource(DimTo(0.));
        app.update();
        app.insert_resource(peopled_table(&rows(4)));
        app.update();
        assert_eq!(
            dropping(&mut app),
            vec![1, 2, 3],
            "the colony a new table admits was not chosen"
        );
    }

    /// What the filters admit claims the lattice before what they exclude
    ///
    /// Two colonies on one spot, which is one mark's worth of sky. The busier
    /// is excluded and drawn dimmed; the quieter is the faction's. Taken
    /// busiest first, the excluded claimed the spot and the faction's colony
    /// was dropped from its own filter's map.
    #[test]
    fn the_admitted_claim_the_population_lattice_first() {
        use crate::map::filter::{DimTo, Filter, Filters};

        let spot = placed(1);
        let mut app =
            peopled(&[(1, spot, 2_000, Vec::new()), (2, spot, 1_000, vec![7])]);
        app.world_mut()
            .resource_mut::<Filters>()
            .add(Filter::Faction { id: 7, name: "Ours".into() });
        app.insert_resource(DimTo(0.5));
        app.update();
        assert_eq!(
            dropping(&mut app),
            vec![1],
            "the excluded colony took the admitted one's patch"
        );
    }

    /// A route's stop the filters exclude is neither offered nor held below
    /// the dim
    ///
    /// The reported trouble: every stop was offered whatever the filters
    /// said. Below the dim `spawn_systems` refuses an excluded one, so it was
    /// offered again every frame and the pass never settled; and one already
    /// standing was held, where the dim says the excluded leave the map.
    #[test]
    fn a_route_stop_the_filters_exclude_leaves_the_map_below_the_dim() {
        use crate::map::filter::{DimTo, Filter, Filters};
        use elite_journal::prelude::Allegiance;
        use galos_index::read::inhabited::Bucketed;
        use galos_index::records::NameEntry;

        let names = Names::reaching(
            (1..=3)
                .map(|address| NameEntry {
                    address,
                    name: format!("Stop {address}").into(),
                    position: [address as f32 * 10., 0., 0.],
                })
                .collect(),
            Vec::new(),
        );
        // The first stop is nobody's; the other two are the Federation's,
        // which the key hides.
        let mut populated = peopled_table(&[
            (1, [10., 0., 0.], 1, Vec::new()),
            (2, [20., 0., 0.], 1, Vec::new()),
            (3, [30., 0., 0.], 1, Vec::new()),
        ]);
        {
            let rows = std::sync::Arc::get_mut(&mut populated.0)
                .expect("the table is not shared yet");
            for address in [2, 3] {
                rows.get_mut(&address).expect("a row").allegiance =
                    Some(Allegiance::Federation);
            }
        }
        // The second stop already standing, the other two never built.
        let standing = System::find(2, &populated, &names).expect("a stop");

        let mut app = walking();
        app.insert_resource(names);
        app.insert_resource(populated);
        app.world_mut().spawn(standing);
        let mut filters = app.world_mut().resource_mut::<Filters>();
        filters.add(Filter::Route {
            label: "Stop 1 to Stop 3".into(),
            systems: vec![1, 2, 3],
            range: "10".into(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        });
        filters.edit_mask(|mask| {
            mask.set(
                ColorBy::Allegiance,
                [Allegiance::bucket(Some(Allegiance::Federation))],
                true,
            )
        });
        app.insert_resource(DimTo(0.));

        app.update();

        assert_eq!(
            dropping(&mut app),
            vec![2],
            "an excluded stop was held below the dim"
        );
        assert_eq!(
            app.world().resource::<PendingSpawns>().queued(),
            1,
            "an excluded stop was offered below the dim"
        );
    }

    /// A merged mark the filters exclude is dimmed, and dropped below the dim
    ///
    /// **A merged mark answers the filters as the marks it replaces do.**
    /// Before this it answered nothing: at galaxy scale nearly every mark
    /// on the map is a merged one, so a faction filter dimmed the handful
    /// of drawn stars and left the galaxy standing at full strength. What
    /// it can be asked is whether anything under it is admitted — see
    /// [`crate::map::galaxy::blobs::Standing`] — and the verdict is spent here
    /// exactly as a system's is: dimmed while the excluded are drawn,
    /// dropped when they are not.
    #[test]
    fn a_merged_mark_the_filters_exclude_is_dimmed_then_dropped() {
        use crate::map::filter::{DimTo, Filter, Filters};

        let merged = |id: CellId| galos_index::read::walk::BlobRef {
            id,
            count: 4_000,
            blend: 1.,
            at: id.bounds().center(),
            aged: [500; galos_index::core::aggregate::AGE_BUCKETS],
            m_min: Some(2.),
        };
        let held = CellId::of_point([0., 0., 0.], 6);

        let standing = |app: &mut App, share: f32| {
            app.insert_resource(crate::map::galaxy::blobs::Standing::weighed(
                vec![(Vec3::splat(0.25), share, 4_000)],
            ));
        };

        let mut app = walking();
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
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
    /// what [`crate::map::index::refresh`] exists to keep current.
    #[test]
    fn a_republished_cell_rebuilds_the_systems_already_drawn() {
        use galos_index::prelude::{BuildParams, Snapshot};

        let at = |id: u64, when: u32| galos_index::prelude::System {
            id64: id,
            position: placed(id as i64),
            absolute_magnitude: id as f64,
            temperature: 5000.,
            age_bucket: 0,
            updated_at: when,
            kind: galos_index::prelude::StarKind::G,
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
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
        app.world_mut().resource_mut::<ResidentCells>().0.insert(
            owner,
            built.payload(owner).to_vec(),
            built.lit(owner).to_vec(),
        );
        // Marked as well as held: the walk draws the cells the plan names.
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks: vec![galos_index::read::walk::MarkRef {
                id: owner,
                slice: built.payload(owner).len() as u32,
                at: owner.bounds().center(),
            }],
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        // Drawn already, as it would be a frame after the first read.
        app.world_mut().spawn(crate::map::galaxy::tests::system(1));

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
                        Some(&mut *republished),
                        owner,
                        payload.clone(),
                        Vec::new(),
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

    /// A cell read further under the stamp it is held at leaves what is drawn
    /// out of it standing; under any other stamp, or none, it is rebuilt
    ///
    /// A filtered frame reads every drawing cell twice, its prefix and then
    /// the rest, and a share that outgrows a prefix reads it again. Taken as
    /// a republish, each of those rebuilt every system already drawn out of
    /// the cell, spending the spawn budget on systems nothing had changed.
    #[test]
    fn a_cell_read_further_rebuilds_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        use galos_index::prelude::{BuildParams, Snapshot};

        let built = Snapshot::build(
            &[galos_index::prelude::System {
                id64: 1,
                position: placed(1),
                absolute_magnitude: 1.,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 1_700_000_000,
                kind: galos_index::prelude::StarKind::G,
            }],
            &BuildParams::default(),
        );
        let owner = built
            .index
            .cells()
            .map(|cell| cell.id)
            .find(|&id| !built.payload(id).is_empty())
            .expect("some cell owns the system");
        let payload = built.payload(owner).to_vec();

        let mut app = walking();
        app.init_resource::<BoundedTasks>();
        app.init_resource::<crate::map::index::refresh::Stamps>();
        app.insert_resource(Transport(std::sync::Arc::new(
            galos_index::prelude::FsSource::new("unread"),
        )));
        app.insert_resource(crate::map::index::ResidentIndex(
            built.index.clone(),
        ));
        app.insert_resource(Planned(galos_index::prelude::Needed {
            mode: galos_index::prelude::Mode::Shell,
            marks: vec![galos_index::read::walk::MarkRef {
                id: owner,
                slice: payload.len() as u32,
                at: owner.bounds().center(),
            }],
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        // A read lands as the workers land one, and is taken in.
        let land = |app: &mut App, stamp: Option<u64>| {
            app.world()
                .resource::<BoundedTasks>()
                .shared
                .lock()
                .expect("the reads lock")
                .landed
                .push((owner, Ok((payload.clone(), Vec::new(), stamp))));
            app.world_mut().run_system_once(collect).expect("collect runs");
            app.world_mut().insert_resource(PendingSpawns::default());
            app.update();
            app.world().resource::<PendingSpawns>().queued()
        };

        // First read, and the system drawn out of it.
        land(&mut app, Some(7));
        app.world_mut().spawn(crate::map::galaxy::tests::system(1));
        app.insert_resource(PendingSpawns::default());
        app.update();
        assert_eq!(app.world().resource::<PendingSpawns>().queued(), 0);

        assert_eq!(
            land(&mut app, Some(7)),
            0,
            "the same payload read further rebuilt what was drawn out of it"
        );
        assert_eq!(
            land(&mut app, Some(8)),
            1,
            "a read under a new stamp left the drawn system as first read"
        );
        assert_eq!(
            land(&mut app, None),
            1,
            "a read nothing could stamp was taken for the payload held"
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
        use crate::map::galaxy::tests::system;
        use crate::map::selection::{Picked, Selection};

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

    /// A pass that found nothing to do is not done again until something it
    /// reads moves, and is done again the frame something does
    ///
    /// Stood still, the walk used to re-decide every mark it had decided the
    /// frame before. What it may not do in its place is miss a move: the
    /// eviction it would queue is the sign it ran. Cleared by hand between
    /// frames, it comes back only from a pass that ran.
    #[test]
    fn a_settled_pass_waits_for_something_to_move() {
        use crate::map::galaxy::tests::system;

        let mut app = walking();
        app.world_mut().spawn(system(1));
        app.update();
        assert_eq!(dropping(&mut app), vec![1], "the first pass never ran");

        let cleared = |app: &mut App| {
            app.world_mut().resource_mut::<PendingEvictions>().0.clear();
        };
        cleared(&mut app);
        app.update();
        assert!(dropping(&mut app).is_empty(), "a still frame was walked");

        // A setting it reads.
        app.world_mut().resource_mut::<crate::map::filter::DimTo>().0 = 0.5;
        app.update();
        assert_eq!(dropping(&mut app), vec![1], "a changed dim was missed");

        // And the camera, which is written every frame whether it moves or
        // not and so is compared rather than asked.
        cleared(&mut app);
        app.update();
        assert!(dropping(&mut app).is_empty(), "a still frame was walked");
        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        for mut camera in cameras.iter_mut(app.world_mut()) {
            camera.stands_at(DVec3::new(10., 0., 0.));
        }
        app.update();
        assert_eq!(dropping(&mut app), vec![1], "a moved camera was missed");
    }
}
