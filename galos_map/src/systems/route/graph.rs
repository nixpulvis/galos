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
use galos_index::meta::Boost;
use galos_index::{Node, Sky};
use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

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

/// Whether a setting is allowed to approximate.
///
/// The one rule the search obeys: **an approximation belongs to
/// [`Routing::Quick`] and to nothing else.** A setting that claims the
/// fewest jumps has to mean it, so everything that trades exactness for
/// speed — the leaned-on estimate, the fanout cap, and whatever comes after
/// — is switched on here together and nowhere apart.
///
/// Which is why this is a method on the setting rather than a parameter
/// threaded through the searches: the next approximation is added by
/// reading this, and cannot be added by forgetting to.
impl Routing {
    /// How many of an expansion's neighbours may be relaxed, or [`None`]
    /// for all of them.
    ///
    /// A cap is the fix for a search whose work is quadratic in stellar
    /// density — a boosted jump in the core sees thousands of systems — but
    /// it can drop the very neighbour a fewest-jumps chain went through, so
    /// only a setting that has not promised the fewest may have one.
    fn fanout(&self) -> Option<usize> {
        match self {
            Routing::Quick => Some(FANOUT),
            Routing::Direct | Routing::Shortest => None,
        }
    }

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
/// A search keeps one of these per system it has reached, so what "nothing
/// has reached this yet" means is that the map has no entry at all rather
/// than a value of the type saying so.
trait Metric: Ord + Copy + std::ops::Add<Output = Self> {
    /// What the system the search sets out from has cost so far
    const ZERO: Self;
}

impl Metric for u32 {
    const ZERO: u32 = 0;
}

impl Metric for Cost {
    const ZERO: Cost = Cost { jumps: 0, light_years: 0 };
}

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
    ///
    /// Read on its own by [`super::frontier::draw`] before it takes a copy,
    /// so a frame where nothing has moved costs one lock and one compare
    /// rather than a walk of the whole closed set.
    revision: u64,
    /// How many times the closed set has grown, or been drawn coarser
    cells_at: u64,
    /// How many times the leading edge has moved
    edge_at: u64,
    /// How many times the chain to the closest reached has moved
    ///
    /// The three of these are what let a flush that only added a cell leave
    /// the edge's and the chain's meshes alone. [`Self::revision`] is their
    /// disjunction, and is what says whether to look at them at all.
    reaching_at: u64,
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
    /// How many times the closed set has grown or coarsened
    pub(crate) cells_at: u64,
    /// How many times the leading edge has moved
    pub(crate) edge_at: u64,
    /// How many times the chain has moved
    pub(crate) reaching_at: u64,
}

impl Frontier {
    /// A frontier for a search from `from` to `goal`
    ///
    /// The cell of the closed set is [`super::frontier::CELLS`]-th of the way
    /// between them, so the picture is about that many cells along the route
    /// whether that is two hundred light years or twenty-two thousand. Which
    /// is what bounds the set by the geometry rather than by a count: the
    /// cells the search touches are the corridor it searched, and a corridor
    /// is not much wider than the line through it.
    ///
    /// A corridor is what a search that has a route to find walks. One that
    /// has not expands in every direction until the reachable component runs
    /// out, and then the geometry bounds nothing — so the cell is widened
    /// again past [`super::frontier::CELL_CEILING`] of them; see
    /// [`Sampler::flush`].
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
            stepped: false,
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
            cells_at: reached.cells_at,
            edge_at: reached.edge_at,
            reaching_at: reached.reaching_at,
        })
    }

    /// Where the search set out from, once its ends have resolved
    ///
    /// Fixed for the whole of a search, so [`super::frontier::draw`] takes it
    /// once and keeps it: the depth a mark is sized at is measured from here,
    /// and that has to be known before the revision can be weighed against a
    /// zoom that has moved.
    pub(crate) fn from(&self) -> Option<DVec3> {
        self.0.lock().expect("the frontier lock").from
    }

    /// How many times the picture has moved; see [`Reached::revision`]
    ///
    /// The whole of what [`super::frontier::draw`] needs to know whether to
    /// take a copy at all. Asked first and on its own, since [`Self::drawn`]
    /// walks every cell of the closed set and clones the chain, under the lock
    /// the search flushes through — work worth nothing on a frame where the
    /// picture has not changed, which is most of them.
    pub(crate) fn revision(&self) -> u64 {
        self.0.lock().expect("the frontier lock").revision
    }

    /// How many systems the search has expanded.
    pub(crate) fn expanded(&self) -> u64 {
        self.0.lock().expect("the frontier lock").expanded
    }

    /// Whether nothing more will come of this search
    ///
    /// Set when the search stops of its own accord ([`Sampler::done`]) and
    /// when its leg is given up on ([`Self::abandon`]), the map having the
    /// same thing to do either way: take the layers down.
    pub(crate) fn finished(&self) -> bool {
        self.0.lock().expect("the frontier lock").finished
    }

    /// Give up on this search, its leg having been cancelled
    ///
    /// A route task dropped before the pool has begun polling it never runs,
    /// so nothing calls [`Sampler::done`] and the frontier would sit here
    /// unfinished for the rest of the session — three layer entities apiece,
    /// re-uploaded on every zoom, and counted by [`Frontiers::expanded`] and
    /// [`Frontiers::closest`] that the form reads. A leg the pool had already
    /// begun does finish its own body and needs none of this; a queued one,
    /// which is every leg past the pool's width, needs it.
    ///
    /// [`Frontiers::expanded`]: super::frontier::Frontiers::expanded
    /// [`Frontiers::closest`]: super::frontier::Frontiers::closest
    pub(crate) fn abandon(&self) {
        self.0.lock().expect("the frontier lock").finished = true;
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

/// The cell a place falls in once the grid is twice as wide
///
/// Euclidean division, which is the whole of why it is exact: the cell of a
/// grid `2w` wide is `floor(x / 2w)`, and that is `floor(floor(x / w) / 2)`
/// for every sign of `x`. So a set of cells can be widened without going back
/// to the places that filled it.
fn coarser(cell: [i32; 3]) -> [i32; 3] {
    [cell[0].div_euclid(2), cell[1].div_euclid(2), cell[2].div_euclid(2)]
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
    /// Whether the edge has moved since the last flush, so a flush that only
    /// added a cell leaves the edge's mesh where it is
    stepped: bool,
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
    /// from — the thing A* keeps in order to give an answer at all — so the
    /// jump drawn is the jump the search took to get here and the chain is
    /// the one it would hand back if this were the goal. `place` says where
    /// a system sits, and is asked only where something is drawn or the
    /// record moves.
    pub(crate) fn expanded(
        &mut self,
        node: Node,
        at: DVec3,
        goal: DVec3,
        came: &FxHashMap<Node, Node>,
        place: impl Fn(Node) -> DVec3,
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
            self.stepped = true;
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
        node: Node,
        came: &FxHashMap<Node, Node>,
        place: &impl Fn(Node) -> DVec3,
    ) {
        let mut at = node;
        let mut chain = vec![place(at)];
        while let Some(&before) = came.get(&at) {
            at = before;
            chain.push(place(at));
            // A walk that will not end is a link cycle rather than a route,
            // and one drawn forever would be a hang.
            if chain.len() > came.len() + 1 {
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
    /// chain is handed over where it has moved. Nothing is thrown away: the
    /// edge is bounded by its own length, and the cells by the corridor
    /// searched — or, where the search is not walking a corridor, by being
    /// drawn coarser.
    ///
    /// That is the loop at the end. Past [`super::frontier::CELL_CEILING`]
    /// cells the grid doubles and every cell held is mapped onto the wider
    /// one, which is exact ([`coarser`]) and needs none of the places back.
    /// The sampler's own width goes with it, so what it counts next lands on
    /// the same grid; so does its edge, which is copied over whole every
    /// flush and would otherwise put cells of the old width back. A doubling
    /// takes about eight cells to one, so the loop runs once in practice and
    /// terminates in any case.
    ///
    /// Each layer's own revision moves only where that layer did, so a flush
    /// that added a cell and nothing else leaves the edge's and the chain's
    /// meshes alone. [`Reached::revision`] moves where any of them did, and is
    /// what the map reads first: unchanged, it never asks for the copy.
    fn flush(&mut self) {
        let mut reached = self.into.0.lock().expect("the frontier lock");
        reached.expanded = self.expanded;

        let grew = !self.cells.is_empty();
        reached.cells.extend(self.cells.drain());
        if grew {
            reached.cells_at += 1;
        }

        let stepped = std::mem::take(&mut self.stepped);
        if stepped {
            reached.edge.clear();
            reached.edge.extend(self.edge.iter().copied());
            reached.edge_at += 1;
        }

        // Taken rather than read, so the chain is handed over on the flush
        // after it moved and not on every flush thereafter.
        let settled = std::mem::take(&mut self.settled);
        if settled {
            reached.closest = self.closest;
            reached.reaching = std::mem::take(&mut self.reaching);
            reached.reaching_at += 1;
        }

        let mut coarsened = false;
        while reached.cells.len() > super::frontier::CELL_CEILING {
            reached.across *= 2.;
            reached.cells =
                reached.cells.iter().copied().map(coarser).collect();
            for cell in reached.edge.iter_mut() {
                *cell = coarser(*cell);
            }
            self.across = reached.across;
            for cell in self.edge.iter_mut() {
                *cell = coarser(*cell);
            }
            self.worked = self.worked.map(coarser);
            coarsened = true;
        }
        // A wider cell moves every mark of both sampled layers, whatever else
        // happened this flush.
        if coarsened {
            reached.cells_at += 1;
            reached.edge_at += 1;
        }

        if grew || stepped || settled || coarsened {
            reached.revision += 1;
        }
    }

    /// Say the search has stopped, and hand over whatever is left.
    pub(crate) fn done(&mut self) {
        self.flush();
        self.into.0.lock().expect("the frontier lock").finished = true;
    }
}

/// The router's galaxy, and its graph once something has asked for a route.
///
/// The graph is [`None`] until then, and it costs almost nothing to make:
/// the galaxy's places are the cell payloads, mapped as a query reaches
/// them, so a graph is a handle on the index and not a structure over it. It
/// used to be a grid of its own — 32 bytes a system of points, an address
/// map beside them and a bucket per occupied cell, 13.7 GB and 32 s at
/// 200 M, paid on the click that asked for a route.
#[derive(Resource, Clone, Default)]
pub struct Jumps {
    /// The graph, once a route has asked for one.
    pub graph: Option<Arc<JumpGraph>>,
    /// The galaxy it reads, opened where the index is a directory this
    /// process can map. [`None`] over a transport that cannot be mapped,
    /// and then nothing routes.
    pub sky: Option<Arc<Sky>>,
}

impl Jumps {
    /// The galaxy as the router reads it, opened once for the session.
    pub fn over(sky: Arc<Sky>) -> Jumps {
        Jumps { graph: None, sky: Some(sky) }
    }

    /// The graph, opening it if this is the first route of the session.
    pub fn built(&mut self, boosts: &Boosts) -> Option<Arc<JumpGraph>> {
        let sky = self.sky.clone()?;
        let held = self
            .graph
            .get_or_insert_with(|| Arc::new(JumpGraph::over(&sky, boosts)));
        Some(Arc::clone(held))
    }
}

/// The galaxy a route is searched over: the mapped cell payloads, and what
/// can supercharge a drive.
///
/// A route in flight holds the graph it started on and finishes against
/// that. It is the right answer as well as the cheap one: a search half-run
/// against a galaxy that grew underneath it has been searching two
/// different skies. The payloads make that hold for free — a cell the feed
/// republishes is renamed into place, so a mapping this holds keeps reading
/// what it was given ([`galos_index::store`]).
#[derive(Clone)]
pub struct JumpGraph {
    /// The galaxy's places, read where they lie.
    sky: Arc<Sky>,
    /// Which systems can supercharge a drive, as published.
    ///
    /// Held beside the places rather than folded into them, though a boost
    /// is a fact about a place. The table moves on nearly every publish —
    /// four systems in a hundred can supercharge and the feed names eighty
    /// a minute — and it is read per expansion, which is one lookup for the
    /// system being left rather than one for each of the thousands it can
    /// see.
    boosts: Boosts,
}

/// How many of an expansion's neighbours are relaxed.
///
/// The fix for a search whose work is quadratic in stellar density. A
/// boosted jump in the core reaches a sphere holding thousands of systems,
/// and relaxing every one costs milliseconds *per expansion* — the same
/// place EDDA landed, whose `LEG_FANOUT` is this number and whose audit
/// found the result tracks corridor density at 0.66–1.03× rather than its
/// square.
///
/// The ones kept are those that get nearest the goal, so what is thinned is
/// the half of the sphere a route was never going to step into.
const FANOUT: usize = 512;

/// The squared distance between two points, the distance itself wanted for
/// nothing here but comparing.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

impl JumpGraph {
    /// The graph over a galaxy and the supercharge table beside it.
    ///
    /// Nothing is built. The places are the cell payloads, read where they
    /// lie, which is why this is instant where the grid it replaced was 32
    /// seconds.
    pub fn over(sky: &Arc<Sky>, boosts: &Boosts) -> JumpGraph {
        JumpGraph { sky: Arc::clone(sky), boosts: boosts.clone() }
    }

    /// How many systems the graph can route between.
    pub fn len(&self) -> usize {
        self.sky.len() as usize
    }

    /// Whether it holds no places at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where a node sits. A node a mapping no longer holds reads as the
    /// origin rather than panicking a route task.
    fn place(&self, node: Node) -> [f64; 3] {
        self.sky.place(node).unwrap_or_default()
    }

    /// What a node's system is called.
    fn address(&self, node: Node) -> i64 {
        self.sky.address(node).unwrap_or_default()
    }

    /// What the system at a node can supercharge, if anything.
    fn boost(&self, node: Node) -> Option<Boost> {
        self.boosts.get(self.address(node))
    }

    /// Which node holds `address`, given where the names table says it sits.
    fn node_of(&self, address: i64, near: [f64; 3]) -> Option<Node> {
        self.sky.node_of(address, near)
    }

    /// The systems a node can jump to, with their places.
    ///
    /// `range` is what the ship reaches unaided, and the jump is scaled by
    /// whatever the system being left can supercharge into the `drive`
    /// fitted: a neutron star is four times as far, or six. Which is why a
    /// boost is read off the system a jump leaves rather than the one it
    /// lands in — the charge is taken in the jet cone and spent on the jump
    /// out.
    ///
    /// **Capped where `fanout` says so**, keeping the neighbours that get
    /// nearest `goal`. A boosted jump in the core sees thousands of systems
    /// and relaxing every one is what makes the search quadratic in
    /// density — but a cap can drop the very neighbour a fewest-jumps chain
    /// went through, so only a setting that has not promised the fewest
    /// carries one. See [`Routing::fanout`].
    ///
    /// `out` is the caller's buffer, cleared here: there are half a million
    /// expansions in a route across the galaxy and an allocation apiece is
    /// not worth paying.
    fn neighbors(
        &self,
        node: Node,
        at: [f64; 3],
        range: f64,
        drive: Drive,
        goal: [f64; 3],
        fanout: Option<usize>,
        out: &mut Vec<(Node, [f64; 3], f64)>,
    ) {
        out.clear();
        let range = range * drive.factor(self.boost(node));
        self.sky.each_near(at, range, |found, place, away| {
            if found != node {
                out.push((found, place, away));
            }
        });
        if let Some(cap) = fanout
            && out.len() > cap
        {
            // Nearest the goal first, and only far enough into the order to
            // find the boundary: the rest are dropped unsorted.
            out.select_nth_unstable_by(cap, |a, b| {
                let (a, b) = (dist2(a.1, goal), dist2(b.1, goal));
                a.total_cmp(&b)
            });
            out.truncate(cap);
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
        start: (i64, [f64; 3]),
        end: (i64, [f64; 3]),
        range: f64,
        how: Routing,
        drive: Drive,
        watching: Option<&Arc<Frontier>>,
    ) -> Option<Vec<(i64, [f64; 3])>> {
        // The ends are the one thing a route knows by *name*: a commander
        // picked them, so the names table says where they are and this finds
        // the records. Twice a leg, against the millions of places the
        // search itself reads straight out of the payloads.
        let from = self.node_of(start.0, start.1)?;
        let to = self.node_of(end.0, end.1)?;
        let goal = self.place(to);
        let mut sampled = watching.map(Frontier::sampler);
        let path = match how {
            Routing::Quick => {
                self.quick(from, to, goal, range, drive, how, &mut sampled)
            }
            Routing::Direct => {
                self.direct(from, to, goal, range, drive, how, &mut sampled)
            }
            Routing::Shortest => {
                self.shortest(from, to, goal, range, drive, how, &mut sampled)
            }
        };
        if let Some(sampled) = &mut sampled {
            sampled.done();
        }
        Some(
            path?
                .into_iter()
                .map(|node| (self.address(node), self.place(node)))
                .collect(),
        )
    }

    /// A route inside a twentieth of the fewest jumps, and quickly
    ///
    /// [`Self::direct`] with the estimate leaned on. Everything else is the
    /// same walk: the same jump costs, the same tie-break, the same answer
    /// where the estimate happens to be tight. See [`Routing::Quick`] and
    /// [`LEANING`].
    fn quick(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            // A jump costs the denominator, so the weighted estimate below
            // is an exact multiple of the admissible one rather than a float
            // rounded twice.
            |_, _, _| LEANING.1,
            // Saturating, because the range is whatever was typed into the
            // form and a small enough one puts more jumps between two systems
            // than a `u32` holds: the cast pins at the top and the scaling
            // would then overflow. A saturated estimate is still an
            // overstatement of a distance nothing can cross, which is what
            // the search does with it.
            |at| {
                let left = dist2(at, goal).sqrt();
                let jumps = (left / widest).ceil() as u32;
                jumps.saturating_mul(LEANING.0)
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
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            |_, _, _| 1u32,
            |at| (dist2(at, goal).sqrt() / widest).ceil() as u32,
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
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        let widest = range * drive.widest();
        self.search(
            start,
            end,
            goal,
            range,
            drive,
            how,
            // The leg's length comes off the neighbour query, which measured
            // it to decide the system was in range at all.
            |_, _, leg| Cost { jumps: 1, light_years: leg.ceil() as u32 },
            |at| {
                let left = dist2(at, goal).sqrt();
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
    /// **What a system is remembered by is a map over what the search
    /// reached**, not an array over the galaxy. The arrays were the right
    /// answer at 2.6 M systems, where three of them came to twenty megabytes
    /// and a hash per neighbour was the whole of where the time went; at
    /// 200 M they are 1.6 GB a leg, allocated before the first expansion, for
    /// a search that will touch a corridor. The hasher is `FxHash` rather
    /// than the default, the keys being the index's own numbering and not
    /// anything a stranger chooses.
    #[allow(clippy::too_many_arguments)]
    fn search<C: Metric>(
        &self,
        start: Node,
        end: Node,
        goal: [f64; 3],
        range: f64,
        drive: Drive,
        how: Routing,
        step: impl Fn(&JumpGraph, Node, f64) -> C,
        estimate: impl Fn([f64; 3]) -> C,
        sampled: &mut Option<Sampler>,
    ) -> Option<Vec<Node>> {
        if start == end {
            return Some(vec![start]);
        }
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
        let mut best: FxHashMap<Node, C> = FxHashMap::default();
        let mut came: FxHashMap<Node, Node> = FxHashMap::default();
        // Nearest the goal is the smallest number and a heap pops the
        // greatest, so the whole key is reversed: the ordering is "cheapest
        // first, and of those the one nearest the goal".
        let mut open = BinaryHeap::new();
        let mut near: Vec<(Node, [f64; 3], f64)> = Vec::new();
        // Read once, here, so every setting's promise is kept by the one
        // place that could break it. See [`Routing::fanout`].
        let fanout = how.fanout();

        // Squared, and as an integer: the key is only ever compared, and
        // squaring is monotone over distances that are never negative, so
        // the ordering is the same one a square root would give and the
        // root itself is forty million calls nobody reads.
        let away = |at: [f64; 3]| dist2(at, goal) as u64;

        let from = self.place(start);
        best.insert(start, C::ZERO);
        open.push(Reverse((estimate(from), away(from), C::ZERO, start)));

        while let Some(Reverse((_, _, was, node))) = open.pop() {
            // A system can be pushed more than once, a cheaper way to it
            // having been found after the first; the dearer entries are still
            // in the heap and are nothing to expand again.
            if best.get(&node).is_some_and(|held| was > *held) {
                continue;
            }
            let at = self.place(node);
            if node == end {
                let mut path = vec![end];
                while let Some(&before) = came.get(path.last().expect("a step"))
                {
                    path.push(before);
                }
                path.reverse();
                return Some(path);
            }
            if let Some(sampled) = sampled.as_mut() {
                sampled.expanded(
                    node,
                    DVec3::from(at),
                    DVec3::from(goal),
                    &came,
                    |node| DVec3::from(self.place(node)),
                );
            }
            self.neighbors(node, at, range, drive, goal, fanout, &mut near);

            for &(next, place, leg) in &near {
                let cost = was + step(self, node, leg);
                if best.get(&next).is_none_or(|held| cost < *held) {
                    best.insert(next, cost);
                    came.insert(next, node);
                    open.push(Reverse((
                        cost + estimate(place),
                        away(place),
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
    use crate::testing::{Scratch, sky_apart, sky_of};
    use galos_index::CellId;
    use galos_index::meta::NameEntry;
    use std::collections::HashMap;

    /// A system named for its address, at `at`.
    fn at(address: i64, at: [f32; 3]) -> NameEntry {
        NameEntry { address, name: format!("S{address}").into(), position: at }
    }

    /// A route's end, as the router takes one
    ///
    /// The address and where the names table says it sits: the pair
    /// [`JumpGraph::route`] wants, because the record itself is found by
    /// descending to that place ([`galos_index::Sky::node_of`]). A test
    /// knows both, having put the system there.
    fn end(entries: &[NameEntry], address: i64) -> (i64, [f64; 3]) {
        let found = entries
            .iter()
            .find(|entry| entry.address == address)
            .expect("a system the test placed");
        (
            found.address,
            [
                found.position[0] as f64,
                found.position[1] as f64,
                found.position[2] as f64,
            ],
        )
    }

    /// A built galaxy holding `entries`, and the directory it lives in
    ///
    /// The directory comes back with it and must be held for as long as the
    /// galaxy is read: the places are the cell payloads, mapped where they
    /// lie, so a route over a directory that has been removed is a route
    /// over nothing.
    fn galaxy(what: &str, entries: &[NameEntry]) -> (Scratch, Arc<Sky>) {
        let dir = Scratch::new(what);
        let sky = sky_of(dir.path(), entries);
        (dir, sky)
    }

    /// How far a route runs, following its legs.
    fn run(path: &[(i64, [f64; 3])]) -> f64 {
        path.windows(2).map(|w| dist2(w[0].1, w[1].1).sqrt()).sum()
    }

    /// Both settings, since every claim below holds of both.
    const BOTH: [Routing; 2] = [Routing::Direct, Routing::Shortest];

    /// A range small enough to make the estimate overflow is answered, not
    /// panicked on
    ///
    /// The form takes any range over nothing, and a small enough one puts more
    /// jumps between two systems than a `u32` holds: the cast pins at the top
    /// and scaling it by [`LEANING`] wrapped — a panic on the compute pool in
    /// a debug build. It fires on the first push, before a neighbour is looked
    /// at, so having no reachable neighbours is no protection.
    #[test]
    fn a_range_too_small_to_estimate_does_not_overflow() {
        let entries = vec![at(0, [0., 0., 0.]), at(1, [100., 0., 0.])];
        let (_dir, sky) = galaxy("overflow", &entries);
        let graph = JumpGraph::over(&sky, &Boosts::default());

        assert!(
            graph
                .route(
                    end(&entries, 0),
                    end(&entries, 1),
                    1e-9,
                    Routing::Quick,
                    Drive::Standard,
                    None,
                )
                .is_none(),
            "nothing is reachable at that range"
        );
    }

    /// A search with no corridor to walk is drawn coarser rather than without
    /// bound
    ///
    /// The cell of the closed set is sized off how far there is to go, on the
    /// argument that the cells touched are the corridor searched. A leg with
    /// no route expands in every direction instead, and the layer grew with
    /// the search: every cell copied out under the lock each frame and turned
    /// into four vertices. Held at [`super::frontier::CELL_CEILING`] now, by
    /// widening the cell.
    #[test]
    fn a_search_that_spreads_is_held_at_the_cell_ceiling() {
        let goal = DVec3::new(100., 0., 0.);
        let frontier = Frontier::between(DVec3::ZERO, goal);
        let first = frontier.drawn().expect("a frontier").across;
        let mut sampler = frontier.sampler();
        // One node, reached from nowhere: the sampler hashes what it is
        // given and asks the closure where it sits, so a search's worth of
        // expansions needs no galaxy behind it.
        let node = Node {
            cell: CellId { level: 13, x: 4096, y: 4096, z: 4096 },
            at: 0,
        };
        let came: FxHashMap<Node, Node> = FxHashMap::default();

        // Expansions marching away in a straight line, one cell apiece: what
        // a search with nowhere to go looks like to the sampler.
        let ceiling = super::super::frontier::CELL_CEILING;
        let stride = super::super::frontier::STRIDE;
        let step = first * 1.5;
        for n in 0..(ceiling as u64 * 4 * stride) {
            let at = DVec3::new(0., 0., (n / stride) as f64 * step);
            sampler.expanded(node, at, goal, &came, |_| DVec3::ZERO);
        }
        sampler.done();

        let drawn = frontier.drawn().expect("a frontier");
        assert!(
            drawn.cells.len() <= ceiling,
            "the closed set held {} cells, over the ceiling of {ceiling}",
            drawn.cells.len()
        );
        assert!(
            drawn.across > first,
            "the cell never widened: still {} light years",
            drawn.across
        );
    }

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
        let (_dir, sky) = galaxy("straighter", &entries);
        let graph = JumpGraph::over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    None,
                )
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
        let (_dir, sky) = galaxy("fewest", &entries);
        let graph = JumpGraph::over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    None,
                )
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
        let boosts = Boosts::holding(HashMap::from([(15, Boost::Neutron)]));
        let (_dir, sky) = galaxy("quick", &entries);
        let graph = JumpGraph::over(&sky, &boosts);
        let drive = Drive::Standard;
        let (start, goal) = (end(&entries, 0), end(&entries, 999));

        let fewest = graph
            .route(start, goal, 100., Routing::Direct, drive, None)
            .expect("a route")
            .len()
            - 1;
        let quick = graph
            .route(start, goal, 100., Routing::Quick, drive, None)
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
            // the only thing that says how far a jump out of it may go. Its
            // record is found from the place the route itself came back with,
            // that being where the payload has it.
            let reach = 100.
                * drive.factor(
                    graph
                        .node_of(leg[0].0, leg[0].1)
                        .and_then(|node| graph.boost(node)),
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
        let (_dir, sky) = galaxy("proves", &entries);
        let graph = JumpGraph::over(&sky, &Boosts::default());

        for how in BOTH {
            let path = graph
                .route(
                    end(&entries, 0),
                    end(&entries, 9),
                    500.,
                    how,
                    Drive::Unaided,
                    None,
                )
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
        let (_dir, sky) = galaxy("gap", &entries);
        let graph = JumpGraph::over(&sky, &Boosts::default());
        let (start, goal) = (end(&entries, 0), end(&entries, 1));

        for how in BOTH {
            assert!(
                graph
                    .route(start, goal, 500., how, Drive::Unaided, None)
                    .is_none(),
                "{how:?} jumped 600 at 500"
            );
            assert!(
                graph
                    .route(start, goal, 700., how, Drive::Unaided, None)
                    .is_some(),
                "{how:?} refused 600 inside 700"
            );
        }
    }

    /// A neighbour search reads cells other than the one the jump leaves,
    /// and offers each system once
    ///
    /// The places are the cell payloads now, so what a jump can reach has
    /// nothing to do with where a cell boundary fell: a search bounded by
    /// the payload it starts in would only ever step to whatever happens to
    /// share that payload, which is a route through one cell of the galaxy
    /// and nothing else. The whole of the neighbour query is the sphere.
    ///
    /// Cut to a system a cell so there is something to cross ([`sky_apart`]:
    /// the build divides on count, so a handful of systems under the
    /// published caps is one root payload however far apart they lie), and
    /// the two systems 384 light years apart land the two ways they can. One
    /// is in an *internal* node's slice and the other below it — a cell
    /// keeps the brightest of what fell in it and pushes the rest down, so a
    /// system in an internal node is ordinary and not an edge case — and the
    /// third is off in a sibling subtree the search's own cell does not
    /// contain at all.
    ///
    /// Once each, and never itself. A system is in exactly one cell's
    /// payload, so a doubled neighbour would mean a cell of the descent had
    /// been scanned twice; and a system offered as its own neighbour is a
    /// jump of no distance, which every setting would take for free forever.
    #[test]
    fn a_neighbour_search_reads_past_the_cell_it_starts_in() {
        // Wider than any jump, and the same either side, so the one range
        // reaches both of the systems it should and the boundary is one
        // number.
        let far = 384.;
        let here = [0., 0., 0.];
        let dir = Scratch::new("across-cells");
        // The brightest is the system the root keeps, and it is parked far
        // enough off that no range below reaches it: what it is for is to
        // leave the three the assertions are about under the root rather
        // than in it.
        let sky = sky_apart(
            dir.path(),
            &[
                (0, [-5_000., 0., 0.]),
                (1, here),
                (2, [far, 0., 0.]),
                (3, [-far, 0., 0.]),
            ],
        );
        let graph = JumpGraph::over(&sky, &Boosts::default());
        let from = graph.node_of(1, here).expect("the system it maps");
        for address in [2, 3] {
            let cell = graph
                .node_of(address, [0., 0., 0.])
                .expect("the others too")
                .cell;
            assert_ne!(
                from.cell, cell,
                "S{address} shares a payload, so there is nothing to cross"
            );
        }

        let found = |range: f64| {
            let mut out = Vec::new();
            graph.neighbors(
                from,
                here,
                range,
                Drive::Unaided,
                [far, 0., 0.],
                // Uncapped: what this pins is which systems are in reach,
                // not which of them a setting would bother to relax.
                None,
                &mut out,
            );
            let mut addresses: Vec<i64> =
                out.iter().map(|&(node, ..)| graph.address(node)).collect();
            addresses.sort();
            addresses
        };

        assert_eq!(
            found(far + 1.),
            vec![2, 3],
            "the systems a cell away, each once and itself never"
        );
        assert!(found(far - 1.).is_empty(), "and nothing inside that");
    }

    /// Only the setting that has not promised the fewest jumps thins an
    /// expansion.
    ///
    /// The cap is the fix for a search whose work is quadratic in stellar
    /// density, and it is also the one thing here that can lose the right
    /// answer: the neighbour a fewest-jumps chain went through may be the
    /// one thinned away. So [`Routing::Direct`] and [`Routing::Shortest`],
    /// which both claim the fewest, must carry no cap at all — and a
    /// setting added later must decide which it is rather than inherit
    /// whatever the match arm above it said.
    #[test]
    fn only_a_quick_route_thins_an_expansion() {
        assert_eq!(Routing::Quick.fanout(), Some(FANOUT));
        for how in BOTH {
            assert_eq!(
                how.fanout(),
                None,
                "{how:?} promises the fewest jumps and may not thin",
            );
        }
    }

    /// A cap keeps the neighbours that get nearest the goal, and uncapped
    /// keeps them all.
    ///
    /// Which is the whole of what the cap does, and what makes it a
    /// defensible approximation rather than an arbitrary one: what it drops
    /// is the far side of the sphere, away from where the route is going.
    #[test]
    fn a_cap_keeps_the_neighbours_nearest_the_goal() {
        let here = [0., 0., 0.];
        let goal = [1_000., 0., 0.];
        // Nine systems in a line, the goal off one end, so which of them is
        // nearest it is unambiguous and the order is the line's own.
        let places: Vec<(i64, [f64; 3])> =
            (1..=9).map(|n| (n, [n as f64 * 10., 0., 0.])).collect();
        let dir = Scratch::new("capped");
        let sky = crate::testing::sky(dir.path(), &places);
        let graph = JumpGraph::over(&sky, &Boosts::default());
        let from = graph.node_of(1, places[0].1).expect("the first system");

        let reached = |cap: Option<usize>| {
            let mut out = Vec::new();
            graph.neighbors(
                from,
                here,
                500.,
                Drive::Unaided,
                goal,
                cap,
                &mut out,
            );
            let mut found: Vec<i64> =
                out.iter().map(|&(node, ..)| graph.address(node)).collect();
            found.sort();
            found
        };

        // Every other system is in range, and itself is never among them.
        assert_eq!(reached(None), vec![2, 3, 4, 5, 6, 7, 8, 9]);
        // Capped, the ones kept are the far end of the line: nearest the
        // goal, which sits past S9.
        assert_eq!(reached(Some(3)), vec![7, 8, 9]);
        assert_eq!(reached(Some(1)), vec![9]);
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
        let boosts = Boosts::holding(HashMap::from([(2, Boost::Neutron)]));
        let (_dir, sky) = galaxy("neutron", &entries);
        let graph = JumpGraph::over(&sky, &boosts);
        let (start, goal) = (end(&entries, 1), end(&entries, 9));

        for how in BOTH {
            assert!(
                graph
                    .route(start, goal, 100., how, Drive::Unaided, None)
                    .is_none(),
                "{how:?} crossed 350 ly at a 100 ly range unaided"
            );

            let path = graph
                .route(start, goal, 100., how, Drive::Standard, None)
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
        let boosts = Boosts::holding(HashMap::from([(9, Boost::Neutron)]));
        let (_dir, sky) = galaxy("leaves", &entries);
        let graph = JumpGraph::over(&sky, &boosts);

        for how in BOTH {
            assert!(
                graph
                    .route(
                        end(&entries, 1),
                        end(&entries, 9),
                        100.,
                        how,
                        Drive::Standard,
                        None,
                    )
                    .is_none(),
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
        let boosts = Boosts::holding(HashMap::from([(1, Boost::WhiteDwarf)]));
        let (_dir, sky) = galaxy("dwarf", &entries);
        let graph = JumpGraph::over(&sky, &boosts);
        let (start, goal) = (end(&entries, 1), end(&entries, 9));

        for how in BOTH {
            assert!(
                graph
                    .route(start, goal, 100., how, Drive::Standard, None)
                    .is_none(),
                "{how:?} made 250 ly of a 150 ly boosted jump"
            );
            assert!(
                graph
                    .route(start, goal, 100., how, Drive::Optimised, None)
                    .is_some(),
                "{how:?} refused 250 ly of a 300 ly boosted jump"
            );
        }
    }
}
