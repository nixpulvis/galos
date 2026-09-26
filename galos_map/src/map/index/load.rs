//! Standing the window up while the index is still being read
//!
//! A built index is gigabytes on disk and what the map stands up from it is
//! the cell aggregates, three tables of a couple of million entries each,
//! and the names table — which used to be most of the read: 8.70 GiB of
//! MessagePack decoded across the pool, 33 s at 200 M systems. It is mapped
//! now rather than decoded, so what is left is seconds rather than a minute,
//! and it used to happen before the `App` existed at all: the window itself
//! waited on it, so the map opened with nothing on screen and nothing to say
//! why. Reported as a launch that looks hung.
//!
//! So the window comes up on the first frame and the read runs on a task pool
//! behind a loading screen. Which part it has reached is published as it goes,
//! because "reading the cells" for several seconds is a different thing to a
//! reader than a spinner that never says anything.
//!
//! The map is held off with a state rather than by handing every system an
//! empty table. Bevy skips nothing for a missing resource -- it panics -- so
//! the choice is between placeholders everywhere and one gate, and a
//! placeholder index is a lie the walk would act on: an empty sky drawn as
//! though the galaxy held nothing, a route refused for want of a graph. The
//! gate says the truth, which is that there is no map yet.
//!
//! Two things run either side of it. [`crate::map::space::spawn_map`] stands up the
//! cameras in `Startup`, one of which carries egui's primary context, so it
//! must not be gated: without that context every egui system in the crate
//! fails on its first line. And `MapSet` is gated whole in
//! [`crate::map::schedule`], which covers everything but the two egui passes that
//! read the tables -- [`crate::ui::chrome`] and [`crate::dev::diagnostics`] --
//! and those carry the gate themselves.

use crate::map::index::names;
use crate::map::index::refresh::Held;
use crate::map::index::{
    Factions, IndexDir, Names, Populated, ResidentIndex, Settled, Transport,
};
use bevy::log::tracing::Instrument;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::Index;
use galos_index::read::inhabited::Inhabitance;
use galos_index::records::{Faction, PopulatedSystem, SystemBoost};
use galos_route::Boosts;
use galos_route::graph::Jumps;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

pub fn plugin(app: &mut App) {
    app.init_state::<Opening>();
    app.add_systems(Startup, start);
    // Outside `MapSet`, which is gated on the state this sets: a system that
    // waits for the map to be drawn cannot be the one that says it is.
    app.add_systems(Update, finish.run_if(in_state(Opening::Reading)));
}

/// Whether the index is in hand yet
///
/// The one thing the map cannot be drawn without. Everything else it wants --
/// a system's rows, a route, a name it was asked about -- is fetched while the
/// map runs and drawn when it lands; the index is what says where anything is
/// at all.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opening {
    /// The index is being read off the disk
    #[default]
    Reading,
    /// It is in hand, and the map draws
    Drawn,
}

/// How far the read has got
///
/// Published from the task as it goes and read by the loading screen. In the
/// order the parts are read, so the reader watching it sees it run down the
/// list rather than jump about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    Stamps,
    Cells,
    Populated,
    Names,
    Reaches,
    Boosts,
    Factions,
    Jumps,
}

impl Step {
    /// What is being read, in the words the map uses for it elsewhere
    pub(crate) fn said(self) -> &'static str {
        match self {
            Step::Stamps => "what the directory holds",
            Step::Cells => "the cells",
            Step::Populated => "the populated systems",
            Step::Names => "the names",
            Step::Reaches => "how far systems reach",
            Step::Boosts => "the supercharge table",
            Step::Factions => "the factions",
            Step::Jumps => "the jumps a ship can make",
        }
    }

    /// The step `said` stands for, for the reader on the other thread
    ///
    /// A number rather than the value itself, an [`AtomicU8`] being the whole
    /// of what a step has to be: one writer, one reader, and nothing either
    /// of them does depends on catching every value on the way past.
    fn from(said: u8) -> Step {
        match said {
            1 => Step::Cells,
            2 => Step::Populated,
            3 => Step::Names,
            4 => Step::Reaches,
            5 => Step::Boosts,
            6 => Step::Factions,
            7 => Step::Jumps,
            _ => Step::Stamps,
        }
    }
}

/// The read under way
#[derive(Resource)]
pub(crate) struct Reading {
    task: Task<Result<Loaded, String>>,
    /// Which part it has reached, written from the task
    step: Arc<AtomicU8>,
    /// What went wrong, where something did
    ///
    /// Kept rather than panicked on. A directory that is not there is the
    /// commonest thing to get wrong about running the map, and a backtrace out
    /// of a task pool thread is a poor way to be told which path was tried.
    failed: Option<String>,
}

impl Reading {
    /// Which part the read has reached
    pub(crate) fn step(&self) -> Step {
        Step::from(self.step.load(Ordering::Relaxed))
    }

    /// What went wrong, where something did
    pub(crate) fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }
}

/// Everything the map is stood up with, built and ready to be handed over
///
/// The tables as the map holds them rather than as they were read. What is
/// built from them is the heavy half of opening -- a name lookup and a jump
/// graph are a couple of million entries each -- and building it here means
/// the frame that takes the read in only has to hand the resources over.
struct Loaded {
    held: Held,
    index: ResidentIndex,
    populated: Populated,
    settled: Settled,
    names: Names,
    boosts: Boosts,
    factions: Factions,
    jumps: Jumps,
}

/// Set the read going, on the frame the window comes up
fn start(
    mut commands: Commands,
    transport: Res<Transport>,
    dir: Res<IndexDir>,
) {
    let source = transport.0.clone();
    let step = Arc::new(AtomicU8::new(Step::Stamps as u8));
    let saying = Arc::clone(&step);
    let dir = dir.0.clone();

    // The whole of opening as one zone, on whichever pool thread runs it.
    // Wrapped around the future rather than entered inside it: the guard
    // `entered()` hands back is not `Send`, and a future holding one across an
    // await is one the task pool will not take. Instrumenting enters the span
    // on each poll instead, which for a read that never yields — every
    // `Source` method behind it is a blocking file read in an `async fn` — is
    // one zone opened and closed on the one thread, as a profiler's zone has
    // to be.
    let task = AsyncComputeTaskPool::get().spawn(
        async move { read(&source, &saying, &dir).await }
            .instrument(info_span!("index read")),
    );

    commands.insert_resource(Reading { task, step, failed: None });
}

/// Read the whole of it, saying which part is being read as it goes
///
/// The stamps come first, before a byte is read. A publish landing during the
/// read is then held under the older stamp and re-read on the first poll;
/// stamped afterwards, a part read before the publish would be filed under the
/// stamp of the publish and never asked for again. See
/// [`Held::before_reading`].
async fn read(
    source: &Arc<dyn galos_index::Source>,
    step: &Arc<AtomicU8>,
    dir: &str,
) -> Result<Loaded, String> {
    let at = |reached: Step| step.store(reached as u8, Ordering::Relaxed);

    at(Step::Stamps);
    let held = Held::before_reading(&**source).await;

    at(Step::Cells);
    let index = source
        .index()
        .await
        .map_err(|e| format!("reading the index at {dir}: {e}"))?;

    at(Step::Populated);
    let populated = source.populated().await.unwrap_or_default();
    at(Step::Names);
    // One call, and nothing decoded: the table is a file the client maps.
    //
    // This was the heaviest part of opening by a long way. A galaxy's names
    // were 8.70 GiB of MessagePack over 3,053 chunks, which had to be read,
    // decoded and packed across the whole task pool to be had in 33 s and
    // 7.9 GB of resident arrays. [`galos_index::Names::open`] maps the five
    // sections of the published base and reads the delta log, so what a
    // session touches is what the kernel pages in and the rest costs
    // nothing. See [`crate::map::index::names`].
    //
    // Refused rather than read as an empty galaxy: a directory that has
    // published no names opens as the empty table, so an error here is a
    // head this build of the map does not know or a section that is not the
    // length it claims, and saying which path said so is the whole point of
    // the failed-read screen.
    let table = source
        .names()
        .await
        .map_err(|e| format!("reading the names table at {dir}: {e}"))?;
    // And the text a search sweeps read once, in order, here rather than
    // on the first query: a cold sweep faults 128 MB a page at a time and
    // was reported as a four-second search for `SOL`. See
    // [`galos_index::Names::warm`] for what it leaves cold, which is every
    // other section — the point of the format is not reading those.
    //
    // Best effort: a warming read that failed is a slow first search, not
    // a directory that cannot be opened, and the table has already been
    // mapped by the line above.
    if let Err(err) = table.warm() {
        warn!("the names text could not be read ahead: {err}");
    }

    at(Step::Reaches);
    let reaches =
        names::Reaches::of(source.reaches().await.unwrap_or_default());
    at(Step::Boosts);
    let boosts = source.boosts().await.unwrap_or_default();
    at(Step::Factions);
    let factions = source.factions().await.unwrap_or_default();

    at(Step::Jumps);
    Ok(stood_up(dir, held, index, populated, table, reaches, boosts, factions))
}

/// Take the read in once it lands, and let the map draw
fn finish(
    mut commands: Commands,
    mut reading: ResMut<Reading>,
    mut opening: ResMut<NextState<Opening>>,
) {
    if reading.failed.is_some() {
        return;
    }
    let Some(found) = block_on(future::poll_once(&mut reading.task)) else {
        return;
    };
    let Loaded {
        held,
        index,
        populated,
        settled,
        names,
        boosts,
        factions,
        jumps,
    } = match found {
        Ok(loaded) => loaded,
        Err(said) => {
            error!("galos: {said}");
            reading.failed = Some(said);
            return;
        }
    };

    commands.insert_resource(held);
    commands.insert_resource(index);
    commands.insert_resource(populated);
    commands.insert_resource(settled);
    commands.insert_resource(names);
    commands.insert_resource(boosts);
    commands.insert_resource(factions);
    commands.insert_resource(jumps);
    // All of them together, and the gate opened after: the first frame the map
    // draws is a frame in which every one of them answers.
    commands.remove_resource::<Reading>();
    opening.set(Opening::Drawn);
}

/// Work up what was read into what the map holds
///
/// All of it together, since the tables are read against each other: a name
/// is looked up beside the reach it was published with, and a route is walked
/// over the graph built from both. Half of them would be a map answering out
/// of two different reads.
#[allow(clippy::too_many_arguments)]
fn stood_up(
    dir: &str,
    held: Held,
    index: Index,
    populated: Vec<PopulatedSystem>,
    table: galos_index::Names,
    reaches: names::Reaches,
    boosts: Option<Vec<SystemBoost>>,
    factions: Vec<Faction>,
) -> Loaded {
    info!(
        "index {dir} has {} cells, {} populated, {} names, {} reaches, \
         {} supercharging, {} factions",
        index.len(),
        populated.len(),
        table.len(),
        reaches.len(),
        boosts.as_ref().map_or(0, Vec::len),
        factions.len(),
    );
    // A cell tree with no metadata beside it is a stale or half-written build:
    // the map would draw every system uncolored and unnamed rather than say so.
    // Loud, rather than a plausible-but-wrong sky.
    if !index.is_empty() && (populated.is_empty() || table.is_empty()) {
        warn!(
            "{dir} has cells but no metadata sidecars; systems will be \
             uncolored and unnamed. Rebuild the index with `cargo run \
             --bin galos -- ingest --from database --index {dir}`."
        );
    }

    // Which systems can supercharge a drive, which the router plots by and the
    // graph below is built against. Absent where the index publishes no such
    // table, which is not a galaxy without jet cones: the form refuses a
    // supercharged route rather than handing back the unaided one under its
    // name. See [`Boosts::published`].
    let boosts = boosts.map_or_else(Boosts::absent, Boosts::of);
    if !index.is_empty() && !boosts.published() {
        warn!(
            "{dir} publishes no supercharge table this can read, so routes \
             for a supercharging drive cannot be plotted. A table written \
             before its rows carried the place they sit at reads as absent \
             and wants bringing forward, which any `galos index` run over \
             the directory does on the way past. An index built from the \
             database takes a fresh one from `cargo run --bin galos -- \
             ingest --from database --index {dir} --only boosts`; one \
             written from a feed or a journal writes its own on the next \
             publish."
        );
    }

    // The galaxy as the router reads it: the cell payloads, mapped where
    // they lie. Opening it is reading the index file this already holds and
    // nothing else — no places are copied and no grid is built, which is
    // what a route used to wait 32 s and 13.7 GB for. A directory this
    // process cannot map leaves it absent and nothing routes.
    let sky = match galos_index::Sky::open(std::path::Path::new(dir)) {
        Ok(sky) => Some(Arc::new(sky)),
        Err(err) => {
            warn!("{dir} cannot be mapped for routing: {err}");
            None
        }
    };

    // The political aggregation, rolled up here rather than fetched: every
    // populated row contributes to each cell standing over it, so a cell
    // carries the colonies in its whole subtree and a far political view needs
    // nothing loaded. Built before the rows are folded into their map, which
    // is the one place both the tree and the flat table are in hand.
    let settled = Settled(Arc::new(Inhabitance::of(&index, populated.iter())));

    Loaded {
        held,
        jumps: sky.clone().map_or_else(Jumps::default, Jumps::over),
        index: ResidentIndex(index),
        populated: Populated(Arc::new(
            populated.into_iter().map(|s| (s.address, s)).collect(),
        )),
        settled,
        names: Names::packed(table, reaches, sky),
        boosts,
        factions: Factions(
            factions.into_iter().map(|f| (f.id, f.name)).collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::schedule::MapSet;
    use bevy::state::app::StatesPlugin;

    /// The map does not run until the index is in hand
    ///
    /// Which is the whole of what the state is for: every one of the map's
    /// systems reads a table the read has not delivered yet, and bevy panics
    /// on a missing resource rather than skipping the system that wants it.
    #[test]
    fn the_map_waits_for_the_index() {
        #[derive(Resource, Default)]
        struct Ran(usize);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_state::<Opening>();
        app.init_resource::<Ran>();
        app.add_plugins(crate::map::schedule::plugin);
        app.add_systems(
            Update,
            (|mut ran: ResMut<Ran>| ran.0 += 1).in_set(MapSet::Populate),
        );

        app.update();
        assert_eq!(app.world().resource::<Ran>().0, 0, "the map ran early");

        app.world_mut()
            .resource_mut::<NextState<Opening>>()
            .set(Opening::Drawn);
        app.update();
        app.update();

        assert!(
            app.world().resource::<Ran>().0 > 0,
            "the map never ran once the index was in hand"
        );
    }
}
