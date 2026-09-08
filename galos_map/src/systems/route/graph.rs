//! The jump graph the router walks.
//!
//! Routing used to be a database question: A* over `ST_3DDWithin` neighbour
//! queries. With the map drawing from the index there is no database, so the
//! same walk runs here over the resident names table, which carries every
//! system's place. The one thing a walk over a million points needs that a
//! database index gave it for free is a way to ask for neighbours without
//! scanning them all, so the positions are bucketed into a coarse spatial grid
//! and a jump looks only in the buckets a ship could reach.

use crate::Boosts;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::meta::{Boost, NameEntry};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// The edge of a grid bucket, in light years.
///
/// A jump reaches a few tens of light years, so a bucket this size means a
/// neighbour search looks in a handful of buckets rather than the whole grid.
/// Larger wastes the pruning; smaller multiplies the buckets a jump must visit.
const BUCKET_LY: f64 = 64.0;

/// How hard the map works at a route
///
/// Two of the three are the fewest jumps — that is what a jump range means,
/// fuel and time — and search with an estimate that never overstates the jumps
/// left. Twenty-two thousand light years is hundreds of jumps, though, and the
/// number of chains exactly that long is enormous; the last of the three
/// declines to prove it is not on a shorter one.
///
/// Measured Sol to Colonia at a 50 light year range, over the 2.4 million
/// systems the names table carries:
///
/// | | jumps | found in |
/// |---|---|---|
/// | [`Quick`](Self::Quick) | 458, and bounded at 480 | 0.08 s |
/// | [`Direct`](Self::Direct) | 458 | 1.8 s |
/// | [`Shortest`](Self::Shortest) | 458, the shortest of them | 3.2 s |
///
/// The same four hundred and fifty-eight jumps either way here, for a
/// twentieth of the wait — but only the middle row *proves* it is the fewest,
/// and that proof is nearly all of the time: it means expanding every system
/// that could have been on an equally short chain. Hence the default in the
/// middle, and hence the choice either side of it.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum Routing {
    /// Lean on the estimate: at most one jump in twenty over the fewest
    ///
    /// The estimate is multiplied by [`LEANING`], which makes it inadmissible
    /// on purpose — it may now overstate what is left, and A* stops expanding
    /// the enormous plateau of systems that could have been on an equally
    /// short chain. What comes back is bounded rather than proven: weighted A*
    /// returns a route costing no more than the weight times the fewest, and
    /// with a jump costing twenty that comes to
    ///
    /// ```text
    /// jumps <= fewest + fewest / 20, rounded down
    /// ```
    ///
    /// so a route of under twenty jumps is the fewest there are and cannot be
    /// anything else, and a crossing of the galaxy may be a jump over for
    /// every twenty it takes. Which is the whole of the claim: it bounds
    /// *jumps* and says nothing about light years. The tie-break is
    /// [`Routing::Direct`]'s, the same walk under both — it prefers the chain
    /// that heads at the goal — but a chain that is a jump longer than the
    /// fewest is longer whichever of its neighbours were preferred, and
    /// nothing here proves otherwise.
    ///
    /// The bound needs the search to reopen a system a cheaper way turns up
    /// to, which is what [`JumpGraph::search`] does and why it holds no closed
    /// set: a weighted estimate is inconsistent, and held shut the bound is a
    /// hope rather than a theorem.
    ///
    /// It buys nothing with supercharging on. What is slow there is the reach
    /// cube of a boosted jump — nine cells to a side rather than three — and
    /// the plateau this cuts is not where the time was going.
    Quick,
    /// Take the neighbour that gets nearest the goal, and do not look back.
    #[default]
    Direct,
    /// Prove the shortest of the chains that are the fewest jumps.
    Shortest,
}

impl Routing {
    /// What this is called where a route says what it was plotted with
    ///
    /// One word, lowercase, to be read in a line of prose beside the range
    /// and the drive rather than as a heading. The form's own labels are
    /// capitalised; this is the same choice said in a sentence.
    pub(crate) fn named(&self) -> &'static str {
        match self {
            Routing::Quick => "quick",
            Routing::Direct => "direct",
            Routing::Shortest => "shortest",
        }
    }
}

/// How far [`Routing::Quick`] leans on the estimate, as a fraction
///
/// Weighted A*: an estimate multiplied by `w` returns a route no worse than
/// `w` times the fewest jumps. A twentieth is the knee of the curve measured
/// on the galaxy — a fiftieth still expands most of the plateau, and a tenth
/// buys another factor of three for four times the jumps over.
///
/// A fraction of two integers, and the jump cost scaled by the denominator,
/// because the guarantee is easy to spend by accident. The estimate is whole
/// jumps: rounding a *weighted distance* up to whole jumps is not the same as
/// weighting the whole jumps, and it overstates by up to a jump wherever it
/// lands — which for a system one jump out is an estimate of two, a weight of
/// two, and no twentieth about it. Scaling instead keeps the arithmetic exact
/// and the bound the theorem's.
const LEANING: (u32, u32) = (21, 20);

/// Which drive is fitted, and so what a jet cone is worth
///
/// Flying the jet of a neutron star or a white dwarf charges a frame shift
/// drive for one jump. What that multiplies the range by is a fact about the
/// drive, not about the star: a standard drive takes four times off a neutron
/// star and half again off a white dwarf, and the Mk II Supercharge Optimised
/// drive takes six and three. Where the boost can be had at all is
/// [`galos_index::Boost`], published per system.
///
/// Asked per route rather than set once, for the reason a jump range is: the
/// same two ends flown by a different ship is a different route through
/// different systems, and a map that redrew the line under an old label would
/// be lying about what it plotted. See [`crate::systems::filter::Filter`].
///
/// White dwarfs are in both settings and worth less than they look. Their
/// exclusion zone is much larger, the wiki calls them not worth the risk for
/// half again, and there are seven thousand of them against ninety-four
/// thousand neutron stars in the data — so what they change is a route with a
/// gap in its neutron chain, and little else.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum Drive {
    /// No supercharging: every jump is the range the ship reaches unaided.
    #[default]
    Unaided,
    /// A standard drive: four times off a neutron star, half again off a
    /// white dwarf.
    Standard,
    /// The Mk II Supercharge Optimised drive: six times and three.
    Optimised,
}

impl Drive {
    /// What a jump out of a system offering `boost` is multiplied by
    ///
    /// One where there is no boost to be had or no drive to take it, so a
    /// caller can scale by this unconditionally.
    pub(crate) fn factor(&self, boost: Option<Boost>) -> f64 {
        match (self, boost) {
            (Drive::Unaided, _) | (_, None) => 1.,
            (Drive::Standard, Some(Boost::WhiteDwarf)) => 1.5,
            (Drive::Standard, Some(Boost::Neutron)) => 4.,
            (Drive::Optimised, Some(Boost::WhiteDwarf)) => 3.,
            (Drive::Optimised, Some(Boost::Neutron)) => 6.,
        }
    }

    /// The most any jump can be multiplied by
    ///
    /// What the estimate of the jumps remaining has to divide by. A heuristic
    /// that never overstates what is left is what makes the search come back
    /// with a genuine fewest-jumps route, and any step of the way might be
    /// taken out of a neutron star — so the estimate has to allow the widest
    /// jump the drive could make, however few systems can offer one. It costs
    /// a weaker estimate and more of the graph searched, which is the price of
    /// the answer being true.
    pub(crate) fn widest(&self) -> f64 {
        self.factor(Some(Boost::Neutron))
    }

    /// What the row for a route says it was plotted for, where anything.
    pub(crate) fn named(&self) -> Option<&'static str> {
        match self {
            Drive::Unaided => None,
            Drive::Standard => Some("supercharged"),
            Drive::Optimised => Some("SCO supercharged"),
        }
    }
}

/// What a leg costs when the shortest route is being proved: one jump, and
/// how far that jump goes.
///
/// Two numbers rather than one because the policy is two-tiered, and the order
/// of the fields is the whole of it: the route with the fewest jumps wins
/// outright, and between two of the same length the shorter one wins. Derived
/// [`Ord`] compares the fields in order, which is exactly that. A jump can
/// never be traded away for any amount of distance, however much.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Cost {
    jumps: u32,
    /// Whole light years. An `f64` is not [`Ord`] and a route is not decided
    /// by fractions of a light year, so the distance is rounded to an integer
    /// and the comparison is total.
    ///
    /// A leg rounds up and the estimate rounds down, which is what keeps the
    /// estimate a lower bound: rounding both to nearest would let a long chain
    /// of legs each shaved by half a light year add to less than the straight
    /// line it followed, and an estimate that overstates is one that can send
    /// A* home with the wrong answer.
    ///
    /// Whole light years across a galaxy a hundred thousand of them wide, so
    /// a `u32` holds every route there is with room to spare — and a search
    /// holds one of these per system, where the eight bytes a `u64` would add
    /// are twenty megabytes that could never carry a number.
    light_years: u32,
}

/// What a step and a route cost, as a search adds them up
///
/// A search keeps one of these per system in the graph, in an array rather
/// than a map, so "nothing has reached this yet" is a value of the type rather
/// than an [`Option`] around it: at two and a half million systems the tag
/// alone is megabytes that say nothing.
trait Metric: Ord + Copy + std::ops::Add<Output = Self> {
    /// What the system the search sets out from has cost so far
    const ZERO: Self;
    /// Dearer than any route, which is what an unreached system holds
    const MAX: Self;
}

impl Metric for u32 {
    const ZERO: u32 = 0;
    const MAX: u32 = u32::MAX;
}

impl Metric for Cost {
    const ZERO: Cost = Cost { jumps: 0, light_years: 0 };
    const MAX: Cost = Cost { jumps: u32::MAX, light_years: u32::MAX };
}

/// No parent on record, which is a system no search has reached.
pub(crate) const UNSEEN: u32 = u32::MAX;

impl std::ops::Add for Cost {
    type Output = Cost;

    fn add(self, other: Cost) -> Cost {
        Cost {
            jumps: self.jumps + other.jumps,
            light_years: self.light_years + other.light_years,
        }
    }
}

/// What a search has reached, as it reaches it
///
/// Shared between the task running the search and the map drawing it: the
/// search fills it in and [`super::frontier`] reads it out a frame at a time.
/// Behind a lock rather than a channel because what the map wants is not every
/// message but the state of the thing.
///
/// Three things, which are the three ways a search is worth drawing. The set
/// of coarse cells it has expanded in, which only ever grows: the closed set,
/// a region filling. The last few of those cells, which replaces itself: the
/// leading edge, where the search is working now. And the chain it has found
/// to the closest system it has reached, which is what says whether any of it
/// is getting anywhere.
///
/// The edge is cells and not jumps for the same reason the closed set is. A
/// window of the last jumps taken is what this held first, and A* pops from
/// all over its frontier, so those jumps are scattered and unrelated to one
/// another — forty-eight strokes of debris that flicker rather than move,
/// because there is no motion between them to draw. Quantised to the same
/// cells, the edge holds still while the search works a region and steps when
/// it moves to the next, which is the thing worth seeing.
///
/// None of the three is thinned. An earlier cut of this kept one cumulative
/// sample and halved it whenever it filled, which on a search of half a
/// million expansions fires eight times: what that draws is a haze that fills
/// and abruptly thins, over and over, and it reads as the search starting
/// again rather than as the search going on. A bound the picture never
/// notices is worth more than a bound that shows.
#[derive(Default)]
pub(crate) struct Frontier(Mutex<Reached>);

/// What has been reached, under the lock.
#[derive(Default)]
struct Reached {
    /// Where the search set out from, in light years, once it is known
    ///
    /// [`None`] until the search starts, which is a search whose ends could
    /// not be resolved: there is nothing to draw and nowhere to draw it.
    from: Option<DVec3>,
    /// How wide a cell of the closed set is, in light years
    across: f64,
    /// The cells the search has expanded in, by their corner in cell counts
    cells: HashSet<[i32; 3]>,
    /// The last cells worked in, oldest first
    edge: VecDeque<[i32; 3]>,
    /// The chain to the closest system reached, from the start
    reaching: Vec<DVec3>,
    /// How far that system is from the goal, in light years
    closest: f64,
    /// How many systems have been expanded, every one of them counted
    expanded: u64,
    /// Whether the search has stopped, whatever it found
    finished: bool,
    /// How many times anything here has moved
    ///
    /// What the map rebuilds its meshes against. Counting what is held cannot
    /// answer it: the edge is a window that turns over at a fixed length and
    /// the chain moves without changing how many links it has, so two frames
    /// of entirely different pictures count the same and the drawing stands
    /// still until something else disturbs it — which is what happened, and
    /// what read as a picture that only updated when the camera moved.
    revision: u64,
}

/// What the map draws of a search, taken in one lock.
pub(crate) struct Drawn {
    /// Where the search set out from
    pub(crate) from: DVec3,
    /// How wide a cell of the closed set is, in light years
    pub(crate) across: f64,
    /// The middle of every cell expanded in
    pub(crate) cells: Vec<DVec3>,
    /// The middle of the last few cells worked in, newest last
    pub(crate) edge: Vec<DVec3>,
    /// The chain to the closest system reached
    pub(crate) reaching: Vec<DVec3>,
    /// How far that system is from the goal, in light years
    pub(crate) closest: f64,
    /// How many times anything here has moved; see [`Reached::revision`]
    pub(crate) revision: u64,
}

impl Frontier {
    /// A frontier for a search from `from` to `goal`
    ///
    /// The cell of the closed set is a sixty-fourth of the way between them,
    /// so the picture is about sixty-four cells along the route whether that
    /// is two hundred light years or twenty-two thousand. Which is what bounds
    /// the set by the geometry rather than by a count: the cells the search
    /// touches are the corridor it searched, and a corridor is not much wider
    /// than the line through it.
    pub(crate) fn between(from: DVec3, goal: DVec3) -> Arc<Frontier> {
        Arc::new(Frontier(Mutex::new(Reached {
            from: Some(from),
            across: (from.distance(goal) / super::frontier::CELLS).max(1.),
            closest: f64::INFINITY,
            ..Reached::default()
        })))
    }

    /// A sampler that feeds this, for a search to carry.
    pub(crate) fn sampler(self: &Arc<Frontier>) -> Sampler {
        let reached = self.0.lock().expect("the frontier lock");
        Sampler {
            into: Arc::clone(self),
            expanded: 0,
            across: reached.across,
            cells: HashSet::new(),
            edge: VecDeque::with_capacity(super::frontier::EDGE),
            worked: None,
            reaching: Vec::new(),
            closest: f64::INFINITY,
            settled: true,
        }
    }

    /// What there is to draw, or [`None`] where the search has reached
    /// nothing yet.
    pub(crate) fn drawn(&self) -> Option<Drawn> {
        let reached = self.0.lock().expect("the frontier lock");
        Some(Drawn {
            from: reached.from?,
            across: reached.across,
            cells: reached
                .cells
                .iter()
                .map(|cell| middle(*cell, reached.across))
                .collect(),
            edge: reached
                .edge
                .iter()
                .map(|cell| middle(*cell, reached.across))
                .collect(),
            reaching: reached.reaching.clone(),
            closest: reached.closest,
            revision: reached.revision,
        })
    }

    /// How many systems the search has expanded.
    pub(crate) fn expanded(&self) -> u64 {
        self.0.lock().expect("the frontier lock").expanded
    }

    /// Whether the search has stopped.
    pub(crate) fn finished(&self) -> bool {
        self.0.lock().expect("the frontier lock").finished
    }
}

/// Which cell of a grid `across` light years wide a place falls in.
fn cell_of(at: DVec3, across: f64) -> [i32; 3] {
    [
        (at.x / across).floor() as i32,
        (at.y / across).floor() as i32,
        (at.z / across).floor() as i32,
    ]
}

/// The middle of such a cell.
fn middle(cell: [i32; 3], across: f64) -> DVec3 {
    DVec3::new(
        (cell[0] as f64 + 0.5) * across,
        (cell[1] as f64 + 0.5) * across,
        (cell[2] as f64 + 0.5) * across,
    )
}

/// A search's own tally, flushed into a [`Frontier`] in batches
///
/// What every expansion pays: a counter, a compare and a distance to the
/// goal. What one in [`super::frontier::STRIDE`] pays on top: a cell insert,
/// and a ring push where that cell is a new one. What the closest system reached moving pays: a walk back
/// up the search's parent map, hundreds of links at the worst. The lock and
/// the copies happen once a batch.
pub(crate) struct Sampler {
    into: Arc<Frontier>,
    /// How many expansions have been seen
    expanded: u64,
    /// How wide a cell of the closed set is, in light years
    across: f64,
    /// The cells expanded in since the last flush
    cells: HashSet<[i32; 3]>,
    /// The last cells worked in, oldest first
    edge: VecDeque<[i32; 3]>,
    /// The cell the last sample fell in, so a run of them in one cell is one
    /// step of the edge rather than a dozen
    worked: Option<[i32; 3]>,
    /// The chain to the closest system reached
    reaching: Vec<DVec3>,
    /// How far that system is from the goal, in light years
    closest: f64,
    /// Whether the chain in hand is the one for the closest system reached
    settled: bool,
}

impl Sampler {
    /// Note the expansion of `node`, which sits at `at`
    ///
    /// `came` is the search's own record of where each system was reached
    /// from, indexed by system and [`UNSEEN`] where nothing has — the thing A*
    /// keeps in order to give an answer at all — so the jump drawn is the jump
    /// the search took to get here and the chain is the one it would hand back
    /// if this were the goal. `place` says where a system sits, and is asked
    /// only where something is drawn or the record moves.
    pub(crate) fn expanded(
        &mut self,
        node: usize,
        at: DVec3,
        goal: DVec3,
        came: &[u32],
        place: impl Fn(usize) -> DVec3,
    ) {
        self.expanded += 1;

        // How close the search has got, which is what the chain is drawn to.
        let away = at.distance(goal);
        if away < self.closest {
            self.closest = away;
            self.settled = false;
            self.chain(node, came, &place);
        }

        if self.expanded % super::frontier::STRIDE != 0 {
            return;
        }
        let cell = cell_of(at, self.across);
        self.cells.insert(cell);
        // Where the work is now. Only where it has moved to a cell it was not
        // in: the search grinds through hundreds of systems in one cell, and
        // an edge that re-listed the same cell every sample would be a window
        // holding one place a dozen times over.
        if self.worked != Some(cell) {
            self.worked = Some(cell);
            self.edge.push_back(cell);
            if self.edge.len() > super::frontier::EDGE {
                self.edge.pop_front();
            }
        }
        if self.expanded
            % (super::frontier::STRIDE * super::frontier::BATCH as u64)
            == 0
        {
            self.flush();
        }
    }

    /// Walk back from `node` to the system the search set out from
    ///
    /// The chain of jumps A* would hand back if this were the goal: every link
    /// a jump it found, in the order they are flown. Bounded by the jumps in
    /// it — hundreds at the worst — and taken only when the closest reached
    /// has moved.
    fn chain(
        &mut self,
        node: usize,
        came: &[u32],
        place: &impl Fn(usize) -> DVec3,
    ) {
        let mut at = node;
        let mut chain = vec![place(at)];
        while came[at] != UNSEEN {
            at = came[at] as usize;
            chain.push(place(at));
            // A walk that will not end is a link cycle rather than a route,
            // and one drawn forever would be a hang.
            if chain.len() > came.len() {
                return;
            }
        }
        chain.reverse();
        self.reaching = chain;
        self.settled = true;
    }

    /// Hand what is held to the frontier
    ///
    /// The cells are merged in, the edge replaces whatever was there, and the
    /// chain is handed over where it has moved. Nothing is thinned: the cells
    /// are bounded by the corridor searched and the edge by its own length.
    ///
    /// The revision moves with them, since a flush is exactly when the picture
    /// has changed and the map has no other way to know it.
    fn flush(&mut self) {
        let mut reached = self.into.0.lock().expect("the frontier lock");
        reached.expanded = self.expanded;
        reached.revision += 1;
        reached.cells.extend(self.cells.drain());
        reached.edge.clear();
        reached.edge.extend(self.edge.iter().copied());
        if self.settled {
            reached.closest = self.closest;
            reached.reaching = std::mem::take(&mut self.reaching);
            self.reaching = reached.reaching.clone();
        }
    }

    /// Say the search has stopped, and hand over whatever is left.
    pub(crate) fn done(&mut self) {
        self.flush();
        self.into.0.lock().expect("the frontier lock").finished = true;
    }
}

/// The jump graph, held behind an [`Arc`] so a route task takes a cheap handle.
#[derive(Resource, Clone)]
pub struct Jumps(pub Arc<JumpGraph>);

/// Every system's place, bucketed in space for neighbour queries.
///
/// Two of these: the table as it was read, and the systems the feed has named
/// since. Both behind [`Arc`]s and neither ever written to, so a refresh
/// publishes a new [`JumpGraph`] by cloning two handles and building the small
/// one — where growing the base in place would copy a hundred and fifty
/// megabytes, and would do it under whatever route is being searched.
///
/// A route in flight holds the graph it started on and finishes against that.
/// It is the right answer as well as the cheap one: a search half-run against
/// a set of places that grew underneath it has been searching two different
/// skies.
pub struct JumpGraph {
    /// The table as it was read, which is most of the galaxy.
    base: Arc<Places>,
    /// The systems named since, bucketed the same way. See
    /// [`extended`](Self::extended).
    fresh: Arc<Places>,
    /// Which systems can supercharge a drive, as published.
    ///
    /// Held beside the places rather than baked into them, though a boost is a
    /// fact about a place. The table moves on nearly every publish — four
    /// systems in a hundred can supercharge and the feed names eighty a minute
    /// — and baking it in would mean rebuilding the base to take one in, which
    /// is two hundred milliseconds and a hundred and fifty megabytes. Read per
    /// expansion instead, which is one lookup for the system being left, not
    /// one for each of the thousands it can see.
    boosts: Boosts,
}

/// A set of systems' places, and the grid that finds them by neighbourhood.
#[derive(Default)]
struct Places {
    /// Each system's address and position, in light years.
    points: Vec<(i64, [f64; 3])>,
    /// Address to its index in `points`.
    by_address: HashMap<i64, usize>,
    /// Grid bucket to the indices of the points that fall in it.
    buckets: HashMap<[i32; 3], Vec<usize>>,
}

impl Places {
    /// Bucket `entries` into a searchable set.
    fn of(entries: impl IntoIterator<Item = (i64, [f64; 3])>) -> Places {
        let points: Vec<(i64, [f64; 3])> = entries.into_iter().collect();
        let by_address =
            points.iter().enumerate().map(|(i, (a, _))| (*a, i)).collect();
        let mut buckets: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
        for (i, (_, p)) in points.iter().enumerate() {
            buckets.entry(bucket_of(*p)).or_default().push(i);
        }
        Places { points, by_address, buckets }
    }
}

/// The place of a [`NameEntry`], at the table's own precision.
fn placed(entry: &NameEntry) -> (i64, [f64; 3]) {
    (
        entry.address,
        [
            entry.position[0] as f64,
            entry.position[1] as f64,
            entry.position[2] as f64,
        ],
    )
}

/// How far `p` is from the nearest corner of bucket `cell`, squared
///
/// Zero where the point is inside it. A bucket no jump can reach into is one
/// whose systems need not be measured at all; see
/// [`JumpGraph::neighbors_each`].
fn bucket_away(p: [f64; 3], cell: [i32; 3]) -> f64 {
    let mut away = 0.;
    for axis in 0..3 {
        let low = cell[axis] as f64 * BUCKET_LY;
        // Outside the bucket on this axis, by however much; zero within it.
        let out = (low - p[axis]).max(p[axis] - (low + BUCKET_LY)).max(0.);
        away += out * out;
    }
    away
}

/// Which bucket a point falls in.
fn bucket_of(p: [f64; 3]) -> [i32; 3] {
    [
        (p[0] / BUCKET_LY).floor() as i32,
        (p[1] / BUCKET_LY).floor() as i32,
        (p[2] / BUCKET_LY).floor() as i32,
    ]
}

/// The squared distance between two points, the distance itself wanted for
/// nothing here but comparing.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

impl JumpGraph {
    /// Build the graph from the resident names table and the supercharge
    /// table beside it.
    pub fn new(entries: &[NameEntry], boosts: &Boosts) -> JumpGraph {
        JumpGraph {
            base: Arc::new(Places::of(entries.iter().map(placed))),
            fresh: Arc::default(),
            boosts: boosts.clone(),
        }
    }

    /// The same base with `arrivals` alongside it: the systems the feed has
    /// named since the table was read.
    ///
    /// Whole rather than added to, since [`crate::Names::fresh`] is itself the
    /// accumulated set and is handed here entire. Rebuilding the small side
    /// costs its own size and nothing else — the base is a handle clone — so a
    /// pass that found one arrival pays for the few thousand of a session, not
    /// for the two million of the galaxy.
    ///
    /// Only addresses the base does not hold. A rename is nothing to a router,
    /// which asks where a system is and not what it is called, and a position
    /// corrected under an address already known is not applied until the table
    /// is read afresh: the names table's own doc has it that a position is
    /// corrected about never, and taking one here would leave the same system
    /// bucketed twice, in two places, for a search to route through either.
    pub fn extended<'a>(
        &self,
        arrivals: impl IntoIterator<Item = &'a NameEntry>,
        boosts: &Boosts,
    ) -> JumpGraph {
        let known = &self.base.by_address;
        JumpGraph {
            base: Arc::clone(&self.base),
            fresh: Arc::new(Places::of(
                arrivals
                    .into_iter()
                    .filter(|entry| !known.contains_key(&entry.address))
                    .map(placed),
            )),
            boosts: boosts.clone(),
        }
    }

    /// How many systems the graph can route between.
    pub fn len(&self) -> usize {
        self.base.points.len() + self.fresh.points.len()
    }

    /// Whether the graph holds no places at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The system at an index, whichever set holds it
    ///
    /// One index space over the two: below the base's length it is the base's
    /// own, above it the overlay's. Which is what lets the searches below key
    /// on a plain `usize` as they did when there was one set.
    fn place(&self, i: usize) -> (i64, [f64; 3]) {
        match i.checked_sub(self.base.points.len()) {
            Some(i) => self.fresh.points[i],
            None => self.base.points[i],
        }
    }

    /// What the system at an index can supercharge, if anything.
    fn boost(&self, i: usize) -> Option<Boost> {
        self.boosts.get(self.place(i).0)
    }

    /// Where a system sits in that one index space, by address
    ///
    /// The overlay first, so a system named since the table was read is
    /// routable at all.
    fn index_of(&self, address: i64) -> Option<usize> {
        match self.fresh.by_address.get(&address) {
            Some(&i) => Some(self.base.points.len() + i),
            None => self.base.by_address.get(&address).copied(),
        }
    }

    /// The systems the point at `i` can jump to, by index
    ///
    /// `range` is what the ship reaches unaided, and the jump is scaled by
    /// whatever the system being left can supercharge into the `drive` fitted:
    /// a neutron star is four times as far, or six. Which is why a boost is
    /// read off the system a jump leaves rather than the one it lands in — the
    /// charge is taken in the jet cone and spent on the jump out.
    ///
    /// Both sets, cell by cell: one bucket lookup each over the same reach
    /// cube. The overlay's is a hash lookup into a table of the arrivals of
    /// one session, so it misses cheaply, and the distance tests it adds are
    /// the arrivals it actually holds nearby. A boosted jump widens that cube
    /// — four times the range is nine cells to a side rather than three — so
    /// an expansion out of a neutron star costs many times one out of an
    /// ordinary system, and there are far fewer of them.
    /// Collected, for the tests that ask what a jump reaches. The searches
    /// take them one at a time; see [`Self::neighbors_each`].
    #[cfg(test)]
    fn neighbors(&self, i: usize, range: f64, drive: Drive) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        self.neighbors_each(i, range, drive, |j| out.push(j));
        out
    }

    /// Hand each of them to `found` as it is found
    ///
    /// What the searches walk. A collected `Vec` is an allocation and a length
    /// per expansion, and there are half a million expansions in a route
    /// across the galaxy; the caller keeps whatever buffer it wants filled and
    /// this one keeps none.
    fn neighbors_each(
        &self,
        i: usize,
        range: f64,
        drive: Drive,
        mut found: impl FnMut(usize),
    ) {
        let range = range * drive.factor(self.boost(i));
        let p = self.place(i).1;
        let home = bucket_of(p);
        let reach = (range / BUCKET_LY).ceil() as i32;
        let held = self.base.points.len();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let cell = [home[0] + dx, home[1] + dy, home[2] + dz];
                    // The reach is a cube of buckets and a jump is a sphere
                    // inside it, so nearly half of them cannot hold anything
                    // in range — a boosted jump asks after seven hundred and
                    // twenty-nine buckets and three hundred and fifty of them
                    // are corners. Cheaper to measure the box than to measure
                    // every system in it.
                    if bucket_away(p, cell) > range * range {
                        continue;
                    }
                    let sets = [
                        (self.base.buckets.get(&cell), 0, &self.base.points),
                        (
                            self.fresh.buckets.get(&cell),
                            held,
                            &self.fresh.points,
                        ),
                    ];
                    for (bucket, offset, points) in sets {
                        let Some(bucket) = bucket else { continue };
                        for &j in bucket {
                            let there = points[j].1;
                            if j + offset != i
                                && dist2(p, there) <= range * range
                            {
                                found(j + offset);
                            }
                        }
                    }
                }
            }
        }
    }

    /// A route between two systems by address, at a ship's jump `range` and
    /// with `drive` fitted, as the hops it passes through. [`None`] where
    /// either end is unknown or no chain of jumps that long connects them.
    ///
    /// All three cost one per jump and estimate what is left as the
    /// straight-line distance in whole jumps. [`Routing::Direct`] and
    /// [`Routing::Shortest`] leave that estimate alone, so it can never
    /// overstate the jumps remaining and both come back with a genuine
    /// fewest-jumps route; what they choose between is the enormous number of
    /// chains that are all exactly that long. [`Routing::Quick`] multiplies it
    /// instead, and comes back with a route inside a twentieth of the fewest
    /// in a hundredth of the time.
    ///
    /// The estimate divides by the widest jump the drive could make and not by
    /// the range asked for, or a boost would let the route beat the estimate
    /// and the search would stop settling for the fewest jumps. See
    /// [`Drive::widest`].
    /// `watching` is filled in as the search runs, for the map to draw what it
    /// has reached; see [`super::frontier`]. [`None`] where nothing is
    /// watching, and the search then records nothing at all.
    pub(crate) fn route(
        &self,
        start: i64,
        end: i64,
        range: f64,
        how: Routing,
        drive: Drive,
        watching: Option<&Arc<Frontier>>,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        let start = self.index_of(start)?;
        let end = self.index_of(end)?;
        let goal = self.place(end).1;
        let mut sampled = watching.map(Frontier::sampler);
        let path = match how {
            Routing::Quick => {
                self.quick(start, end, goal, range, drive, &mut sampled)
            }
            Routing::Direct => {
                self.direct(start, end, goal, range, drive, &mut sampled)
            }
            Routing::Shortest => {
                self.shortest(start, end, goal, range, drive, &mut sampled)
            }
        };
        if let Some(sampled) = &mut sampled {
            sampled.done();
        }
        Some(path?.into_iter().map(|i| self.place(i)).collect())
    }

    /// A route inside a twentieth of the fewest jumps, and quickly
    ///
    /// [`Self::direct`] with the estimate leaned on. Everything else is the
    /// same walk: the same jump costs, the same tie-break, the same answer
    /// where the estimate happens to be tight. See [`Routing::Quick`] and
    /// [`LEANING`].
    fn quick(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<usize>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            // A jump costs the denominator, so the weighted estimate below
            // is an exact multiple of the admissible one rather than a float
            // rounded twice.
            |graph, i, out| {
                graph.neighbors_each(i, range, drive, |j| {
                    out.push((j, LEANING.1))
                });
            },
            |graph, i| {
                let left = dist2(graph.place(i).1, goal).sqrt();
                let jumps = (left / widest).ceil() as u32;
                jumps * LEANING.0
            },
            sampled,
        )
    }

    /// Fewest jumps, taking the neighbour that gets nearest the goal first
    ///
    /// Distance is not in the cost at all, so nothing here decides between two
    /// chains of the same length — the tie-break does. Nearest the goal first
    /// is what makes a route look like one, and it is a preference rather than
    /// a bound: no claim is made that the chain it finds is the shortest of
    /// them, only that each step reaches as far toward the goal as the ones
    /// beside it.
    fn direct(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<usize>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            // Unsorted: the tie-break in `search` is what prefers the step
            // nearest the goal, where this used to hand the neighbours over in
            // that order and lean on the crate's own tie-breaking.
            |graph, i, out| {
                graph.neighbors_each(i, range, drive, |j| out.push((j, 1u32)));
            },
            |graph, i| {
                (dist2(graph.place(i).1, goal).sqrt() / widest).ceil() as u32
            },
            sampled,
        )
    }

    /// Fewest jumps, and provably the shortest of them
    ///
    /// Distance goes into the cost under the jump count, so the search settles
    /// the tie itself instead of leaving it to the tie-break. See [`Cost`] for
    /// the ordering and for why the rounding leans as it does.
    fn shortest(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<usize>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            |graph, i, out| {
                let from = graph.place(i).1;
                graph.neighbors_each(i, range, drive, |j| {
                    let leg = dist2(from, graph.place(j).1).sqrt();
                    out.push((
                        j,
                        Cost { jumps: 1, light_years: leg.ceil() as u32 },
                    ));
                });
            },
            |graph, i| {
                let left = dist2(graph.place(i).1, goal).sqrt();
                Cost {
                    jumps: (left / widest).ceil() as u32,
                    light_years: left.floor() as u32,
                }
            },
            sampled,
        )
    }

    /// A* over the jump graph, keeping where each system was reached from
    ///
    /// Its own loop rather than the crate's, for the parent map. A* keeps one
    /// as a matter of course — it is what the answer is reconstructed from —
    /// and a library that hands back only the path leaves the map to guess at
    /// it: an earlier cut recorded the most promising few steps of every
    /// expansion instead, and a supercharged route across the galaxy came back
    /// with a chain two links long, a jump out of a neutron star landing
    /// somewhere no expansion had thought its own best step. Here the chain to
    /// the closest system reached is exact and costs nothing, being the same
    /// map the route itself comes out of.
    ///
    /// Ties are broken by distance to the goal. Where the cost carries
    /// distance ([`Cost`]) it settles its own ties and this does nothing; where
    /// it counts jumps alone, every chain of the same length costs the same
    /// and this is what picks the one that heads at the goal instead of the
    /// one that happened to be reached first.
    ///
    /// Everything a system is remembered by is an array indexed by its place
    /// in the graph, not a map keyed by it: a route across the galaxy expands
    /// half a million systems and looks at forty million neighbours, and three
    /// hashes per neighbour was the whole of where the time went. Three arrays
    /// over two and a half million systems come to twenty megabytes, held for
    /// as long as the search runs.
    fn search<C: Metric>(
        &self,
        start: usize,
        end: usize,
        goal: [f64; 3],
        successors: impl Fn(&JumpGraph, usize, &mut Vec<(usize, C)>),
        estimate: impl Fn(&JumpGraph, usize) -> C,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<usize>> {
        if start == end {
            return Some(vec![start]);
        }
        let held = self.len();
        // What each system has been reached for, and where it was reached
        // from. No closed set: a system is expanded again if a cheaper way to
        // it turns up, and the entry a cheaper way replaced is recognised on
        // the way out of the heap by the cost it was pushed with.
        //
        // Which is not an optimisation to skip. Where the estimate is exact it
        // never fires — an exact estimate never reaches a system for less
        // after settling it — so it costs the comparison and nothing else.
        // Where the estimate is leaned on ([`Routing::Quick`]) it can, the
        // weighting being inconsistent by up to a jump, and the bound that
        // setting claims is the one weighted A* proves *with* reopening. Held
        // shut instead, a route inside a twentieth would be a hope.
        let mut best = vec![C::MAX; held];
        let mut came = vec![UNSEEN; held];
        // Nearest the goal is the smallest number and a heap pops the
        // greatest, so the whole key is reversed: the ordering is "cheapest
        // first, and of those the one nearest the goal".
        let mut open = BinaryHeap::new();
        let mut near: Vec<(usize, C)> = Vec::new();

        let away = |graph: &JumpGraph, i: usize| {
            // Squared, and as an integer: the key is only ever compared, and
            // squaring is monotone over distances that are never negative, so
            // the ordering is the same one a square root would give and the
            // root itself is forty million calls nobody reads.
            dist2(graph.place(i).1, goal) as u64
        };
        best[start] = C::ZERO;
        open.push(Reverse((
            estimate(self, start),
            away(self, start),
            C::ZERO,
            start,
        )));

        while let Some(Reverse((_, _, was, node))) = open.pop() {
            // A system can be pushed more than once, a cheaper way to it
            // having been found after the first; the dearer entries are still
            // in the heap and are nothing to expand again.
            if was > best[node] {
                continue;
            }
            if node == end {
                let mut path = vec![end];
                while came[*path.last().expect("a step")] != UNSEEN {
                    path.push(came[*path.last().expect("a step")] as usize);
                }
                path.reverse();
                return Some(path);
            }
            if let Some(sampled) = sampled.as_mut() {
                sampled.expanded(
                    node,
                    DVec3::from(self.place(node).1),
                    DVec3::from(goal),
                    &came,
                    |i| DVec3::from(self.place(i).1),
                );
            }
            near.clear();
            successors(self, node, &mut near);

            for &(next, step) in &near {
                let cost = was + step;
                if cost < best[next] {
                    best[next] = cost;
                    came[next] = node as u32;
                    open.push(Reverse((
                        cost + estimate(self, next),
                        away(self, next),
                        cost,
                        next,
                    )));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system named for its address, at `at`.
    fn at(address: i64, at: [f32; 3]) -> NameEntry {
        NameEntry { address, name: format!("S{address}"), position: at }
    }

    /// How far a route runs, following its legs.
    fn run(path: &[(i64, [f64; 3])]) -> f64 {
        path.windows(2).map(|w| dist2(w[0].1, w[1].1).sqrt()).sum()
    }

    /// Both settings, since every claim below holds of both.
    const BOTH: [Routing; 2] = [Routing::Direct, Routing::Shortest];

    /// Of two chains the same number of jumps long, the route follows the
    /// straighter
    ///
    /// The reported trouble, in miniature: counting jumps alone makes every
    /// chain of five equally good, so a detour off the line and back is free
    /// and the line drawn wanders. The detour is offered to the search first,
    /// by being built first, so a router that takes whatever it reaches first
    /// takes the detour.
    ///
    /// [`Routing::Direct`] gets here by preferring the neighbour nearest the
    /// goal and [`Routing::Shortest`] by costing the distance, so the two
    /// arrive by different means and must agree on the answer.
    #[test]
    fn a_route_of_equal_jumps_follows_the_straighter_chain() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        // Five jumps off the line and back, all inside a 500 ly range.
        for (k, side) in [(1, 150.), (2, -150.), (3, 150.), (4, -150.)] {
            entries.push(at(100 + k, [400.0 * k as f32, side, 0.]));
        }
        // And five straight down it.
        for k in 1..=4 {
            entries.push(at(k, [400.0 * k as f32, 0., 0.]));
        }
        entries.push(at(9, [2000., 0., 0.]));
        let graph = JumpGraph::new(&entries, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(0, 9, 500., how, Drive::Unaided, None)
                .expect("a route");

            assert_eq!(path.len() - 1, 5, "not five jumps, {how:?}");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![0, 1, 2, 3, 4, 9],
                "{how:?} wandered off the line, running {:.0} ly against 2000",
                run(&path)
            );
        }
    }

    /// And fewer jumps beats a shorter way, whichever setting is asked
    ///
    /// The half neither setting may undo: a jump is fuel and time, so two long
    /// jumps beat five short ones over the same ground. Both chains here run
    /// 900 light years, so nothing but the count can choose between them —
    /// which is what [`Cost`]'s field order is for on the one side, and what
    /// costing only jumps gives outright on the other.
    #[test]
    fn a_route_takes_the_fewest_jumps_before_the_straightest() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        // Five short hops.
        for k in 1..=4 {
            entries.push(at(k, [180.0 * k as f32, 0., 0.]));
        }
        // Two long ones, over the same ground.
        entries.push(at(50, [450., 0., 0.]));
        entries.push(at(9, [900., 0., 0.]));
        let graph = JumpGraph::new(&entries, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(0, 9, 500., how, Drive::Unaided, None)
                .expect("a route");

            assert_eq!(path.len() - 1, 2, "{how:?} took the short hops");
            assert_eq!(path[1].0, 50, "{how:?} missed the far waypoint");
        }
    }

    /// A quick route is a route: real jumps, and none of them cheating
    ///
    /// [`Routing::Quick`] leans on the estimate until it overstates, which is
    /// what stops A* expanding the plateau of systems that could have been on
    /// an equally short chain — and a bound on how much worse the answer may
    /// be is all that weighting an estimate gives back. What it must not touch
    /// is what a jump is: every leg inside the range asked for, and never
    /// fewer jumps than the fewest, which would mean the search had found a
    /// chain the proof says does not exist.
    ///
    /// A corridor with two lanes down it is that plateau in miniature: every
    /// crossing between them makes another chain of the same length, and the
    /// chains number two to the thirtieth.
    #[test]
    fn a_quick_route_is_made_of_real_jumps() {
        let mut entries = vec![at(0, [0., 0., 0.])];
        for k in 1..=30 {
            entries.push(at(k, [90.0 * k as f32, 0., 0.]));
            entries.push(at(100 + k, [90.0 * k as f32, 40., 0.]));
        }
        // Named outside the lanes, which hold 1..=30 and 101..=130.
        entries.push(at(999, [2790., 0., 0.]));
        // One neutron star in the corridor, so the range a jump may take and
        // the range the estimate divides by are different numbers — which is
        // what a leg has to be measured against, and what a search reaching
        // by the wrong one of the two would be caught by below.
        let boosts =
            Boosts(std::sync::Arc::new(HashMap::from([(15, Boost::Neutron)])));
        let graph = JumpGraph::new(&entries, &boosts);
        let drive = Drive::Standard;

        let fewest = graph
            .route(0, 999, 100., Routing::Direct, drive, None)
            .expect("a route")
            .len()
            - 1;
        let quick = graph
            .route(0, 999, 100., Routing::Quick, drive, None)
            .expect("a quick route");

        assert_eq!(quick.first().expect("a start").0, 0, "started elsewhere");
        assert_eq!(quick.last().expect("an end").0, 999, "ended elsewhere");
        assert!(
            quick.len() - 1 >= fewest,
            "{} jumps beats the fewest there are, {fewest}",
            quick.len() - 1
        );
        for leg in quick.windows(2) {
            let flown = dist2(leg[0].1, leg[1].1).sqrt();
            // What the system being left could charge the drive to, which is
            // the only thing that says how far a jump out of it may go.
            let reach = 100.
                * drive.factor(
                    graph
                        .index_of(leg[0].0)
                        .map(|i| graph.boost(i))
                        .unwrap_or(None),
                );
            assert!(
                flown <= reach,
                "a leg of {flown:.0} ly out of {} reaching {reach:.0}",
                leg[0].0
            );
        }
    }

    /// Where the two part: only one of them proves the shortest chain
    ///
    /// A detour that is *not* offered first and is not the nearest step at any
    /// point — its first leg goes almost sideways — so preferring the nearest
    /// neighbour walks straight past it. Both find four jumps; the point is
    /// that [`Routing::Shortest`] is the one that has proved no shorter four
    /// exists, and the map's default has only preferred one.
    #[test]
    fn only_the_shortest_setting_proves_what_it_found() {
        let entries = vec![
            at(0, [0., 0., 0.]),
            at(1, [450., 0., 0.]),
            at(2, [900., 0., 0.]),
            at(3, [1350., 0., 0.]),
            at(9, [1800., 0., 0.]),
        ];
        let graph = JumpGraph::new(&entries, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(0, 9, 500., how, Drive::Unaided, None)
                .expect("a route");
            assert_eq!(path.len() - 1, 4, "{how:?}");
            assert!(
                (run(&path) - 1800.).abs() < 1.,
                "{how:?} ran {:.0} ly down a 1800 ly line",
                run(&path)
            );
        }
    }

    /// A goal nothing reaches is no route rather than a wrong one
    #[test]
    fn a_gap_wider_than_the_range_is_no_route() {
        let entries = vec![at(0, [0., 0., 0.]), at(1, [600., 0., 0.])];
        let graph = JumpGraph::new(&entries, &Boosts::default());

        for how in BOTH {
            assert!(
                graph.route(0, 1, 500., how, Drive::Unaided, None).is_none(),
                "{how:?} jumped 600 at 500"
            );
            assert!(
                graph.route(0, 1, 700., how, Drive::Unaided, None).is_some(),
                "{how:?} refused 600 inside 700"
            );
        }
    }

    /// A neighbour search reaches past its own bucket
    ///
    /// The buckets are [`BUCKET_LY`] on a side and a range many times that
    /// has to look many buckets out, or a route would only ever step to the
    /// system next door.
    #[test]
    fn a_range_wider_than_a_bucket_still_finds_its_neighbours() {
        let far = BUCKET_LY as f32 * 6.;
        let entries = vec![at(0, [0., 0., 0.]), at(1, [far, 0., 0.])];
        let graph = JumpGraph::new(&entries, &Boosts::default());

        assert_eq!(
            graph.neighbors(0, far as f64 + 1., Drive::Unaided),
            vec![1]
        );
        assert!(graph.neighbors(0, far as f64 - 1., Drive::Unaided).is_empty());
    }

    /// A system named since the table was read is routable, as an end and as
    /// a waypoint
    ///
    /// The router reads the names table, and the map reads that table once at
    /// startup. A system the feed named while the map ran was not in the graph
    /// at all, so a route to it came back with nothing and a route past it
    /// took the long way round — which is what [`crate::refresh`] hands the
    /// arrivals here for.
    #[test]
    fn a_system_named_since_is_routable() {
        // Two ends 900 ly apart, too far for one 500 ly jump, with nothing
        // between them when the table was read.
        let base = vec![at(0, [0., 0., 0.]), at(9, [900., 0., 0.])];
        let graph = JumpGraph::new(&base, &Boosts::default());
        for how in BOTH {
            assert!(
                graph.route(0, 9, 500., how, Drive::Unaided, None).is_none(),
                "{how:?} crossed 900 ly at a 500 ly range"
            );
        }

        // The feed names one in the middle, and one further out again.
        let arrivals = vec![at(50, [450., 0., 0.]), at(99, [1350., 0., 0.])];
        let grown = graph.extended(&arrivals, &Boosts::default());

        assert_eq!(grown.len(), 4, "two known systems and two arrivals");
        for how in BOTH {
            let path = grown
                .route(0, 9, 500., how, Drive::Unaided, None)
                .expect("a route");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![0, 50, 9],
                "{how:?} did not route through the arrival"
            );

            let path = grown
                .route(0, 99, 500., how, Drive::Unaided, None)
                .expect("a route to it");
            assert_eq!(
                path.last().map(|(a, _)| *a),
                Some(99),
                "{how:?} could not reach the arrival itself"
            );
            assert_eq!(path.len() - 1, 3, "{how:?} took the wrong count");
        }

        // And the base is untouched by any of it: the graph it was asked for
        // is the graph it keeps, which is what a route in flight holds.
        for how in BOTH {
            assert!(
                graph.route(0, 9, 500., how, Drive::Unaided, None).is_none(),
                "{how:?} saw an arrival the graph it holds never had"
            );
        }
    }

    /// An arrival under an address the table already names is left alone
    ///
    /// A rename is nothing to a router: it asks where a system is. Taking one
    /// anyway would bucket the same system twice and let a search route
    /// through either copy.
    #[test]
    fn a_rename_does_not_double_a_system() {
        let base = vec![at(0, [0., 0., 0.]), at(1, [450., 0., 0.])];
        let graph = JumpGraph::new(&base, &Boosts::default());

        let renamed = vec![NameEntry {
            address: 1,
            name: "Renamed".into(),
            position: [450., 0., 0.],
        }];
        let grown = graph.extended(&renamed, &Boosts::default());

        assert_eq!(grown.len(), 2, "the renamed system is held once");
        assert_eq!(
            grown
                .neighbors(
                    grown.index_of(0).expect("an index"),
                    500.,
                    Drive::Unaided,
                )
                .len(),
            1,
            "and offered as one neighbour, not two"
        );
    }

    /// A boost is worth what the drive fitted makes of it, and nothing
    /// unaided
    ///
    /// The multipliers the game gives: four times off a neutron star and half
    /// again off a white dwarf, six and three off the drive built for it. A
    /// scale of one where there is no boost to be had or no drive to take it,
    /// so a caller can scale by this without asking first.
    #[test]
    fn a_boost_is_worth_what_the_drive_makes_of_it() {
        for drive in [Drive::Unaided, Drive::Standard, Drive::Optimised] {
            assert_eq!(drive.factor(None), 1., "no jet cone, {drive:?}");
        }
        for boost in [Boost::Neutron, Boost::WhiteDwarf] {
            assert_eq!(
                Drive::Unaided.factor(Some(boost)),
                1.,
                "no drive to take it, {boost:?}"
            );
        }

        assert_eq!(Drive::Standard.factor(Some(Boost::Neutron)), 4.);
        assert_eq!(Drive::Standard.factor(Some(Boost::WhiteDwarf)), 1.5);
        assert_eq!(Drive::Optimised.factor(Some(Boost::Neutron)), 6.);
        assert_eq!(Drive::Optimised.factor(Some(Boost::WhiteDwarf)), 3.);

        // What the estimate of the jumps left has to divide by: the widest
        // any one of them could be.
        assert_eq!(Drive::Unaided.widest(), 1.);
        assert_eq!(Drive::Standard.widest(), 4.);
        assert_eq!(Drive::Optimised.widest(), 6.);
    }

    /// A neutron star carries a jump no unboosted ship could make
    ///
    /// Which is the whole of what supercharging is for. The gap out of the
    /// neutron star is three and a half times the ship's own reach, so unaided
    /// there is no route at all; with a standard drive it is one jump of the
    /// four the star is worth.
    #[test]
    fn a_neutron_star_carries_a_jump_the_ship_could_not_make() {
        let entries = vec![
            at(1, [0., 0., 0.]),
            // The neutron star, one ordinary jump along.
            at(2, [90., 0., 0.]),
            // And the far side of a gap only a boost crosses.
            at(9, [440., 0., 0.]),
        ];
        let boosts =
            Boosts(std::sync::Arc::new(HashMap::from([(2, Boost::Neutron)])));
        let graph = JumpGraph::new(&entries, &boosts);

        for how in BOTH {
            assert!(
                graph.route(1, 9, 100., how, Drive::Unaided, None).is_none(),
                "{how:?} crossed 350 ly at a 100 ly range unaided"
            );

            let path = graph
                .route(1, 9, 100., how, Drive::Standard, None)
                .expect("a supercharged route");
            assert_eq!(
                path.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
                vec![1, 2, 9],
                "{how:?} did not go by way of the neutron star"
            );
        }
    }

    /// The boost is spent on the jump out, not on the one that arrives
    ///
    /// A charge is taken in the jet cone and held until a jump uses it, so
    /// what a jump may reach is the star of the system it leaves. Reading it
    /// off the system landed in instead would let a ship cross the gap first
    /// and pick up the boost afterwards, which is a route it cannot fly.
    #[test]
    fn the_boost_belongs_to_the_system_a_jump_leaves() {
        // The neutron star is at the far side of the gap this time.
        let entries = vec![
            at(1, [0., 0., 0.]),
            at(2, [90., 0., 0.]),
            at(9, [440., 0., 0.]),
        ];
        let boosts =
            Boosts(std::sync::Arc::new(HashMap::from([(9, Boost::Neutron)])));
        let graph = JumpGraph::new(&entries, &boosts);

        for how in BOTH {
            assert!(
                graph.route(1, 9, 100., how, Drive::Standard, None).is_none(),
                "{how:?} flew a gap on a boost it had not collected yet"
            );
        }
    }

    /// White dwarfs are worth half again, and only the drive says how much
    ///
    /// The same gap, at a range only the optimised drive's threefold boost
    /// crosses: a standard drive's half again is not enough, so the two
    /// settings differ on the same sky rather than on the same ship.
    #[test]
    fn a_white_dwarf_carries_what_the_drive_allows() {
        let entries = vec![at(1, [0., 0., 0.]), at(9, [250., 0., 0.])];
        let boosts = Boosts(std::sync::Arc::new(HashMap::from([(
            1,
            Boost::WhiteDwarf,
        )])));
        let graph = JumpGraph::new(&entries, &boosts);

        for how in BOTH {
            assert!(
                graph.route(1, 9, 100., how, Drive::Standard, None).is_none(),
                "{how:?} made 250 ly of a 150 ly boosted jump"
            );
            assert!(
                graph.route(1, 9, 100., how, Drive::Optimised, None).is_some(),
                "{how:?} refused 250 ly of a 300 ly boosted jump"
            );
        }
    }
}
