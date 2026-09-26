//! Picking up what the feed has published since the map read it
//!
//! Everything the map draws from is read once and then held: the cell
//! aggregates the walk plans on, the payloads of the cells in view, and the
//! tables a name, a color and a size come out of. A feed rewrites all of it
//! underneath — `galos ingest --from database --index DIR --watch`
//! republishes every few seconds — and nothing here re-read any of it. The
//! one way a republished cell reached the map was to be evicted and fetched
//! again, which is what zooming out until the walk stops marking it and
//! coming back does. A system scanned while the map stood still never
//! appeared, and a cell the index did
//! not hold at startup could not appear at all: the walk plans off the
//! aggregates, so a cell absent from them is never marked and never asked
//! for.
//!
//! So: a [`Stamp`] per part held, and a poll that asks the transport what each
//! part is now. A stamp is a stat on the filesystem and a conditional
//! request's worth of work over HTTP, so a still map with a quiet index reads
//! nothing; what has moved is re-read and nothing else. See
//! [`galos_index::Source::stamp`].
//!
//! What is re-read whole and what is patched follows what a publish costs to
//! write. The aggregates are half a megabyte and rewritten every pass, so they
//! are read whole. A payload is tens of kilobytes and only the cells whose
//! systems moved are rewritten, so only those are read. The populated,
//! reaches and factions tables are a few megabytes each and written whole, so
//! they are read the same way. The names table is a mapped base and an
//! append-only log of what the feed has said since, so a publish that named
//! something moves the log and nothing else: the refresh reads the rows past
//! the offset it holds and folds them in. The base moves only when the table
//! is recompacted whole, and then it is re-opened — which is five `mmap`
//! calls, not a read.
//!
//! One task at a time, off the main thread, and the whole of it applied in one
//! frame when it lands. The reads are independent, so a pass that finds six
//! parts moved reads six and hands them back together rather than trickling
//! them in over six polls.

use crate::map::galaxy::fetch::Poll;
use crate::map::galaxy::walk::{
    PointOrders, Republished, ResidentCells, adopt,
};
use crate::map::index::{
    Factions, Names, Populated, ResidentIndex, Settled, Transport,
};
use bevy::log::tracing::Instrument;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::read::inhabited::Inhabitance;
use galos_index::records::{
    Faction, PopulatedSystem, SystemBoost, SystemReach,
};
use galos_index::store::names::Delta;
use galos_index::{CellId, Index, Part, Point, Stamp};
use galos_route::Boosts;
use galos_route::graph::Jumps;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<Held>();
    app.init_resource::<Refreshing>();
    // In the fetch set, before the walk asks for cells: a refreshed index is
    // what says which cells the walk should be marking, and a refreshed
    // payload is what the draw reads this frame rather than next. Ordered
    // rather than left to the set — the two conflict on [`ResidentCells`] so
    // they cannot overlap, but which runs first was whatever registration
    // gave them, and `galaxy::plugin` is added first.
    app.add_systems(
        Update,
        (apply, poll)
            .chain()
            .in_set(crate::map::schedule::MapSet::Fetch)
            .before(crate::map::galaxy::walk::fetch),
    );
}

/// What each part was when the map read it
///
/// The whole of what tells a republished part from the one in hand. A part
/// with no stamp — a transport that cannot say, a file that is not there — is
/// read every pass, which is the honest fallback: not knowing whether
/// something changed is not knowing that it did not.
#[derive(Resource, Default)]
pub struct Held {
    index: Option<Stamp>,
    populated: Option<Stamp>,
    reaches: Option<Stamp>,
    boosts: Option<Stamp>,
    factions: Option<Stamp>,
    /// The names table's base: `names/head.bin`, which names the live
    /// generation. It moves only where the table was written whole — a cold
    /// build, or a fold of a log that had grown long — so this is the rare
    /// half of the two.
    names: Option<Stamp>,
    /// The names table's delta log, which every publish that named something
    /// appends to. The common half, and the whole of what a refresh usually
    /// has to read. How far the map has read into it is the table's own
    /// business — [`Names::read_to`] — rather than a second copy kept here.
    delta: Option<Stamp>,
    /// The payloads held, by cell. Kept here rather than beside the payload so
    /// that [`ResidentCells`] stays the walk's set arithmetic and nothing else.
    cells: HashMap<CellId, Option<Stamp>>,
}

impl Held {
    /// What every part is, taken before the map reads any of them
    ///
    /// Seeded rather than left empty, or the first refresh would find every
    /// part unstamped and read the lot — the whole delta log among it — to
    /// discover that nothing had changed.
    ///
    /// Before the reads, not after, which is the same order
    /// [`crate::map::galaxy::walk`]'s fetch takes and for the same reason. A
    /// startup read is seconds long and the feed republishes every few
    /// seconds, so a publish landing in the middle of it is ordinary. Held
    /// under a stamp taken first, the part read is either the one stamped or
    /// the one after it, and the mismatch costs one redundant re-read on the
    /// first poll. Held under a stamp taken afterwards, a part read before the
    /// publish is filed under the stamp of the publish, and every later poll
    /// compares equal: the table would stay as it was read for the life of the
    /// session. `factions.bin` is the worst of those, since it may not move
    /// again for hours.
    ///
    /// Both halves of the names table are stamped: the base's head, which
    /// moves when the table is recompacted, and the log, which moves on
    /// every publish that named anything.
    pub async fn before_reading(source: &dyn galos_index::Source) -> Held {
        let stamp = async |part| source.stamp(part).await.ok().flatten();
        Held {
            index: stamp(Part::Index).await,
            populated: stamp(Part::Populated).await,
            reaches: stamp(Part::Reaches).await,
            boosts: stamp(Part::Boosts).await,
            factions: stamp(Part::Factions).await,
            names: stamp(Part::Names).await,
            delta: stamp(Part::NamesDelta).await,
            cells: HashMap::new(),
        }
    }

    /// Note what a cell's payload was when it was read
    pub fn holding(&mut self, id: CellId, stamp: Option<Stamp>) {
        self.cells.insert(id, stamp);
    }

    /// Forget every cell, the map having let go of every payload at once
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Forget a cell, its payload having been freed
    pub fn forget(&mut self, id: CellId) {
        self.cells.remove(&id);
    }
}

/// The refresh under way, and when the last one went out
#[derive(Resource, Default)]
struct Refreshing {
    task: Option<Task<Refreshed>>,
    asked_at: Option<Instant>,
}

/// What a pass found had moved, and the stamps to hold for it
///
/// Empty for the common case, a quiet index or a pass that found every stamp
/// where it left it, and then nothing is applied and nothing is rebuilt.
#[derive(Default)]
struct Refreshed {
    index: Option<(Index, Option<Stamp>)>,
    populated: Option<(Vec<PopulatedSystem>, Option<Stamp>)>,
    reaches: Option<(Vec<SystemReach>, Option<Stamp>)>,
    /// The supercharge table, or its absence: the inner [`Option`] is whether
    /// the index publishes one at all, which the router needs told apart from
    /// a galaxy where nobody has a jet cone. See [`Boosts::published`].
    boosts: Option<(Option<Vec<SystemBoost>>, Option<Stamp>)>,
    factions: Option<(Vec<Faction>, Option<Stamp>)>,
    /// The names table re-opened whole, with the stamps of both its halves:
    /// the base had been written again, so there is nothing to merge into
    /// the one the map holds. Rare.
    names: Option<(galos_index::Names, Option<Stamp>, Option<Stamp>)>,
    /// The delta log's rows past the offset the map had read to, and the
    /// log's stamp. The common case, and the whole of what a publish that
    /// named something moves.
    delta: Option<(Delta, Option<Stamp>)>,
    /// The payloads re-read, by cell
    cells: Vec<(CellId, Vec<Point>, Option<Stamp>)>,
}

impl Refreshed {
    fn is_empty(&self) -> bool {
        self.index.is_none()
            && self.populated.is_none()
            && self.reaches.is_none()
            && self.boosts.is_none()
            && self.factions.is_none()
            && self.names.is_none()
            && self.delta.is_none()
            && self.cells.is_empty()
    }
}

/// Ask the transport what has moved, and read what has
///
/// On the [`Poll`] beat, which is the one setting for how often the map goes
/// back to what it reads from, and one task at a time: a pass still on the
/// wire is a pass whose answer has not landed, and asking again over the top
/// of it would read the same files twice.
fn poll(
    transport: Res<Transport>,
    resident: Res<ResidentCells>,
    held: Res<Held>,
    names: Res<Names>,
    time: Res<Time<Real>>,
    poll: Res<Poll>,
    mut refreshing: ResMut<Refreshing>,
) {
    if refreshing.task.is_some() {
        return;
    }
    let now = time.last_update().unwrap_or(time.startup());
    let asked_at = refreshing.asked_at.unwrap_or_else(|| time.startup());
    if !poll.elapsed(asked_at, now) {
        return;
    }
    refreshing.asked_at = Some(now);

    let source = transport.0.clone();
    // What the task must ask about: the parts held, named here on the main
    // thread where the resident sets are.
    let index = held.index;
    let (populated, reaches, boosts, factions) =
        (held.populated, held.reaches, held.boosts, held.factions);
    let (names_head, delta) = (held.names, held.delta);
    // How far the table has read into the log, which is what the tail is
    // asked for past. Read off the table rather than filed beside the stamps:
    // the offset is the table's own bookkeeping, moved by every
    // [`Names::absorb`], and a second copy here is a copy to get wrong.
    let read_to = names.read_to();
    let cells: Vec<(CellId, Option<Stamp>)> = resident
        .0
        .iter()
        .map(|(id, _)| (id, held.cells.get(&id).copied().flatten()))
        .collect();

    // The pass as one zone, wrapped around the future for the reason
    // [`crate::map::index::load::start`] gives: a task pool future cannot hold an
    // entered span's guard, and nothing under this one yields — a stamp and a
    // part are both blocking reads — so the span opens and closes on the one
    // pool thread.
    let pass = async move {
        let mut found = Refreshed::default();

        // Whether a part has moved since the stamp in hand. Every part the
        // map holds was stamped before it was read (see
        // [`Held::before_reading`]), so there is always something to compare
        // against: a part still absent stamps [`None`] on both sides and reads
        // as unchanged, where taking that for "cannot say" would re-read the
        // same absence on every poll and mark its table changed each time.
        async fn moved(
            source: &Arc<dyn galos_index::Source>,
            part: Part,
            held: Option<Stamp>,
        ) -> (bool, Option<Stamp>) {
            match source.stamp(part).await {
                Ok(now) => (now != held, now),
                // A transport that errors on a stamp is one to ask again next
                // pass, not one to read the whole index from.
                Err(_) => (false, held),
            }
        }

        let (index_moved, stamp) = moved(&source, Part::Index, index).await;
        if index_moved && let Ok(read) = source.index().await {
            found.index = Some((read, stamp));
        }

        let (moved_it, stamp) =
            moved(&source, Part::Populated, populated).await;
        if moved_it && let Ok(read) = source.populated().await {
            found.populated = Some((read, stamp));
        }

        let (moved_it, stamp) = moved(&source, Part::Reaches, reaches).await;
        if moved_it && let Ok(read) = source.reaches().await {
            found.reaches = Some((read, stamp));
        }

        // A table gone as well as a table moved: an index rebuilt without one
        // takes the supercharging away, and the map has to stop claiming it.
        let (moved_it, stamp) = moved(&source, Part::Boosts, boosts).await;
        if moved_it && let Ok(read) = source.boosts().await {
            found.boosts = Some((read, stamp));
        }

        let (moved_it, stamp) = moved(&source, Part::Factions, factions).await;
        if moved_it && let Ok(read) = source.factions().await {
            found.factions = Some((read, stamp));
        }

        // The names table, in the two shapes a publish can move it in. Both
        // stamped before either is read, so a publish landing between the two
        // reads costs a redundant pass and never a row held under a stamp
        // newer than it.
        let (base_moved, base_stamp) =
            moved(&source, Part::Names, names_head).await;
        let (log_moved, log_stamp) =
            moved(&source, Part::NamesDelta, delta).await;
        if base_moved && let Ok(read) = source.names().await {
            // The base was written whole — a cold build, or a fold of a log
            // that had grown long. The log is removed by the swap, so the
            // offset the map holds means nothing against the new one and
            // there is nothing to merge: the table is re-opened instead,
            // which is five `mmap` calls and whatever log now stands.
            found.names = Some((read, base_stamp, log_stamp));
        } else if log_moved && let Ok(tail) = source.names_delta(read_to).await
        {
            // The common case: the feed named something and the rows past
            // the offset the map holds are exactly what changed.
            found.delta = Some((tail, log_stamp));
        }

        for (id, held) in cells {
            let (moved_it, stamp) = moved(&source, Part::Cell(id), held).await;
            if moved_it && let Ok(read) = source.payload(id).await {
                found.cells.push((id, read, stamp));
            }
        }

        found
    };
    refreshing.task = Some(
        AsyncComputeTaskPool::get()
            .spawn(pass.instrument(info_span!("refresh poll"))),
    );
}

/// Take what a finished pass read into the resident tables
///
/// All of it in one frame. The parts are read from one published directory and
/// belong to one another — the names table and the cell tree stand for the same
/// systems by construction — so applying half of them would draw a sky the
/// builder never published.
#[expect(
    clippy::too_many_arguments,
    reason = "every resident table a refresh can replace, and the stamps"
)]
fn apply(
    mut refreshing: ResMut<Refreshing>,
    mut held: ResMut<Held>,
    mut index: ResMut<ResidentIndex>,
    mut resident: ResMut<ResidentCells>,
    mut admitted: ResMut<PointOrders>,
    mut republished: ResMut<Republished>,
    mut populated: ResMut<Populated>,
    mut settled: ResMut<Settled>,
    mut names: ResMut<Names>,
    mut factions: ResMut<Factions>,
    mut boosts: ResMut<Boosts>,
    mut jumps: ResMut<Jumps>,
) {
    let Some(task) = refreshing.task.as_mut() else { return };
    let Some(found) = block_on(future::poll_once(task)) else { return };
    refreshing.task = None;
    if found.is_empty() {
        return;
    }

    // The political aggregation is derived from the tree and the populated
    // table together, so either of them moving invalidates it. Read before
    // the two branches below consume what they found.
    let resettle = found.index.is_some() || found.populated.is_some();

    if let Some((read, stamp)) = found.index {
        index.0 = read;
        held.index = stamp;
    }
    if let Some((read, stamp)) = found.populated {
        populated.0 =
            Arc::new(read.into_iter().map(|it| (it.address, it)).collect());
        held.populated = stamp;
    }
    // Rolled up again over whichever of the two just moved. The same cost as
    // the whole-table replacements it sits among — one pass over a resident
    // table — and it must run after both branches, since it reads both.
    if resettle {
        settled.0 = Arc::new(Inhabitance::of(&index.0, populated.0.values()));
    }
    if let Some((read, stamp)) = found.factions {
        factions.0 = read.into_iter().map(|it| (it.id, it.name)).collect();
        held.factions = stamp;
    }
    if let Some((read, stamp)) = found.reaches {
        names.reaches = Arc::new(crate::map::index::names::Reaches::of(read));
        held.reaches = stamp;
    }

    // Which systems can supercharge, replaced whole: the table is about a
    // megabyte and written whole, so there is no part of it to read.
    let found_boosts = found.boosts.is_some();
    if let Some((read, stamp)) = found.boosts {
        *boosts = read.map_or_else(Boosts::absent, Boosts::of);
        held.boosts = stamp;
    }

    // The names table, in the two shapes a publish can move it in.
    //
    // The base written whole is the rare one — a cold build, or a fold of a
    // log that had grown long. The table is re-opened and put in place of the
    // one the map holds, which is five mappings and whatever log now stands;
    // there is nothing to merge, the offset the map had read to belonging to
    // a log the swap removed.
    let rebased = found.names.is_some();
    if let Some((read, base, log)) = found.names {
        names.table = read;
        held.names = base;
        held.delta = log;
    }
    // The log moving is the common case, and folding its tail in is the whole
    // of the merge: the rows *are* the changes. Nothing is diffed against
    // what the table already says — a row is only in the log because it
    // changed something — and a withdrawal is a row like any other, so
    // nothing has to be filed under where it came from to be taken back out
    // again. What the map reads is the arrivals, not the table and not the
    // log.
    if let Some((tail, stamp)) = found.delta {
        names.absorb(tail);
        held.delta = stamp;
    }

    // The router reads the cell payloads where they lie and the supercharge
    // table beside them, so an arrival is in the galaxy it searches as soon
    // as the feed has published the cell — there is no second copy to keep
    // in step, and nothing here to rebucket. What does go stale is the
    // supercharge table, which the graph holds a copy of.
    //
    // Dropped rather than rebuilt, and it costs nothing to drop: the graph
    // is a handle on the index, not a structure over it, so the next route
    // asked for opens another. See [`Jumps::built`].
    if rebased || found_boosts {
        jumps.graph = None;
    }

    // A replaced payload is a new set of points in the same cell, so whatever
    // was worked out about the old one goes with it, and so do the systems
    // already drawn out of it; [`adopt`] is where the three are kept together.
    //
    // Only cells the map still holds. A pass takes seconds and the walk lets
    // payloads go as the camera moves, so a cell asked about at the start of
    // one may be gone by the time it lands — and a source switch drops every
    // payload at once while a pass is on the wire. Inserting one back would
    // resurrect it: the draw would spawn its systems for a frame before the
    // evictor took it again, and with the walk switched off nothing would ever
    // take it, leaving the payload and its stamp to be re-read every poll for
    // the life of the process.
    for (id, points, stamp) in found.cells {
        if !resident.0.contains(id) {
            continue;
        }
        adopt(&mut resident, &mut admitted, &mut republished, id, points);
        held.holding(id, stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::records::NameEntry;
    use galos_index::{BuildParams, FsSource, Snapshot, Source as IndexSource};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch published directory, removed when the guard drops
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("galos_refresh_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One system for the builder, placed along the x axis
    fn input(id: u64, at: f64) -> galos_index::System {
        galos_index::System {
            id64: id,
            position: [at, 900.0, 24400.0],
            absolute_magnitude: id as f64,
            temperature: 5000.0,
            age_bucket: 0,
            updated_at: 1_700_000_000 + id as u32,
            kind: galos_index::StarKind::G,
        }
    }

    /// Publish `systems` to `dir` and hand back the built tree
    fn publish(
        dir: &std::path::Path,
        systems: &[galos_index::System],
    ) -> Snapshot {
        let built = Snapshot::build(systems, &BuildParams::default());
        built.write(dir).expect("the build should write");
        built
    }

    /// One row of the names table
    fn named(address: i64, name: &str) -> NameEntry {
        NameEntry {
            address,
            name: name.into(),
            position: [0.0, 900.0, 24400.0],
        }
    }

    /// Write `entries` as `dir`'s mapped base, the way a build's writer does
    fn publish_base(dir: &std::path::Path, entries: &[NameEntry]) {
        let mut writing = galos_index::store::names::Writer::writing(dir)
            .expect("the writer should open");
        for entry in entries {
            writing.push(entry.clone()).expect("the row should write");
        }
        writing.finish().expect("the base should swap in");
    }

    /// Name `entries` into `dir`'s delta log the way a feed's publish does,
    /// answering how many rows it appended
    fn name(dir: &std::path::Path, entries: &[NameEntry]) -> usize {
        let mut table =
            galos_index::Names::open(dir).expect("the table should open");
        for entry in entries {
            table.name(entry.clone());
        }
        table.publish(dir).expect("the log should append")
    }

    /// Withdraw `address` through the log, the way a feed's publish does
    fn unname(dir: &std::path::Path, address: i64) {
        let mut table =
            galos_index::Names::open(dir).expect("the table should open");
        assert!(table.unname(address), "{address} was there to withdraw");
        table.publish(dir).expect("the log should append");
    }

    /// A map holding what `dir` published, with the refresh wired as the app
    /// wires it and the poll wide open
    ///
    /// `built` is the directory's own tree, the payloads being what the walk
    /// would have fetched by the time the map is up.
    fn watching(dir: &std::path::Path, built: &Snapshot) -> App {
        let source: Arc<dyn IndexSource> = Arc::new(FsSource::new(dir));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, (apply, poll).chain());
        app.insert_resource(block_on(Held::before_reading(&*source)));
        app.init_resource::<Refreshing>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<PointOrders>();
        app.init_resource::<Republished>();
        app.init_resource::<Populated>();
        app.init_resource::<Settled>();
        app.init_resource::<Factions>();
        // The tables as `main` loads them: the names table mapped, and the
        // router's galaxy opened over the same directory the walk reads.
        let table = block_on(source.names()).expect("the names should open");
        let reaches = block_on(source.reaches()).unwrap_or_default();
        let boosts = block_on(source.boosts())
            .ok()
            .flatten()
            .map_or_else(Boosts::absent, Boosts::of);
        let sky = Arc::new(
            galos_index::Sky::open(dir).expect("the galaxy should map"),
        );
        let names = Names::packed(
            table,
            crate::map::index::names::Reaches::of(reaches),
            Some(Arc::clone(&sky)),
        );
        app.insert_resource(Jumps::over(sky));
        app.insert_resource(boosts);
        app.insert_resource(names);
        app.insert_resource(ResidentIndex(
            block_on(source.index()).expect("a published index"),
        ));
        app.insert_resource(Transport(Arc::clone(&source)));
        app.insert_resource(Poll(Some(0.)));

        // Every cell that owns systems, held as the walk's fetch holds it,
        // stamp and all.
        let cells: Vec<CellId> = app
            .world()
            .resource::<ResidentIndex>()
            .0
            .cells()
            .map(|cell| cell.id)
            .collect();
        for id in cells {
            let points = built.payload(id);
            if points.is_empty() {
                continue;
            }
            let stamp = block_on(source.stamp(Part::Cell(id))).expect("stat");
            app.world_mut()
                .resource_mut::<ResidentCells>()
                .0
                .insert(id, points.to_vec());
            app.world_mut().resource_mut::<Held>().holding(id, stamp);
        }
        app
    }

    /// The addresses of every point the map holds, in any cell
    fn holding(app: &App) -> Vec<i64> {
        let mut held: Vec<i64> = app
            .world()
            .resource::<ResidentCells>()
            .0
            .iter()
            .flat_map(|(_, cell)| cell.points.iter())
            .map(|point| point.id64 as i64)
            .collect();
        held.sort();
        held
    }

    /// Run frames until `done`, or give up: the reads are on the task pool, so
    /// an answer lands a frame or two after it is asked for.
    fn pump(app: &mut App, mut done: impl FnMut(&App) -> bool) -> bool {
        for _ in 0..200 {
            app.update();
            if done(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    /// A system published while the map stands still arrives without the
    /// camera moving
    ///
    /// The reported trouble: nothing re-read a cell the map already held, so a
    /// system scanned into a resident cell never appeared. The only way to see
    /// one was to zoom out until the walk stopped marking the cell and come
    /// back, which evicted the payload and fetched it again.
    #[test]
    fn a_system_published_into_a_held_cell_arrives() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        let mut app = watching(&dir.0, &built);

        assert_eq!(holding(&app), vec![1], "one system published, one held");

        // The feed reports a second system in the same neighbourhood, and the
        // builder republishes the cell that owns it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        publish(&dir.0, &[input(1, 0.0), input(2, 8.0)]);

        assert!(
            pump(&mut app, |app| holding(app) == vec![1, 2]),
            "the republished cell never landed: held {:?}",
            holding(&app),
        );
    }

    /// A name appended to the log is found without a restart
    ///
    /// The names table's base is mapped and immutable, so what the feed names
    /// mid-session is a row in the delta log. A refresh reads the rows past
    /// the offset it holds and folds them in, which is the whole of how a
    /// system named while the map runs comes to be named on the map.
    #[test]
    fn a_name_appended_to_the_log_is_found() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        publish_base(&dir.0, &[named(1, "First")]);

        let mut app = watching(&dir.0, &built);
        assert!(
            app.world().resource::<Names>().get(2).is_none(),
            "nothing names the second system yet"
        );

        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[named(2, "Second")]);

        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(2)
                .is_some_and(|entry| entry.name == "SECOND")),
            "the name published mid-session was never picked up",
        );
        assert_eq!(
            app.world().resource::<Names>().address("Second"),
            Some(2),
            "and a search reaches it"
        );
    }

    /// The log accumulates arrivals, and a rename answers over the base
    ///
    /// Which is the whole of how the table stays right over a session: every
    /// pass folds a tail in over what earlier passes folded, and the log is
    /// the later word on every address it mentions — so a system the base
    /// names is answered under the name the feed corrected it to.
    #[test]
    fn the_log_keeps_what_earlier_passes_folded_in() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        publish_base(&dir.0, &[named(1, "First")]);
        let mut app = watching(&dir.0, &built);

        let named_at = |app: &App, address: i64| {
            app.world()
                .resource::<Names>()
                .get(address)
                .map(|entry| entry.name.clone())
        };

        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[named(2, "Second")]);
        assert!(
            pump(&mut app, |app| named_at(app, 2).is_some()),
            "the first arrival was never picked up"
        );

        // A second arrival, and a correction to the name the base holds.
        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[named(3, "Third"), named(1, "First, Renamed")]);
        assert!(
            pump(&mut app, |app| named_at(app, 3).is_some()),
            "the second arrival was never picked up"
        );

        assert_eq!(
            named_at(&app, 2).as_deref(),
            Some("SECOND"),
            "the earlier arrival is still named"
        );
        assert_eq!(
            named_at(&app, 1).as_deref(),
            Some("FIRST, RENAMED"),
            "and a rename answers over what the base was written with"
        );
        let names = app.world().resource::<Names>();
        assert_eq!(names.len(), 3, "three systems named, each counted once");
        assert_eq!(
            names.find("First", None, 25).len(),
            1,
            "a renamed system is listed once, under its new name"
        );
    }

    /// Startup stamps every part it read, so the first refresh reads nothing
    ///
    /// Left unstamped, a part reads as changed — not knowing is not knowing
    /// that it did not — so the first poll would re-read every table the map
    /// had just read to find nothing had moved.
    #[test]
    fn startup_stamps_every_part_it_read() {
        use galos_index::format::layout::{
            factions_path, populated_path, reaches_path,
        };
        use galos_index::format::msgpack::write_meta;

        let dir = Scratch::new();
        publish(&dir.0, &[input(1, 0.0)]);
        write_meta(&populated_path(&dir.0), &Vec::<PopulatedSystem>::new())
            .expect("the populated table should write");
        write_meta(&reaches_path(&dir.0), &Vec::<SystemReach>::new())
            .expect("the reaches table should write");
        write_meta(&factions_path(&dir.0), &Vec::<Faction>::new())
            .expect("the factions table should write");
        publish_base(&dir.0, &[named(1, "First")]);
        name(&dir.0, &[named(2, "Second")]);

        let transport: Arc<dyn IndexSource> = Arc::new(FsSource::new(&dir.0));
        let held = block_on(Held::before_reading(&*transport));

        assert!(held.index.is_some(), "the aggregates");
        assert!(held.populated.is_some(), "the populated table");
        assert!(held.reaches.is_some(), "the reaches table");
        assert!(held.factions.is_some(), "the factions table");
        assert!(held.names.is_some(), "the names table's base");
        assert!(held.delta.is_some(), "and its log");
    }

    /// A republish that says nothing new drops nothing
    ///
    /// The feed republishes every few seconds and names the same systems it
    /// named last pass. A row only reaches the log where it changed something
    /// ([`galos_index::Names::name`]), so a publish of what the table already
    /// says appends nothing, moves no stamp, and is read by nobody — and the
    /// graph a route in flight is searching over is left where it is rather
    /// than dropped for the nothing that moved. Dropping it is cheap now,
    /// but it is not free of consequence: the next route pays for another
    /// and a route already running finishes against the one it holds.
    #[test]
    fn a_republish_of_the_same_names_drops_nothing() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        publish_base(&dir.0, &[named(1, "First")]);
        let mut app = watching(&dir.0, &built);

        // An arrival, folded in.
        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[named(2, "Second")]);
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(2)
                .is_some()),
            "the arrival was never picked up"
        );
        let read_to = app.world().resource::<Names>().read_to();
        // As a route asks for it: the graph is opened on the first ask and
        // held thereafter, so there is one here to be dropped at all.
        let boosts = app.world().resource::<Boosts>().clone();
        let graph = app
            .world_mut()
            .resource_mut::<Jumps>()
            .built(&boosts)
            .expect("a graph over the galaxy the map opened");

        // The same two systems reported again, exactly as the table has them.
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(
            name(&dir.0, &[named(1, "First"), named(2, "Second")]),
            0,
            "the log took rows for names it already held",
        );
        for _ in 0..40 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let names = app.world().resource::<Names>();
        assert_eq!(names.read_to(), read_to, "the log was read again");
        assert_eq!(names.len(), 2, "two systems named, each counted once");
        assert!(
            matches!(
                &app.world().resource::<Jumps>().graph,
                Some(now) if Arc::ptr_eq(&graph, now)
            ),
            "the router's graph was dropped to take in nothing"
        );
        assert!(names.get(2).is_some(), "and the arrival is still named");
    }

    /// A payload that lands for a cell the map has let go is not taken
    ///
    /// A pass takes seconds, and the walk drops payloads as the camera moves —
    /// a source switch drops every one at once. Taking one back would spawn
    /// its systems for a frame, and with the walk switched off nothing would
    /// ever free it again: the payload and its stamp would be re-read on every
    /// poll for the life of the process.
    #[test]
    fn a_cell_let_go_of_mid_pass_is_not_taken_back() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        let mut app = watching(&dir.0, &built);
        assert_eq!(holding(&app), vec![1]);

        // Republished with a second system, so the pass has something to
        // hand back for the cell.
        std::thread::sleep(std::time::Duration::from_millis(10));
        publish(&dir.0, &[input(1, 0.0), input(2, 8.0)]);

        // One frame sends the pass out; the map lets every payload go while it
        // is on the wire, as a source switch does.
        app.update();
        app.world_mut().resource_mut::<ResidentCells>().0 =
            galos_index::read::resident::Resident::default();
        app.world_mut().resource_mut::<Held>().clear();

        for _ in 0..40 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(
            holding(&app).is_empty(),
            "a payload was taken back for a cell the map had let go: {:?}",
            holding(&app),
        );
    }

    /// A withdrawal takes a name off the map
    ///
    /// A system the feed withdraws is a tombstone in the log, over a row the
    /// log named or over one the base holds. Left answering, it would go on
    /// filling the search box, naming a place to plot from and counting in
    /// the diagnostics for the rest of the session, over a sky that had
    /// stopped drawing it.
    #[test]
    fn a_withdrawal_takes_a_name_away() {
        let dir = Scratch::new();
        let placed = |address: i64, at: f32| NameEntry {
            address,
            name: format!("S{address}").into(),
            position: [at, 0.0, 0.0],
        };
        let built = publish(&dir.0, &[input(1, 0.0)]);
        publish_base(&dir.0, &[placed(1, 0.0)]);

        let mut app = watching(&dir.0, &built);
        assert_eq!(
            app.world().resource::<Names>().len(),
            1,
            "the table as it was opened"
        );

        // A system named into the log, which the refresh folds in.
        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[placed(7, 32.0)]);
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(7)
                .is_some()),
            "the named system was never picked up",
        );

        // Withdrawn again: the log says so, and the map has to stop naming it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        unname(&dir.0, 7);
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(7)
                .is_none()),
            "the withdrawn system is still named",
        );
        let names = app.world().resource::<Names>();
        assert_eq!(names.address("S7"), None, "the search cannot reach it");
        assert_eq!(names.len(), 1, "and it is not counted");
        assert!(
            names.get(1).is_some(),
            "the row the base holds is named throughout"
        );

        // And a row of the base withdrawn: the tombstone hides what the
        // mapping still holds, the base being immutable.
        std::thread::sleep(std::time::Duration::from_millis(10));
        unname(&dir.0, 1);
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(1)
                .is_none()),
            "a base row withdrawn is still named",
        );
        let names = app.world().resource::<Names>();
        assert_eq!(names.address("S1"), None, "the search still reaches it");
        assert_eq!(names.table.len(), 0, "and the table still counts it");
    }

    /// A base recompacted whole is re-opened rather than merged
    ///
    /// The rare half of the two. A fold takes the log into a new generation
    /// and removes it, so the offset the map had read to means nothing and
    /// there is no tail to take: the table is mapped afresh. Every name it
    /// answered before the fold it answers after, and the graph the router
    /// holds — whose supercharge table was read beside a base that has been
    /// written again — is dropped for the next route to open another.
    #[test]
    fn a_recompacted_base_is_re_opened() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        publish_base(&dir.0, &[named(1, "First")]);
        let mut app = watching(&dir.0, &built);

        std::thread::sleep(std::time::Duration::from_millis(10));
        name(&dir.0, &[named(2, "Second")]);
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(2)
                .is_some()),
            "the arrival was never picked up"
        );

        // As a route asks for it, so there is a graph to drop at all.
        let boosts = app.world().resource::<Boosts>().clone();
        assert!(
            app.world_mut().resource_mut::<Jumps>().built(&boosts).is_some(),
            "the galaxy the map opened should route"
        );

        // The log folded into a new base, as a log grown long is.
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(
            galos_index::store::names::compact(&dir.0)
                .expect("the fold should write"),
            2,
            "both systems should be in the base the fold wrote",
        );

        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .table
                .delta()
                .is_empty()),
            "the folded-away log is still held",
        );
        let names = app.world().resource::<Names>();
        assert_eq!(names.len(), 2, "both systems are still named");
        assert_eq!(names.address("Second"), Some(2), "out of the new base");
        assert!(
            app.world().resource::<Jumps>().graph.is_none(),
            "the graph read beside the old base was kept"
        );
    }
}
