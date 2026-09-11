//! Standing the window up while the index is still being read
//!
//! The index is a hundred and thirty megabytes on disk, most of it the names
//! table, and what is built from it is three maps of a couple of million
//! entries each. That is seconds of work, and it used to happen before the
//! `App` existed at all: the window itself waited on it, so the map opened
//! with nothing on screen and nothing to say why. Reported as a launch that
//! looks hung.
//!
//! So the window comes up on the first frame and the read runs on a task pool
//! behind a loading screen. Which part it has reached is published as it goes,
//! because "reading the names" for eight seconds is a different thing to a
//! reader than a spinner that never says anything.
//!
//! The map is held off with a state rather than by handing every system an
//! empty table. Bevy skips nothing for a missing resource -- it panics -- so
//! the choice is between placeholders everywhere and one gate, and a
//! placeholder index is a lie the walk would act on: an empty sky drawn as
//! though the galaxy held nothing, a route refused for want of a graph. The
//! gate says the truth, which is that there is no map yet.
//!
//! Two things run either side of it. [`crate::space::spawn_map`] stands up the
//! cameras in `Startup`, one of which carries egui's primary context, so it
//! must not be gated: without that context every egui system in the crate
//! fails on its first line. And `MapSet` is gated whole in
//! [`crate::schedule`], which covers everything but the two egui passes that
//! read the tables -- [`crate::ui::chrome`] and [`crate::dev::diagnostics`] --
//! and those carry the gate themselves.

use crate::refresh::Held;
use crate::systems::route::graph::{JumpGraph, Jumps};
use crate::{
    Boosts, Factions, IndexDir, Names, Populated, ResidentIndex, Transport,
};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use galos_index::meta::{Faction, NameEntry, PopulatedSystem, SystemReach};
use galos_index::{Index, SystemBoost};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

pub fn plugin(app: &mut App) {
    app.init_state::<Opening>();
    app.add_systems(Startup, start);
    // Outside `MapSet`, which is gated on the state this sets: a system that
    // waits for the map to be drawn cannot be the one that says it is.
    app.add_systems(Update, finish.run_if(in_state(Opening::Reading)));
    app.add_systems(
        EguiPrimaryContextPass,
        screen
            .run_if(in_state(Opening::Reading))
            // After the lettering the map is drawn in, which is set on the
            // context once and which this borrows rather than styling itself.
            .after(crate::ui::lettering),
    );
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
enum Step {
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
    fn said(self) -> &'static str {
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
struct Reading {
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

    let task = AsyncComputeTaskPool::get()
        .spawn(async move { read(&source, &saying, &dir).await });

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
    let names = source.names().await.unwrap_or_default();
    at(Step::Reaches);
    let reaches = source.reaches().await.unwrap_or_default();
    at(Step::Boosts);
    let boosts = source.boosts().await.unwrap_or_default();
    at(Step::Factions);
    let factions = source.factions().await.unwrap_or_default();

    // Not read but built, out of the two largest tables there are. On this
    // side of the gate because it is the same seconds of work either way, and
    // over here they are seconds the loading screen is already saying
    // something about rather than a frame the map hangs for.
    at(Step::Jumps);
    Ok(stood_up(dir, held, index, populated, names, reaches, boosts, factions))
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
    let Loaded { held, index, populated, names, boosts, factions, jumps } =
        match found {
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
    names: Vec<NameEntry>,
    reaches: Vec<SystemReach>,
    boosts: Option<Vec<SystemBoost>>,
    factions: Vec<Faction>,
) -> Loaded {
    info!(
        "index {dir} has {} cells, {} populated, {} names, {} reaches, \
         {} supercharging, {} factions",
        index.len(),
        populated.len(),
        names.len(),
        reaches.len(),
        boosts.as_ref().map_or(0, Vec::len),
        factions.len(),
    );
    // A cell tree with no metadata beside it is a stale or half-written build:
    // the map would draw every system uncolored and unnamed rather than say so.
    // Loud, rather than a plausible-but-wrong sky.
    if !index.is_empty() && (populated.is_empty() || names.is_empty()) {
        warn!(
            "{dir} has cells but no metadata sidecars; systems will be \
             uncolored and unnamed. Rebuild the index with \
             `cargo run --bin galos-sync -- db --to index={dir}`."
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
            "{dir} publishes no supercharge table, so routes for a \
             supercharging drive cannot be plotted. An index built from \
             the database takes one from `cargo run --bin galos-sync -- \
             db --to index={dir} --only boosts`; one written from a feed \
             or a journal writes its own on the next publish."
        );
    }

    Loaded {
        held,
        // The jump graph the router walks, bucketed once from the names.
        jumps: Jumps(Arc::new(JumpGraph::new(&names, &boosts))),
        index: ResidentIndex(index),
        populated: Populated(Arc::new(
            populated.into_iter().map(|s| (s.address, s)).collect(),
        )),
        names: Names::reaching(names, reaches),
        boosts,
        factions: Factions(
            factions.into_iter().map(|f| (f.id, f.name)).collect(),
        ),
    }
}

/// Say that the map is coming, and what it is waiting on
fn screen(
    mut contexts: EguiContexts,
    reading: Option<Res<Reading>>,
    dir: Res<IndexDir>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let step = reading
        .as_ref()
        .map_or(Step::Stamps, |it| Step::from(it.step.load(Ordering::Relaxed)));
    let failed = reading.as_ref().and_then(|it| it.failed.clone());

    egui::Area::new(egui::Id::new("loading"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| waiting(ui, &dir.0, step, failed.as_deref()));

    Ok(())
}

/// The words on the loading screen
///
/// The directory first, since which index is being read is the thing a reader
/// with two of them wants to know, and it is the answer to the commonest way
/// of getting this wrong. Then what is happening: the part being read while
/// the read is going, and what went wrong where it did not.
fn waiting(ui: &mut egui::Ui, dir: &str, step: Step, failed: Option<&str>) {
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(dir).weak());
        match failed {
            Some(said) => {
                ui.label(egui::RichText::new(said).color(egui::Color32::RED));
            }
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("Reading {}", step.said()));
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::MapSet;
    use crate::tests::words;
    use bevy::state::app::StatesPlugin;

    /// The screen says which index it is reading and what it has reached
    ///
    /// A spinner alone says the map is busy, which the black window said
    /// already. What a reader wants is which of their directories is being
    /// read and why it is taking this long, and the names table is the answer
    /// to the second for as long as it takes to read a hundred megabytes.
    #[test]
    fn the_screen_says_what_it_is_reading() {
        let said = words(|ui| waiting(ui, ".galos_index", Step::Names, None));

        assert!(said.contains(&".galos_index".to_owned()), "{said:?}");
        assert!(said.contains(&"Reading the names".to_owned()), "{said:?}");
    }

    /// And says what went wrong rather than spinning at nothing
    ///
    /// A directory that is not there is the commonest thing to get wrong about
    /// running the map. Read on a task pool thread, the panic that used to say
    /// so would be a backtrace with the path buried in it.
    #[test]
    fn the_screen_says_what_went_wrong() {
        let said = words(|ui| {
            waiting(ui, "/nowhere", Step::Cells, Some("no such directory"))
        });

        assert!(said.contains(&"no such directory".to_owned()), "{said:?}");
        assert!(
            !said.iter().any(|line| line.starts_with("Reading")),
            "a failed read said it was still going: {said:?}"
        );
    }

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
        app.add_plugins(crate::schedule::plugin);
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
