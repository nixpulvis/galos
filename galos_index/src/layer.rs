//! Two indices served as one, either of them turned off without a restart.
//!
//! The galaxy the map draws comes from EDDN: everyone else's game, forwarded,
//! written to Postgres and baked into a directory by `galos-sync db`. A
//! commander's own journal is the other half of the same sky and never
//! reaches that directory — the systems nobody has reported, the bodies only
//! this commander has scanned, the arrival that happened thirty seconds ago.
//! `galos_journal` turns that directory of `.log` files into the same
//! vocabulary, and this is where the two meet.
//!
//! **They meet here and not on disk.** Baking a commander's journal into the
//! published index would put private, unshared readings into the artefact
//! `galos-sync db` owns and rewrites: the next full build would drop them,
//! a `--only` pass would drop half of them, and there would be no way left to
//! ask what the galaxy looks like without them. So the two directories stay
//! separate, whole, and independently rebuildable, and the join is made in
//! the reader, per call, over whatever each side says at that moment. Which
//! is also what makes the toggle possible at all: a merge is undone by
//! answering the next call differently, where a bake would have to be
//! rebuilt.
//!
//! ## How the two compose
//!
//! Metadata is keyed by address and the overlay wins: a name, a reach, a
//! supercharge, a political column, the bodies inside a system. That is the
//! whole point of reading a journal — for a system EDDN already has, what
//! this commander scanned is the better reading of it, and it is theirs.
//!
//! The cell tree is the part that has to be *exact*, and it composes only
//! because the two sides describe disjoint sets of systems. An
//! [`Aggregate`](crate::Aggregate) merges exactly and a cell's rank range is
//! "how many of my subtree my ancestors claimed", so two trees over disjoint
//! sets add cell by cell: the totals come out as the union's — same count,
//! same flux, same `m_min` — and every system is owned by exactly one cell's
//! slice, which is what the walk draws by.
//!
//! What does not add up is depth. Where one side refined a region into
//! children and the other held it whole in a leaf, the leaf's systems are in
//! none of those children's aggregates, because nothing outside a built tree
//! can share them out among cells that side never raised. A journal's few
//! thousand systems against EDDN's millions makes that the usual case deep
//! in the tree, and what it costs is a deep cell's aggregate standing for
//! one side alone with the rest counted a level up — a little glow in the
//! wrong cell, never a system drawn twice or missed.
//!
//! Over sets that *overlap* it adds a system to the sky twice, which is what
//! [`Claimed`] is for: the overlay is told which addresses the layer below
//! already carries and leaves those systems out of its own tree, keeping
//! every one of them in its metadata. Nothing here can do that job on the
//! overlay's behalf — a system cannot be taken back out of a built tree from
//! outside it — so an overlay that ignores its [`Claimed`] double-counts
//! whatever both sides hold, and the error is a commander's own systems
//! counted twice in cells that are otherwise right.
//!
//! ## How a client finds out
//!
//! Through [`Source::stamp`], as it always did. A layered stamp folds both
//! sides' stamps and the toggle's state into one opaque number, so a journal
//! that has just read an arrival and a toggle that has just been flipped both
//! read as "this part has changed" to a client that knows nothing about
//! layers. The whole of the toggle's effect on a running map is that every
//! part it holds stamps differently once and is read again.

use crate::aggregate::Cell;
use crate::cache::Point;
use crate::geometry::CellId;
use crate::meta::{
    Faction, NameEntry, PopulatedSystem, SystemBodies, SystemBoost, SystemReach,
};
use crate::source::{Part, Source, Stamp};
use crate::walk::Index;
use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

/// However many name chunks a table could conceivably be published in.
///
/// The chunk numbering is the names table's whole layout — numbered from
/// zero, no gaps, read until one is missing — and [`Layered`] has to know
/// where the base's numbering ends before it can append its own chunk after
/// it. It asks by stamping upwards, which needs a stop: a base answering
/// `Some` forever would otherwise be walked forever. At the published chunk
/// size this is several billion systems, so reaching it means the transport
/// is lying rather than that the galaxy grew.
const CHUNK_CEILING: usize = 100_000;

/// Whether a layer is being drawn, shared with whatever flips it.
///
/// An [`AtomicBool`] and not a message, because the question is asked on the
/// read path and answered on a keystroke: a client toggling this wants the
/// next read to see it and wants no channel to drain, and the read wants no
/// lock. Cloneable, so the UI and the transport hold the one flag.
#[derive(Clone, Debug)]
pub struct Toggle(Arc<AtomicBool>);

impl Toggle {
    /// A toggle standing at `on`.
    pub fn new(on: bool) -> Toggle {
        Toggle(Arc::new(AtomicBool::new(on)))
    }

    /// Whether the layer is being drawn.
    pub fn on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Draw the layer, or stop.
    pub fn set(&self, on: bool) {
        self.0.store(on, Ordering::Relaxed);
    }

    /// Flip it, answering what it now stands at.
    pub fn flip(&self) -> bool {
        let now = !self.on();
        self.set(now);
        now
    }
}

impl Default for Toggle {
    /// On, which is what naming a layer at all asks for.
    fn default() -> Toggle {
        Toggle::new(true)
    }
}

/// Which systems the layer below already carries, so the one above can leave
/// them out of its tree.
///
/// The disjointness the cell arithmetic in [`Layered`] rests on, and the one
/// thing an overlay cannot work out for itself: it knows what it holds and
/// nothing about what it is being served over. So it is told, by whoever has
/// both — in the map that is [`crate::NameEntry`]'s table, which is read at
/// startup and answers exactly this question for two and a half million
/// systems with no second copy of anything.
///
/// A predicate rather than a set for that reason. The map hands over a
/// closure onto the table it already holds, and a caller that has only a
/// set of addresses hands over one that asks it.
///
/// Empty until it is set, which is the honest starting state: a client that
/// has not read the base's names yet does not know what the base carries, and
/// an overlay that assumed the worst would show none of its systems. What an
/// unset claim costs is the double count described in the module header,
/// until the client fills it in.
#[derive(Clone, Default)]
pub struct Claimed {
    of: Arc<RwLock<Option<Arc<dyn Fn(i64) -> bool + Send + Sync>>>>,
    /// Bumped on every [`set`](Self::set), so a holder can tell that the
    /// answer it built its tree against is no longer the current one.
    generation: Arc<AtomicU64>,
}

impl Claimed {
    /// Nothing claimed, which is a layer served over nothing.
    pub fn none() -> Claimed {
        Claimed::default()
    }

    /// A claim answered by `of`.
    pub fn by(of: impl Fn(i64) -> bool + Send + Sync + 'static) -> Claimed {
        let claimed = Claimed::none();
        claimed.set(of);
        claimed
    }

    /// Answer the claim with `of` from now on.
    pub fn set(&self, of: impl Fn(i64) -> bool + Send + Sync + 'static) {
        // Poisoning says a claim panicked while it was being read, which says
        // nothing about whether this one can be written.
        let mut held = self.of.write().unwrap_or_else(|it| it.into_inner());
        *held = Some(Arc::new(of));
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the layer below carries the system at `address`.
    pub fn holds(&self, address: i64) -> bool {
        let held = self.of.read().unwrap_or_else(|it| it.into_inner());
        held.as_ref().is_some_and(|of| of(address))
    }

    /// How many times the claim has been answered differently
    ///
    /// What an overlay rebuilds against: it built its tree under one claim,
    /// and a claim set since is a tree that leaves out the wrong systems.
    /// Starts at zero, which is "never answered", and no claim has that.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for Claimed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Claimed")
            .field("generation", &self.generation())
            .finish()
    }
}

/// One index served over another, the upper one turned off without a restart.
///
/// `base` is the published galaxy and is always served. `overlay` is served
/// over it while its [`Toggle`] is on and is not read at all while it is off,
/// which is the whole of what "off" means: not a filter over what was read,
/// but a transport that is not asked.
pub struct Layered {
    base: Arc<dyn Source>,
    overlay: Arc<dyn Source>,
    on: Toggle,
    /// Where the base's names numbering last ended, so the common case costs
    /// one stamp rather than a walk. See [`Layered::base_chunks`].
    chunks: AtomicUsize,
}

impl Layered {
    /// `overlay` over `base`, drawn while `on`.
    pub fn new(
        base: Arc<dyn Source>,
        overlay: Arc<dyn Source>,
        on: Toggle,
    ) -> Layered {
        Layered { base, overlay, on, chunks: AtomicUsize::new(0) }
    }

    /// How many chunks the base's names table runs to, at or past `chunk`
    ///
    /// The layout's own contract read back: numbered from zero with no gaps,
    /// so the first number the base has nothing for is the end of it.
    ///
    /// Remembered, because a client walks the numbering from zero on every
    /// poll and asks about each number in it, so working the end out afresh
    /// for each is the walk squared — thirty-odd chunks over a published
    /// galaxy, a thousand stats a poll to answer what one load answers.
    ///
    /// Remembered is not trusted, though. A table grows a chunk and it also
    /// loses one: [`crate::NameTable::remove`] withdrawing the last entries
    /// of the tail chunk unlinks the file, and an end remembered from before
    /// that files the overlay past a number nothing serves, which is a
    /// client's walk stopping before it and the overlay's names gone. So the
    /// remembered end is confirmed by the chunk below it before it is
    /// answered from — one stamp rather than the walk — and a boundary that
    /// no longer stands is walked for again from zero.
    async fn base_chunks(&self, chunk: usize) -> usize {
        let held = self.chunks.load(Ordering::Relaxed);
        let standing = held > 0
            && matches!(
                self.base.stamp(Part::NamesChunk(held - 1)).await,
                Ok(Some(_))
            );
        if standing && chunk < held {
            return held;
        }

        let mut end = if standing { held } else { 0 };
        let mut certain = true;
        while end < CHUNK_CEILING {
            match self.base.stamp(Part::NamesChunk(end)).await {
                Ok(Some(_)) => end += 1,
                Ok(None) => break,
                // A base that cannot say is not a base that has ended. This
                // pass has to answer something and answers what it reached,
                // but an end an error stopped short of was never established
                // and is not worth coming back to: remembering it files the
                // overlay over a chunk that is being renamed into place.
                Err(_) => {
                    certain = false;
                    break;
                }
            }
        }
        if certain {
            self.chunks.store(end, Ordering::Relaxed);
        }
        end
    }
}

/// Two stamps and a toggle folded into one.
///
/// [`Stamp`] is opaque by contract — "the same means unchanged" and nothing
/// else — so a layered part can stamp as a hash of what it was composed from
/// without telling a client anything it is not allowed to read. The toggle is
/// in the hash because flipping it is a republish of everything: every part
/// the client holds was composed one way and is now composed the other.
///
/// The hash is [`DefaultHasher`], whose values a client may not carry across
/// sessions — and does not: a stamp is taken before a part is read and
/// compared against the next reading of the same part in the same process.
fn folded(on: bool, base: Option<Stamp>, overlay: Option<Stamp>) -> Stamp {
    let mut hasher = DefaultHasher::new();
    on.hash(&mut hasher);
    base.hash(&mut hasher);
    overlay.hash(&mut hasher);
    hasher.finish()
}

/// Two tables keyed the same way, the overlay's row winning.
///
/// One pass over each, which is what keeps a hundred-megabyte base table from
/// being hashed row by row against a handful of overlay rows: the overlay is
/// keyed and the base is filtered against it.
fn over<T, K: std::hash::Hash + Eq>(
    base: Vec<T>,
    overlay: Vec<T>,
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    if overlay.is_empty() {
        return base;
    }
    let replaced: HashSet<K> = overlay.iter().map(&key).collect();
    let mut out: Vec<T> =
        base.into_iter().filter(|it| !replaced.contains(&key(it))).collect();
    out.extend(overlay);
    out
}

#[async_trait]
impl Source for Layered {
    /// The two trees added cell by cell.
    ///
    /// A cell either side holds alone is carried through as it stands. A cell
    /// both hold is the union of what each says about it: the aggregates
    /// merge, the child masks are the cells that exist in either, and the
    /// rank range is both ancestors' claims and both slices. Every system
    /// ends up owned by exactly one slice where the two sets are disjoint,
    /// which is [`Claimed`]'s job to arrange; a cell one side stopped at and
    /// the other refined past keeps the stopped side's systems above the
    /// children rather than in them, as the module header describes.
    async fn index(&self) -> io::Result<Index> {
        let base = self.base.index().await?;
        if !self.on.on() {
            return Ok(base);
        }
        let overlay = self.overlay.index().await?;
        // Nothing to add. Told by the count and not by the cell set: a build
        // over an empty set still raises a root, so an overlay standing over
        // no systems is a one-cell index rather than no index at all.
        if overlay.root().is_none_or(|root| root.aggregate.count() == 0) {
            return Ok(base);
        }

        let mut cells: HashMap<CellId, Cell> =
            base.cells().map(|cell| (cell.id, *cell)).collect();
        for above in overlay.cells() {
            cells
                .entry(above.id)
                .and_modify(|below| {
                    let rank_lo = below.rank_lo + above.rank_lo;
                    let slice = below.slice_len() + above.slice_len();
                    *below = Cell {
                        id: below.id,
                        rank_lo,
                        rank_hi: rank_lo + slice,
                        child_mask: below.child_mask | above.child_mask,
                        aggregate: below.aggregate.merge(above.aggregate),
                    };
                })
                .or_insert(*above);
        }
        Ok(Index::from_cells(cells.into_values()))
    }

    /// One cell's systems from both sides, brightest first.
    ///
    /// Ordered rather than concatenated: the walk draws a prefix of a cell's
    /// payload and calls it the resolvable part, which is only the brightest
    /// systems if the payload is in magnitude order. Each side hands its own
    /// slice over already ordered and a join of two ordered runs is not one.
    ///
    /// Deduplicated by address with the overlay's record winning, which is
    /// belt and braces: a claimed system is not in the overlay's tree to
    /// begin with, and where a claim has not been answered yet this at least
    /// keeps one system from being drawn twice at the same point.
    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>> {
        let base = self.base.payload(id).await?;
        if !self.on.on() {
            return Ok(base);
        }
        let overlay = self.overlay.payload(id).await?;
        if overlay.is_empty() {
            return Ok(base);
        }
        let mut points = over(base, overlay, |point| point.id64);
        points.sort_by(|a, b| {
            a.magnitude.total_cmp(&b.magnitude).then(a.id64.cmp(&b.id64))
        });
        Ok(points)
    }

    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
        let base = self.base.populated().await?;
        if !self.on.on() {
            return Ok(base);
        }
        Ok(over(base, self.overlay.populated().await?, |it| it.address))
    }

    async fn names(&self) -> io::Result<Vec<NameEntry>> {
        let base = self.base.names().await?;
        if !self.on.on() {
            return Ok(base);
        }
        Ok(over(base, self.overlay.names().await?, |it| it.address))
    }

    /// The base's chunks, and the overlay's whole table as the one after them.
    ///
    /// The overlay is a commander's own journal — thousands of systems, not
    /// millions — so it is one chunk however long it runs, and putting it
    /// past the end of the base's numbering keeps the base's chunks answering
    /// exactly what they always did. A client walking the numbering from zero
    /// finds the base's table and then this, which is the contract it reads
    /// by: numbered from zero, no gaps, ends at the first that is missing.
    ///
    /// The base growing a chunk moves this one along by one. The client
    /// re-reads both — the number it held now answers with the base's new
    /// chunk, and the number past it is a chunk it has never seen — and what
    /// it already took from the overlay it keeps, entries being merged by
    /// address rather than by where they were read.
    async fn names_chunk(&self, chunk: usize) -> io::Result<Vec<NameEntry>> {
        if !self.on.on() {
            return self.base.names_chunk(chunk).await;
        }
        let chunks = self.base_chunks(chunk).await;
        if chunk < chunks {
            self.base.names_chunk(chunk).await
        } else if chunk == chunks {
            self.overlay.names().await
        } else {
            Ok(Vec::new())
        }
    }

    async fn factions(&self) -> io::Result<Vec<Faction>> {
        let base = self.base.factions().await?;
        if !self.on.on() {
            return Ok(base);
        }
        Ok(over(base, self.overlay.factions().await?, |it| it.id))
    }

    async fn reaches(&self) -> io::Result<Vec<SystemReach>> {
        let base = self.base.reaches().await?;
        if !self.on.on() {
            return Ok(base);
        }
        Ok(over(base, self.overlay.reaches().await?, |it| it.address))
    }

    /// Whether either side publishes a supercharge table, and what is in it.
    ///
    /// [`None`] only where neither does. A journal knows a jet cone when its
    /// commander has scanned one, so an overlay with a table over a base
    /// without one can answer for the systems it has been to and nothing
    /// else — which is a worse route than a full table gives and a better
    /// one than the refusal an absent table earns.
    async fn boosts(&self) -> io::Result<Option<Vec<SystemBoost>>> {
        let base = self.base.boosts().await?;
        if !self.on.on() {
            return Ok(base);
        }
        let overlay = self.overlay.boosts().await?;
        match (base, overlay) {
            (None, None) => Ok(None),
            (base, overlay) => Ok(Some(over(
                base.unwrap_or_default(),
                overlay.unwrap_or_default(),
                |it| it.address,
            ))),
        }
    }

    /// What is inside a system, the overlay's reading of each thing winning.
    ///
    /// Merged per body rather than taken whole from one side. A commander has
    /// scanned some of a system and EDDN has heard about some of it, and
    /// neither set contains the other: the union is the system as it is best
    /// known, and where both describe the same body the one this commander
    /// scanned is the one they can check out of the window.
    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        let base = self.base.bodies(address).await?;
        if !self.on.on() {
            return Ok(base);
        }
        let overlay = self.overlay.bodies(address).await?;
        Ok(SystemBodies {
            stars: over(base.stars, overlay.stars, |it| it.id),
            bodies: over(base.bodies, overlay.bodies, |it| it.id),
            barycenters: over(base.barycenters, overlay.barycenters, |it| {
                it.id
            }),
        })
    }

    /// What the part is now, on both sides and under the toggle as it stands.
    ///
    /// [`None`] keeps meaning "not there", so a part neither side has is
    /// still nothing to hold a stamp for. A part one side has stamps as
    /// present, which it is: the composition serves it.
    ///
    /// With the overlay off the base's own stamp is not passed through
    /// unchanged, but folded with the toggle. Passing it through would be a
    /// stamp that does not move when the layer is turned on, and the map
    /// would go on drawing the sky it read before the toggle for as long as
    /// the base stayed quiet.
    async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
        let on = self.on.on();
        if !on {
            return Ok(self
                .base
                .stamp(part)
                .await?
                .map(|base| folded(false, Some(base), None)));
        }

        // The names table is the one part whose numbering the composition
        // rewrites, so it is stamped by what actually answers it rather than
        // by both sides at that number.
        if let Part::NamesChunk(chunk) = part {
            let chunks = self.base_chunks(chunk).await;
            return if chunk < chunks {
                Ok(self
                    .base
                    .stamp(part)
                    .await?
                    .map(|base| folded(true, Some(base), None)))
            } else if chunk == chunks {
                // The overlay's table is served whole as one chunk, so what
                // moves it is the overlay moving at all. Its index stamp is
                // that: a journal source stamps every part it serves by the
                // generation it is on. Present whatever that stamp says,
                // because `names_chunk` serves this number either way, and a
                // client walking by stamps would otherwise stop one short of
                // a chunk it can read.
                let over = self.overlay.stamp(Part::Index).await?;
                Ok(Some(folded(true, None, over)))
            } else {
                Ok(None)
            };
        }

        let base = self.base.stamp(part).await?;
        let overlay = self.overlay.stamp(part).await?;
        Ok(match (base, overlay) {
            (None, None) => None,
            (base, overlay) => Some(folded(true, base, overlay)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Boost;
    use crate::tree::{BuildParams, Snapshot, System};

    /// The cuts every fixture here is built under.
    ///
    /// Far below the published ones: [`BuildParams::default`] holds four
    /// thousand systems in a leaf, so a fixture of a few hundred would be a
    /// single root cell and the whole of what this module tests — two cells
    /// at the same address merged, their `rank_lo` added, their child masks
    /// unioned, a cell one side holds alone carried through — would never
    /// run. Cut this small the same few hundred raise a tree several levels
    /// deep, which is what the arithmetic is arithmetic over. `tree.rs`'s
    /// own stress tests cut the same way for the same reason.
    const CUTS: BuildParams = BuildParams { internal_slice: 8, leaf_cap: 32 };

    /// A source over a tree and a set of tables held in memory.
    ///
    /// Enough of one to compose: what [`Layered`] does is arithmetic over
    /// what two sources say, and a real transport would only put a filesystem
    /// between this and that arithmetic. The stamps are a number the test
    /// sets, which is all a stamp is by contract.
    #[derive(Default)]
    struct Held {
        built: Snapshot,
        names: Vec<NameEntry>,
        reaches: Vec<SystemReach>,
        populated: Vec<PopulatedSystem>,
        boosts: Option<Vec<SystemBoost>>,
        factions: Vec<Faction>,
        bodies: HashMap<i64, SystemBodies>,
        /// What every part this holds stamps as, or [`None`] to hold nothing.
        stamp: Option<Stamp>,
        /// How many chunks the names table is served in, one by default.
        ///
        /// Shared and settable, so a test can publish a chunk or withdraw
        /// one under a [`Layered`] that has already read the numbering.
        chunks: Arc<AtomicUsize>,
    }

    impl Held {
        fn over(systems: &[System]) -> Held {
            Held {
                built: Snapshot::build(systems, &CUTS),
                names: systems
                    .iter()
                    .map(|it| NameEntry {
                        address: it.id64 as i64,
                        name: format!("System {}", it.id64),
                        position: [
                            it.position[0] as f32,
                            it.position[1] as f32,
                            it.position[2] as f32,
                        ],
                    })
                    .collect(),
                stamp: Some(1),
                chunks: Arc::new(AtomicUsize::new(1)),
                ..Held::default()
            }
        }

        fn arc(self) -> Arc<dyn Source> {
            Arc::new(self)
        }
    }

    #[async_trait]
    impl Source for Held {
        async fn index(&self) -> io::Result<Index> {
            Ok(self.built.index.clone())
        }
        async fn payload(&self, id: CellId) -> io::Result<Vec<Point>> {
            Ok(self.built.payload(id).to_vec())
        }
        async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
            Ok(self.populated.clone())
        }
        async fn names(&self) -> io::Result<Vec<NameEntry>> {
            Ok(self.names.clone())
        }
        /// The table cut into `chunks` even pieces, so a test can put a base
        /// with more than one chunk under an overlay.
        async fn names_chunk(
            &self,
            chunk: usize,
        ) -> io::Result<Vec<NameEntry>> {
            let chunks = self.chunks.load(Ordering::Relaxed);
            if chunk >= chunks {
                return Ok(Vec::new());
            }
            let each = self.names.len().div_ceil(chunks.max(1));
            Ok(self
                .names
                .iter()
                .skip(chunk * each)
                .take(each)
                .cloned()
                .collect())
        }
        async fn factions(&self) -> io::Result<Vec<Faction>> {
            Ok(self.factions.clone())
        }
        async fn reaches(&self) -> io::Result<Vec<SystemReach>> {
            Ok(self.reaches.clone())
        }
        async fn boosts(&self) -> io::Result<Option<Vec<SystemBoost>>> {
            Ok(self.boosts.clone())
        }
        async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
            Ok(self.bodies.get(&address).cloned().unwrap_or_default())
        }
        async fn stamp(&self, part: Part) -> io::Result<Option<Stamp>> {
            Ok(match part {
                Part::NamesChunk(chunk)
                    if chunk >= self.chunks.load(Ordering::Relaxed) =>
                {
                    None
                }
                Part::Cell(id) if self.built.payload(id).is_empty() => None,
                _ => self.stamp,
            })
        }
    }

    /// A system placed by its id, along a line long enough that a few dozen
    /// of them reach several cells under [`CUTS`].
    fn system(id: u64) -> System {
        let at = id as f64;
        System {
            id64: id,
            position: [at * 37.0, 900.0 + at * 11.0, 24400.0 - at * 23.0],
            absolute_magnitude: (id % 17) as f64,
            temperature: 3000.0 + (id % 7) as f64 * 1000.0,
            age_bucket: (id % 8) as usize,
            updated_at: 0,
        }
    }

    fn layered(base: Held, overlay: Held, on: bool) -> Layered {
        Layered::new(base.arc(), overlay.arc(), Toggle::new(on))
    }

    /// Every cell's rank range says what its ancestors claimed, and the
    /// slices under it add up to what they left it
    ///
    /// The two structural facts the builder in [`crate::tree`] states, read
    /// back off a composed tree. `rank_lo` is how many of a cell's *own*
    /// subtree its ancestors already own, so the slices in that subtree add
    /// up to the count less that claim rather than to the count; and the
    /// claim a cell hands down is shared out among its children, so what the
    /// children say was claimed of them adds up to what was claimed of the
    /// cell plus what the cell took itself.
    ///
    /// The first holds of every cell, and it is the one that says every
    /// system is drawn exactly once. The second holds wherever the children
    /// hold the whole of the cell, which is every cell of a built tree and
    /// every composed cell the two sides resolved alike; where one side
    /// stopped at a leaf the other refined past, its systems are in no
    /// child's aggregate and there is nobody to have claimed them. See
    /// [`a_side_that_stopped_shallower_holds_above_the_children`].
    fn well_formed(index: &Index) {
        fn under(index: &Index, id: CellId) -> u64 {
            let Some(cell) = index.get(id) else { return 0 };
            index
                .children(cell)
                .map(|child| child.id)
                .collect::<Vec<_>>()
                .into_iter()
                .map(|child| under(index, child))
                .sum::<u64>()
                + cell.slice_len()
        }

        for cell in index.cells() {
            assert_eq!(
                under(index, cell.id) + cell.rank_lo,
                cell.aggregate.count(),
                "the slices under {:?} do not add to its count",
                cell.id,
            );
            let held: u64 =
                index.children(cell).map(|it| it.aggregate.count()).sum();
            if held != cell.aggregate.count() {
                continue;
            }
            let handed: u64 = index.children(cell).map(|it| it.rank_lo).sum();
            assert_eq!(
                handed,
                cell.rank_lo + cell.slice_len(),
                "{:?}'s children disagree with it about what was claimed",
                cell.id,
            );
        }
    }

    /// Two trees over disjoint systems add into one that stands for both
    ///
    /// The whole of the composition. What is asked is what the walk reads:
    /// the totals over the union, and the two structural facts a cell's rank
    /// range states. Not cell-for-cell equality with a build over the union —
    /// that build would order the two sets against each other and hand the
    /// bright ones of one set slices the other's ancestors hold here — which
    /// is exactly why this is a layering and not a merge.
    #[test]
    fn two_trees_add_up() {
        let below: Vec<System> = (1..400).map(system).collect();
        let above: Vec<System> = (400..460).map(system).collect();
        let both: Vec<System> = below.iter().chain(&above).copied().collect();

        let source = layered(Held::over(&below), Held::over(&above), true);
        let index = pollster::block_on(source.index()).expect("an index");
        let union = Snapshot::build(&both, &CUTS).index;
        assert!(
            index.len() > 1 && union.len() > 1,
            "the fixtures fit in one cell, so nothing was added cell by cell",
        );

        let count = |index: &Index| {
            index.root().map_or(0, |root| root.aggregate.count())
        };
        assert_eq!(count(&index), both.len() as u64);
        assert_eq!(count(&index), count(&union));

        let flux = |index: &Index| {
            index
                .root()
                .map_or(0.0, |root| root.aggregate.flux().iter().sum::<f64>())
        };
        assert!(
            (flux(&index) - flux(&union)).abs() < flux(&union) * 1e-9,
            "the layered galaxy does not shine what the union does",
        );
        assert_eq!(
            index.root().and_then(|root| root.aggregate.m_min()),
            union.root().and_then(|root| root.aggregate.m_min()),
            "the brightest star came out differently",
        );

        well_formed(&index);
    }

    /// Where one side stopped at a leaf, its systems stay above the other's
    /// children
    ///
    /// Recorded rather than fixed, like the unclaimed overlap, and for the
    /// same reason: nothing outside a built tree can share a side's systems
    /// out among cells that side never raised. A cell one side resolved into
    /// children and the other held whole comes out internal, its own count
    /// the two sides added, and the children carrying only the side that
    /// raised them — a journal's few thousand systems against EDDN's
    /// millions makes that the common case as soon as the tree gets deep.
    ///
    /// What it costs is a deep cell's aggregate standing for one side alone,
    /// the rest counted one level up. What it does not cost is a system
    /// drawn twice or not at all: the slices still partition the sky, which
    /// is [`well_formed`]'s first fact and is asserted here too.
    #[test]
    fn a_side_that_stopped_shallower_holds_above_the_children() {
        let below: Vec<System> = (1..400).map(system).collect();
        let above: Vec<System> = (400..460).map(system).collect();
        let base = Snapshot::build(&below, &CUTS).index;
        let overlay = Snapshot::build(&above, &CUTS).index;

        let source = layered(Held::over(&below), Held::over(&above), true);
        let index = pollster::block_on(source.index()).expect("an index");

        let mut shallower = 0;
        for cell in index.cells().filter(|it| !it.is_leaf()) {
            let held: u64 =
                index.children(cell).map(|it| it.aggregate.count()).sum();
            if held == cell.aggregate.count() {
                continue;
            }
            // Exactly the sides that stopped at this cell, whole.
            let stopped: u64 = [&base, &overlay]
                .iter()
                .filter_map(|side| side.get(cell.id))
                .filter(|side| side.is_leaf())
                .map(|side| side.aggregate.count())
                .sum();
            assert_eq!(
                cell.aggregate.count() - held,
                stopped,
                "{:?} lost more than the side that stopped at it",
                cell.id,
            );
            shallower += 1;
        }
        assert!(
            shallower > 0,
            "the two trees never differ in depth, so this note is untested",
        );
        well_formed(&index);
    }

    /// The toggle takes the overlay off, and says so through the stamps
    ///
    /// A client finds out that anything has changed through [`Source::stamp`]
    /// and nowhere else, so a toggle whose stamps do not move is a toggle
    /// nothing acts on until the base happens to be republished.
    #[test]
    fn the_toggle_is_a_republish() {
        let below: Vec<System> = (1..40).map(system).collect();
        let above: Vec<System> = (40..50).map(system).collect();
        let on = Toggle::new(true);
        let source = Layered::new(
            Held::over(&below).arc(),
            Held::over(&above).arc(),
            on.clone(),
        );

        let count = || {
            pollster::block_on(source.index())
                .expect("an index")
                .root()
                .map_or(0, |root| root.aggregate.count())
        };
        let stamp = || {
            pollster::block_on(source.stamp(Part::Index))
                .expect("a stamp answers")
                .expect("a stamp")
        };

        assert_eq!(count(), 49);
        let showing = stamp();

        on.set(false);
        assert_eq!(count(), 39, "the overlay was drawn with the layer off");
        assert_ne!(stamp(), showing, "flipping the toggle stamped the same");

        on.set(true);
        assert_eq!(count(), 49);
        assert_eq!(stamp(), showing, "flipping back did not come back");
    }

    /// A row the overlay has replaces the base's, and the rest stands
    #[test]
    fn a_table_is_overlaid_by_address() {
        let mut base = Held::over(&[system(1), system(2)]);
        base.reaches = vec![
            SystemReach { address: 1, reach: 100.0 },
            SystemReach { address: 2, reach: 200.0 },
        ];
        base.boosts =
            Some(vec![SystemBoost { address: 1, boost: Boost::WhiteDwarf }]);

        let mut overlay = Held::over(&[system(2)]);
        overlay.reaches = vec![SystemReach { address: 2, reach: 999.0 }];
        overlay.boosts =
            Some(vec![SystemBoost { address: 2, boost: Boost::Neutron }]);

        let source = layered(base, overlay, true);
        let mut reaches =
            pollster::block_on(source.reaches()).expect("the reaches");
        reaches.sort_by_key(|it| it.address);
        assert_eq!(
            reaches,
            vec![
                SystemReach { address: 1, reach: 100.0 },
                SystemReach { address: 2, reach: 999.0 },
            ],
            "the commander's own reading did not win",
        );

        let boosts = pollster::block_on(source.boosts())
            .expect("the boosts")
            .expect("a published table");
        assert_eq!(boosts.len(), 2, "the two tables did not join");
    }

    /// An absent supercharge table on both sides stays absent
    ///
    /// The one distinction the router cannot lose: a table nobody publishes
    /// is a question the map cannot answer, and an empty table is the answer
    /// "nowhere". Told apart here because a route for a supercharging ship
    /// under the first reads as the unaided route under the second's name.
    #[test]
    fn no_supercharge_table_anywhere_is_still_none() {
        let source = layered(Held::default(), Held::default(), true);
        assert_eq!(
            pollster::block_on(source.boosts()).expect("an answer"),
            None
        );
    }

    /// The overlay's names are the chunk after the base's last
    ///
    /// The numbering is the names table's whole layout — from zero, no gaps,
    /// ending at the first that is missing — and a client walks it that way.
    /// So the overlay has to land past the base's end and nowhere else.
    #[test]
    fn the_overlay_names_past_the_base() {
        let below: Vec<System> = (1..40).map(system).collect();
        let base = Held::over(&below);
        base.chunks.store(3, Ordering::Relaxed);
        let overlay = Held::over(&[system(100), system(101)]);

        let source = layered(base, overlay, true);
        let chunk = |n| {
            pollster::block_on(source.names_chunk(n)).expect("a chunk reads")
        };
        let stamp = |n| {
            pollster::block_on(source.stamp(Part::NamesChunk(n)))
                .expect("a stamp answers")
        };

        let read: usize = (0..3).map(|n| chunk(n).len()).sum();
        assert_eq!(read, 39, "the base's own chunks did not come back whole");
        assert_eq!(chunk(3).len(), 2, "the overlay is not the chunk after");
        assert!(chunk(4).is_empty(), "the numbering did not end");

        assert!(stamp(2).is_some());
        assert!(stamp(3).is_some(), "the overlay's chunk stamps as absent");
        assert_eq!(stamp(4), None, "a chunk past the end stamped as present");

        // And the whole table, however it is chunked, holds both.
        let names = pollster::block_on(source.names()).expect("the names");
        assert_eq!(names.len(), 41);
    }

    /// A base that loses its tail chunk takes the overlay down with it
    ///
    /// Where the base's numbering ends is remembered, because a client asks
    /// about every chunk on every poll and working it out afresh for each is
    /// the walk squared. A published table mostly grows, but
    /// [`crate::NameTable`] withdrawing the last entries of the tail chunk
    /// unlinks the file, and an end remembered from before that leaves the
    /// overlay filed past a number nothing serves: the client's walk stops
    /// at the hole and the commander's own systems are gone for as long as
    /// the process runs.
    #[test]
    fn a_base_that_loses_a_chunk_takes_the_overlay_down_with_it() {
        let below: Vec<System> = (1..40).map(system).collect();
        let base = Held::over(&below);
        let chunks = Arc::clone(&base.chunks);
        chunks.store(3, Ordering::Relaxed);
        let source = layered(base, Held::over(&[system(100)]), true);
        let chunk = |n| {
            pollster::block_on(source.names_chunk(n)).expect("a chunk reads")
        };

        assert_eq!(chunk(3).len(), 1, "the overlay is not the fourth chunk");

        // The tail withdrawn, its file unlinked: two chunks left, and the
        // overlay is the third.
        chunks.store(2, Ordering::Relaxed);
        assert_eq!(
            chunk(2).len(),
            1,
            "the overlay stayed filed past the base's new end",
        );
        assert!(
            pollster::block_on(source.stamp(Part::NamesChunk(2)))
                .expect("a stamp answers")
                .is_some(),
            "the overlay's new number stamps as absent",
        );
        assert!(chunk(3).is_empty(), "the numbering outlived the table");
    }

    /// The overlay's chunk stamps present however the overlay stamps
    ///
    /// A client walks the numbering by the stamps, and `names_chunk` serves
    /// the overlay's table at that number whatever the overlay says about
    /// itself. A source answering [`None`] for a part it holds nothing for —
    /// which is what [`Source::stamp`] asks of it — would otherwise end the
    /// walk one number short of a chunk that reads perfectly well.
    #[test]
    fn the_overlay_chunk_stamps_by_what_is_served() {
        let below: Vec<System> = (1..40).map(system).collect();
        let mut overlay = Held::over(&[system(100), system(101)]);
        overlay.stamp = None;
        let source = layered(Held::over(&below), overlay, true);

        assert_eq!(
            pollster::block_on(source.names_chunk(1))
                .expect("a chunk reads")
                .len(),
            2,
            "the overlay is not the chunk after the base's one",
        );
        assert!(
            pollster::block_on(source.stamp(Part::NamesChunk(1)))
                .expect("a stamp answers")
                .is_some(),
            "the chunk the composition serves stamped as absent",
        );
    }

    /// A cell's payload comes back brightest first, and no system twice
    ///
    /// The walk draws a prefix of a payload and calls it the resolvable
    /// part, which is the brightest systems only if the payload is ordered.
    /// Two ordered runs concatenated are not one, which needs the two runs
    /// to interleave: `system` reads a magnitude off the id modulo
    /// seventeen, so ids a hundred apart give the merge two runs it has to
    /// weave rather than append.
    #[test]
    fn a_payload_is_ordered_and_holds_nothing_twice() {
        let below: Vec<System> = (1..80).map(system).collect();
        // Disjoint but for one address, which both sides' root slices hold:
        // zero is the brightest magnitude `system` gives out, and there are
        // fewer than a slice's worth of them on either side.
        let above: Vec<System> =
            (100..180).map(system).chain([system(17)]).collect();
        let source = layered(Held::over(&below), Held::over(&above), true);

        let points = pollster::block_on(source.payload(CellId::ROOT))
            .expect("the root's payload");
        assert!(!points.is_empty(), "the root owns nothing to check");
        assert!(
            points.windows(2).all(|it| it[0].magnitude <= it[1].magnitude),
            "the joined payload is not in magnitude order",
        );

        assert_eq!(
            points.iter().filter(|it| it.id64 == 17).count(),
            1,
            "the address both sides hold was drawn twice",
        );
        let mut seen: Vec<u64> = points.iter().map(|it| it.id64).collect();
        let held = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), held, "a system was in the payload twice");
    }

    /// With nothing claimed, the two sets are not disjoint and it shows
    ///
    /// Recorded rather than fixed. Nothing here can take a system back out of
    /// a built tree, so an overlay that has not been told what the layer
    /// below carries counts the systems both hold twice. This is what
    /// [`Claimed`] exists to stop, and what an unanswered claim costs.
    #[test]
    fn an_unclaimed_overlap_is_counted_twice() {
        let systems: Vec<System> = (1..40).map(system).collect();
        let source = layered(Held::over(&systems), Held::over(&systems), true);
        let index = pollster::block_on(source.index()).expect("an index");
        assert_eq!(
            index.root().map(|root| root.aggregate.count()),
            Some(78),
            "the overlap did not double, so this note is out of date",
        );
        // Still well formed, which is why it is a brightness error in a
        // handful of cells rather than a broken walk.
        well_formed(&index);
    }

    /// A claim answered is a claim a holder can notice
    #[test]
    fn a_claim_says_when_it_has_been_answered() {
        let claimed = Claimed::none();
        assert_eq!(claimed.generation(), 0);
        assert!(!claimed.holds(1), "an unanswered claim held something");

        claimed.set(|address| address == 1);
        assert_eq!(claimed.generation(), 1);
        assert!(claimed.holds(1));
        assert!(!claimed.holds(2));

        claimed.set(|_| false);
        assert_eq!(claimed.generation(), 2, "an answer went unnoticed");
        assert!(!claimed.holds(1));
    }
}
