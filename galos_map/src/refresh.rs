//! Picking up what the feed has published since the map read it
//!
//! Everything the map draws from is read once and then held: the cell
//! aggregates the walk plans on, the payloads of the cells in view, and the
//! tables a name, a color and a size come out of. A feed rewrites all of it
//! underneath — `galos-sync db --watch` republishes every few seconds — and
//! nothing here re-read any of it. The one way a republished cell reached the
//! map was to be evicted and fetched again, which is what zooming out until the
//! walk stops marking it and coming back does. A system scanned while the map
//! stood still never appeared, and a cell the index did not hold at startup
//! could not appear at all: the walk plans off the aggregates, so a cell absent
//! from them is never marked and never asked for.
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
//! they are read the same way. The names table is a hundred megabytes across
//! chunks of which a pass moves one — arrivals land in the tail — so its
//! changed chunks are read and merged into [`Names::fresh`] rather than the
//! table being rebuilt around them.
//!
//! One task at a time, off the main thread, and the whole of it applied in one
//! frame when it lands. The reads are independent, so a pass that finds six
//! parts moved reads six and hands them back together rather than trickling
//! them in over six polls.

use crate::systems::bounded::{PointOrders, Republished, ResidentCells, adopt};
use crate::systems::fetch::Poll;
use crate::systems::route::graph::Jumps;
use crate::{Boosts, Factions, Names, Populated, ResidentIndex, Transport};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::meta::{
    Faction, NameEntry, PopulatedSystem, SystemBoost, SystemReach,
};
use galos_index::{CellId, Index, Part, Point, Stamp};
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
    // gave them, and `systems::plugin` is added first.
    app.add_systems(
        Update,
        (apply, poll)
            .chain()
            .in_set(crate::schedule::MapSet::Fetch)
            .before(crate::systems::bounded::fetch),
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
    /// The chunks of the names table, by number, as far as the table went when
    /// it was last read. A chunk appearing past the end is a chunk to read.
    chunks: Vec<Option<Stamp>>,
    /// The payloads held, by cell. Kept here rather than beside the payload so
    /// that [`ResidentCells`] stays the walk's set arithmetic and nothing else.
    cells: HashMap<CellId, Option<Stamp>>,
}

impl Held {
    /// What every part is, taken before the map reads any of them
    ///
    /// Seeded rather than left empty, or the first refresh would find every
    /// part unstamped, read the lot — a hundred megabytes of names table among
    /// it — and discover that nothing had changed.
    ///
    /// Before the reads, not after, which is the same order
    /// [`super::systems::bounded`]'s fetch takes and for the same reason. A
    /// startup read is a hundred megabytes long and the feed republishes every
    /// few seconds, so a publish landing in the middle of it is ordinary. Held
    /// under a stamp taken first, the part read is either the one stamped or
    /// the one after it, and the mismatch costs one redundant re-read on the
    /// first poll. Held under a stamp taken afterwards, a part read before the
    /// publish is filed under the stamp of the publish, and every later poll
    /// compares equal: the table would stay as it was read for the life of the
    /// session. `factions.bin` is the worst of those, since it may not move
    /// again for hours.
    ///
    /// The chunks are stamped up to the end of the table, the walk stopping at
    /// the first number the transport has nothing for, which is the layout's
    /// own contract: numbered from zero with no gaps.
    pub async fn before_reading(source: &dyn galos_index::Source) -> Held {
        let stamp = async |part| source.stamp(part).await.ok().flatten();
        let mut chunks = Vec::new();
        for chunk in 0.. {
            match stamp(Part::NamesChunk(chunk)).await {
                Some(it) => chunks.push(Some(it)),
                None => break,
            }
        }
        Held {
            index: stamp(Part::Index).await,
            populated: stamp(Part::Populated).await,
            reaches: stamp(Part::Reaches).await,
            boosts: stamp(Part::Boosts).await,
            factions: stamp(Part::Factions).await,
            chunks,
            cells: HashMap::new(),
        }
    }

    /// Note what a cell's payload was when it was read
    pub fn holding(&mut self, id: CellId, stamp: Option<Stamp>) {
        self.cells.insert(id, stamp);
    }

    /// Forget every cell, the map having let go of every payload at once
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
    /// The chunks read, by number, and how far the table now goes
    chunks: Vec<(usize, Vec<NameEntry>, Option<Stamp>)>,
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
            && self.chunks.is_empty()
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
    let chunks = held.chunks.clone();
    let cells: Vec<(CellId, Option<Stamp>)> = resident
        .0
        .iter()
        .map(|(id, _)| (id, held.cells.get(&id).copied().flatten()))
        .collect();

    refreshing.task = Some(AsyncComputeTaskPool::get().spawn(async move {
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

        // The chunks held, and then whatever the table has grown by: a new
        // chunk has no stamp on record, so the walk past the end stops at the
        // first number the transport has nothing for.
        for chunk in 0.. {
            let held = chunks.get(chunk).copied().flatten();
            let (moved_it, stamp) =
                moved(&source, Part::NamesChunk(chunk), held).await;
            if stamp.is_none() && chunk >= chunks.len() {
                break;
            }
            if moved_it && let Ok(read) = source.names_chunk(chunk).await {
                found.chunks.push((chunk, read, stamp));
            }
        }

        for (id, held) in cells {
            let (moved_it, stamp) = moved(&source, Part::Cell(id), held).await;
            if moved_it && let Ok(read) = source.payload(id).await {
                found.cells.push((id, read, stamp));
            }
        }

        found
    }));
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

    if let Some((read, stamp)) = found.index {
        index.0 = read;
        held.index = stamp;
    }
    if let Some((read, stamp)) = found.populated {
        populated.0 =
            Arc::new(read.into_iter().map(|it| (it.address, it)).collect());
        held.populated = stamp;
    }
    if let Some((read, stamp)) = found.factions {
        factions.0 = read.into_iter().map(|it| (it.id, it.name)).collect();
        held.factions = stamp;
    }
    if let Some((read, stamp)) = found.reaches {
        names.reaches = Arc::new(
            read.into_iter().map(|it| (it.address, it.reach)).collect(),
        );
        held.reaches = stamp;
    }

    // Which systems can supercharge, replaced whole: the table is about a
    // megabyte and written whole, so there is no part of it to read.
    let found_boosts = found.boosts.is_some();
    if let Some((read, stamp)) = found.boosts {
        *boosts = read.map_or_else(Boosts::absent, Boosts::of);
        held.boosts = stamp;
    }

    // The chunks that moved, merged into the overlay: only what the table does
    // not already say, so a chunk re-read after one system was appended to it
    // adds one entry rather than sixty-five thousand.
    //
    // Against what the table answers *now* — [`Names::get`], overlay first —
    // and not against the base alone. Diffed against the base, every arrival
    // an earlier pass put in the overlay would read as new again on every
    // re-read of its chunk, and the tail chunk moves on nearly every publish:
    // the overlay and the router's graph would be rebuilt each poll, at the
    // size of the whole session's arrivals, to take in the one system that
    // actually moved. A correction that puts an entry back to what the base
    // says still differs from the overlay's answer, so it is still taken.
    let mut arrived: Vec<NameEntry> = Vec::new();
    // The lowest chunk the transport had nothing for, if any: a table that has
    // shrunk. Cut back to it once, after the loop, since the loop resizes as
    // it goes and truncating inside it would be undone by the next chunk.
    let mut gone: Option<usize> = None;
    for (chunk, entries, stamp) in found.chunks {
        for entry in entries {
            if names.get(entry.address) != Some(&entry) {
                arrived.push(entry);
            }
        }
        if held.chunks.len() <= chunk {
            held.chunks.resize(chunk + 1, None);
        }
        held.chunks[chunk] = stamp;
        if stamp.is_none() {
            gone = Some(gone.unwrap_or(chunk).min(chunk));
        }
    }
    // Or every poll would stat and read files that are not there for as long
    // as the map runs: a missing stamp reads as moved.
    if let Some(gone) = gone {
        held.chunks.truncate(gone);
    }
    let named = !arrived.is_empty();
    if named {
        let mut fresh = HashMap::clone(&names.fresh);
        fresh.extend(arrived.into_iter().map(|entry| (entry.address, entry)));
        names.fresh = Arc::new(fresh);
    }

    // The router reads places and what they can supercharge, and has just been
    // handed either some places it did not have or a new table of the second.
    //
    // Rebuilt from the whole overlay rather than added to, which is the same
    // work and no bookkeeping: the base is a handle clone, the overlay is the
    // arrivals of one session, and the supercharge table is another handle. A
    // route already searching holds the graph it started on and finishes
    // against that.
    if named || found_boosts {
        jumps.0 = Arc::new(jumps.0.extended(names.fresh.values(), &boosts));
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
    use crate::systems::route::graph::{Drive, JumpGraph, Routing};
    use galos_index::{
        BuildParams, FsSource, NameEntry, NameTable, Snapshot,
        Source as IndexSource,
    };
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

    /// A map holding what `dir` published, with the refresh wired as the app
    /// wires it and the poll wide open
    fn watching(dir: &std::path::Path, built: &Snapshot) -> App {
        let source = FsSource::new(dir);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, (apply, poll).chain());
        let transport: Arc<dyn IndexSource> = Arc::new(FsSource::new(dir));
        app.insert_resource(block_on(Held::before_reading(&*transport)));
        app.init_resource::<Refreshing>();
        app.init_resource::<ResidentCells>();
        app.init_resource::<PointOrders>();
        app.init_resource::<Republished>();
        app.init_resource::<Populated>();
        app.init_resource::<Factions>();
        // The tables as `main` loads them: the names table read whole, and
        // the router's graph bucketed off it.
        let named = block_on(source.names()).unwrap_or_default();
        let reaches = block_on(source.reaches()).unwrap_or_default();
        let boosts = block_on(source.boosts())
            .ok()
            .flatten()
            .map_or_else(Boosts::absent, Boosts::of);
        app.insert_resource(Jumps(Arc::new(JumpGraph::new(&named, &boosts))));
        app.insert_resource(boosts);
        app.insert_resource(Names::reaching(named, reaches));
        app.insert_resource(ResidentIndex(
            block_on(source.index()).expect("a published index"),
        ));
        app.insert_resource(Transport(Arc::new(FsSource::new(dir))));
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

    /// A name published after the table was read is found without a restart
    ///
    /// The names table is a hundred megabytes in chunks, so a refresh reads the
    /// chunk that moved and puts what is new in the overlay rather than
    /// rebuilding the table around it. A system the feed names mid-session is
    /// what that is for.
    #[test]
    fn a_name_published_into_a_moved_chunk_is_found() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        let named = |address: i64, name: &str| NameEntry {
            address,
            name: name.to_owned(),
            position: [0.0, 900.0, 24400.0],
        };
        let mut table = NameTable::from_entries(vec![named(1, "First")]);
        table.publish(&dir.0).expect("the names should publish");

        let mut app = watching(&dir.0, &built);
        assert!(
            app.world().resource::<Names>().get(2).is_none(),
            "nothing names the second system yet"
        );

        // The chunk the arrival lands in is rewritten, and only that chunk.
        std::thread::sleep(std::time::Duration::from_millis(10));
        table.upsert(named(2, "Second"));
        table.publish(&dir.0).expect("the names should publish again");

        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(2)
                .is_some_and(|entry| entry.name == "Second")),
            "the name published mid-session was never picked up",
        );
        assert_eq!(
            app.world().resource::<Names>().address("Second"),
            Some(2),
            "and a search reaches it"
        );
    }

    /// The overlay accumulates arrivals, and a correction answers over the base
    ///
    /// Which is the whole of how it stays right over a session: a chunk is
    /// re-read on nearly every publish, so the merge has to keep what earlier
    /// passes put there, take the entries the base does not already say, and
    /// let a name changed since the base was read win.
    #[test]
    fn the_overlay_keeps_what_earlier_passes_found() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        let named = |address: i64, name: &str| NameEntry {
            address,
            name: name.to_owned(),
            position: [0.0, 900.0, 24400.0],
        };
        let mut table = NameTable::from_entries(vec![named(1, "First")]);
        table.publish(&dir.0).expect("the names should publish");
        let mut app = watching(&dir.0, &built);

        let named_at = |app: &App, address: i64| {
            app.world()
                .resource::<Names>()
                .get(address)
                .map(|entry| entry.name.clone())
        };

        std::thread::sleep(std::time::Duration::from_millis(10));
        table.upsert(named(2, "Second"));
        table.publish(&dir.0).expect("a publish");
        assert!(
            pump(&mut app, |app| named_at(app, 2).is_some()),
            "the first arrival was never picked up"
        );

        // A second arrival, and a correction to the name the base was read
        // with, in the same chunk.
        std::thread::sleep(std::time::Duration::from_millis(10));
        table.upsert(named(3, "Third"));
        table.upsert(named(1, "First, Renamed"));
        table.publish(&dir.0).expect("another publish");
        assert!(
            pump(&mut app, |app| named_at(app, 3).is_some()),
            "the second arrival was never picked up"
        );

        assert_eq!(
            named_at(&app, 2).as_deref(),
            Some("Second"),
            "the earlier arrival is still named"
        );
        assert_eq!(
            named_at(&app, 1).as_deref(),
            Some("First, Renamed"),
            "and a correction answers over what the base was read with"
        );
        let names = app.world().resource::<Names>();
        assert_eq!(names.len(), 3, "three systems named, each counted once");
        assert_eq!(
            names.find("First").len(),
            1,
            "a renamed system is listed once, under its new name"
        );
    }

    /// Startup stamps every part it read, so the first refresh reads nothing
    ///
    /// Left unstamped, a part reads as changed — not knowing is not knowing
    /// that it did not — so the first poll would re-read every table the map
    /// had just read, the hundred-megabyte names table among them, to find
    /// nothing had moved.
    #[test]
    fn startup_stamps_every_part_it_read() {
        use galos_index::source::{
            factions_path, populated_path, reaches_path, write_meta,
        };

        let dir = Scratch::new();
        publish(&dir.0, &[input(1, 0.0)]);
        write_meta(&populated_path(&dir.0), &Vec::<PopulatedSystem>::new())
            .expect("the populated table should write");
        write_meta(&reaches_path(&dir.0), &Vec::<SystemReach>::new())
            .expect("the reaches table should write");
        write_meta(&factions_path(&dir.0), &Vec::<Faction>::new())
            .expect("the factions table should write");
        NameTable::from_entries(vec![NameEntry {
            address: 1,
            name: "First".into(),
            position: [0.0, 900.0, 24400.0],
        }])
        .publish(&dir.0)
        .expect("the names should publish");

        let transport: Arc<dyn IndexSource> = Arc::new(FsSource::new(&dir.0));
        let held = block_on(Held::before_reading(&*transport));

        assert!(held.index.is_some(), "the aggregates");
        assert!(held.populated.is_some(), "the populated table");
        assert!(held.reaches.is_some(), "the reaches table");
        assert!(held.factions.is_some(), "the factions table");
        assert_eq!(
            held.chunks.len(),
            1,
            "every chunk of the names table, and no more"
        );
        assert!(held.chunks.iter().all(Option::is_some));
    }
    /// A system named mid-session becomes routable through
    ///
    /// The router's graph is bucketed off the names table at startup, so a
    /// system the feed named while the map ran was not in it: a route to it
    /// found nothing and a route past it took the long way. The refresh hands
    /// the arrivals to [`JumpGraph::extended`], which is what closes that.
    #[test]
    fn an_arrival_becomes_routable() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0), input(9, 900.0)]);
        let named = |address: i64, at: f32| NameEntry {
            address,
            name: format!("S{address}"),
            position: [at, 0.0, 0.0],
        };
        // Two ends 900 ly apart, and nothing between them on record.
        let mut table =
            NameTable::from_entries(vec![named(1, 0.0), named(9, 900.0)]);
        table.publish(&dir.0).expect("the names should publish");

        let mut app = watching(&dir.0, &built);
        let route = |app: &App| {
            app.world()
                .resource::<Jumps>()
                .0
                .route(1, 9, 500., Routing::Direct, Drive::Unaided, None)
                .map(|path| path.iter().map(|(a, _)| *a).collect::<Vec<_>>())
        };
        assert!(
            route(&app).is_none(),
            "900 ly at a 500 ly range, with nothing in between"
        );

        // The feed names one in the middle.
        std::thread::sleep(std::time::Duration::from_millis(10));
        table.upsert(named(5, 450.0));
        table.publish(&dir.0).expect("a publish");

        assert!(
            pump(&mut app, |app| route(app).is_some()),
            "the arrival never reached the router",
        );
        assert_eq!(
            route(&app),
            Some(vec![1, 5, 9]),
            "the route should run through the system named since"
        );
    }

    /// A chunk re-read with nothing new in it rebuilds nothing
    ///
    /// The tail chunk moves on nearly every publish, so a pass that diffed
    /// against the base alone would find every arrival of the session new
    /// again each time, copy the overlay, and re-bucket the router's graph —
    /// work that grows with the session to take in the nothing that moved.
    #[test]
    fn a_chunk_with_nothing_new_rebuilds_nothing() {
        let dir = Scratch::new();
        let built = publish(&dir.0, &[input(1, 0.0)]);
        let named = |address: i64, name: &str| NameEntry {
            address,
            name: name.to_owned(),
            position: [0.0, 900.0, 24400.0],
        };
        let mut table = NameTable::from_entries(vec![named(1, "First")]);
        table.publish(&dir.0).expect("the names should publish");
        let mut app = watching(&dir.0, &built);

        // An arrival, picked up into the overlay.
        std::thread::sleep(std::time::Duration::from_millis(10));
        table.upsert(named(2, "Second"));
        table.publish(&dir.0).expect("a publish");
        assert!(
            pump(&mut app, |app| app
                .world()
                .resource::<Names>()
                .get(2)
                .is_some()),
            "the arrival was never picked up"
        );
        let overlay = Arc::clone(&app.world().resource::<Names>().fresh);
        let graph = Arc::clone(&app.world().resource::<Jumps>().0);

        // The same chunk published again, holding exactly what it held: the
        // stamp moves, so the pass reads it, and finds nothing to take.
        std::thread::sleep(std::time::Duration::from_millis(10));
        NameTable::from_entries(vec![named(1, "First"), named(2, "Second")])
            .publish(&dir.0)
            .expect("a republish of the same names");
        for _ in 0..40 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(
            Arc::ptr_eq(&overlay, &app.world().resource::<Names>().fresh),
            "the overlay was rebuilt to take in nothing"
        );
        assert!(
            Arc::ptr_eq(&graph, &app.world().resource::<Jumps>().0),
            "the router's graph was re-bucketed to take in nothing"
        );
        assert!(
            app.world().resource::<Names>().get(2).is_some(),
            "and the arrival is still named"
        );
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
            galos_index::Resident::default();
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
}
