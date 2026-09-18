//! What flying the map costs a frame, measured against a real directory.
//!
//! The walk's four systems — [`super::bounded::fetch`],
//! [`super::bounded::collect`], [`super::bounded::reconcile`],
//! [`super::bounded::evict_payloads`] — and the queue drain behind them
//! ([`super::spawn::drain_spawns`]) are what a zoom or a pan runs through, and
//! a tracy capture of the galaxy view put them at most of a 19–30 ms frame.
//! A capture cannot be driven from a script, though, and a still camera
//! exercises none of it: what the map does while the view moves is the case
//! worth measuring and the one nobody can hold still to read.
//!
//! So this flies a camera on rails over a built index and runs the real
//! systems against it, one registered system at a time, timing each and
//! counting what it did:
//!
//! - **asked**: payload reads started that frame, which is what the transport
//!   is put to
//! - **held**: cells resident afterwards, and **freed**, what the payload
//!   evictor took off
//! - **queued** and **spawned**: the spawn queue's depth and what the frame's
//!   budget got through
//! - **drawn**: systems on the map, and **dropped**, what the walk queued to
//!   evict
//!
//! A cell asked for twice over one flight is the number to watch: a payload
//! read, freed and read again is work the map paid for twice and drew once.
//!
//! Stands down without `GALOS_PERF_DIR` naming a built index directory, as
//! [`super::route::perf`] does:
//!
//! ```sh
//! GALOS_PERF_DIR=.galos_index cargo test -p galos_map --lib flight -- --nocapture
//! ```
//!
//! The clocks are loose, this running on whatever machine is to hand; the
//! counts are not.

use crate::camera::OrbitCamera;
use crate::space::Galaxy;
use crate::systems::bounded::{BoundedTasks, LodFetch, ResidentCells};
use crate::systems::filter::{Cut, DimTo, Filters};
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::spawn::PendingSpawns;
use crate::systems::{PendingEvictions, Spyglass, System};
use crate::{Names, Populated, ResidentIndex, Transport};
use bevy::ecs::system::SystemId;
use bevy::log::tracing::Subscriber;
use bevy::log::tracing::span::Id;
use bevy::log::tracing_subscriber::Registry;
use bevy::log::tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use bevy::log::tracing_subscriber::registry::LookupSpan;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::{CellId, FsSource};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a frame is told it took, so the poll and the throttle behave as
/// they would at sixty frames a second.
const FRAME: Duration = Duration::from_millis(16);

/// How many frames each step of the rails is held for, so the flight runs at
/// the rate a hand moves rather than a step a frame.
const DWELL: usize = 6;

/// How many frames the flight holds still at the end, so the fill-in after a
/// stop is measured rather than assumed.
const SETTLE: usize = 90;

/// The directory to measure against, or [`None`] to stand down.
fn measured() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("GALOS_PERF_DIR").ok()?);
    if !dir.join("index.bin").exists() {
        eprintln!("{}: not a built index; standing down", dir.display());
        return None;
    }
    Some(dir)
}

/// What the phase spans inside the systems cost, summed over the flight
///
/// The systems carry `info_span!`s over their own phases — the set arithmetic
/// apart from the asking, the prefix loop apart from the eviction scan — for
/// a profiler to read. A profiler cannot be driven from a test, so the same
/// spans are read here instead: a layer that adds up the time between each
/// span's enter and exit, which is what a zone in a capture is.
#[derive(Clone, Default)]
struct Phases(Arc<Mutex<HashMap<&'static str, (Duration, usize)>>>);

impl Phases {
    /// Every span that was entered, dearest first
    fn table(&self) -> Vec<(&'static str, Duration, usize)> {
        let held = self.0.lock().expect("the phase table");
        let mut rows: Vec<(&'static str, Duration, usize)> = held
            .iter()
            .map(|(name, (took, count))| (*name, *took, *count))
            .collect();
        rows.sort_unstable_by_key(|(_, took, _)| std::cmp::Reverse(*took));
        rows
    }
}

/// When a span was entered, kept in the span's own extensions as
/// `tracing-tracy` keeps its zone.
struct Entered(Instant);

impl<S> Layer<S> for Phases
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().replace(Entered(Instant::now()));
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let took = span
            .extensions()
            .get::<Entered>()
            .map(|entered| entered.0.elapsed());
        let Some(took) = took else { return };
        let mut held = self.0.lock().expect("the phase table");
        let row = held.entry(span.name()).or_insert((Duration::ZERO, 0));
        row.0 += took;
        row.1 += 1;
    }
}

/// The five systems of the walk, registered so they can be run one at a time
/// and timed apart.
struct Flight {
    app: App,
    reach: SystemId,
    plan: SystemId,
    fetch: SystemId,
    collect: SystemId,
    reconcile: SystemId,
    evict: SystemId,
    drain: SystemId,
    drop: SystemId,
    /// Every cell a read has ever been started for, so a second read of one
    /// is countable.
    asked: HashSet<CellId>,
    /// How many of those were cells already read once before
    reasked: usize,
}

/// What one frame of the flight did.
#[derive(Default)]
struct Frame {
    took: Vec<(&'static str, Duration)>,
    marks: usize,
    asked: usize,
    held: usize,
    freed: usize,
    queued: usize,
    drawn: usize,
    dropped: usize,
    points: usize,
    /// Which leg of the rails this frame belongs to
    leg: &'static str,
    /// Systems despawned since the map opened, for a per-leg difference
    despawned: u64,
    /// Payload reads started this frame, summed per leg
    reads: usize,
}

impl Frame {
    fn of(&self, phase: &str) -> Duration {
        self.took
            .iter()
            .find(|(name, _)| *name == phase)
            .map_or(Duration::ZERO, |(_, took)| *took)
    }

    fn whole(&self) -> Duration {
        self.took.iter().map(|(_, took)| *took).sum()
    }
}

impl Flight {
    /// A world holding the real index and transport, a camera that can say
    /// what it sees, and the galaxy grid the stars are placed in.
    fn over(dir: &PathBuf) -> Flight {
        let source = FsSource::new(dir);
        let index = pollster::block_on(async {
            use galos_index::Source as _;
            source.index().await.expect("the index should read")
        });

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        app.insert_resource(ResidentIndex(index));
        app.insert_resource(Transport(std::sync::Arc::new(source)));
        app.insert_resource(Populated::default());
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.insert_resource(View::Map);
        app.insert_resource(ScalePopulation(false));
        app.insert_resource(LodFetch(true));
        app.insert_resource(Spyglass {
            radius: 100.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(crate::systems::aggregate::Planned(
            galos_index::Needed {
                mode: galos_index::Mode::Shell,
                marks: Vec::new(),
                splats: Vec::new(),
            },
        ));
        app.init_resource::<ResidentCells>();
        app.init_resource::<BoundedTasks>();
        app.init_resource::<crate::systems::bounded::PointOrders>();
        app.init_resource::<crate::systems::bounded::Republished>();
        app.init_resource::<crate::systems::bounded::Keeping>();
        app.init_resource::<crate::refresh::Held>();
        app.init_resource::<PendingSpawns>();
        app.init_resource::<PendingEvictions>();
        app.init_resource::<crate::systems::Evictions>();
        app.init_resource::<crate::systems::bodies::spawn::HeldSystem>();
        app.init_resource::<crate::systems::selection::Selection>();
        app.init_resource::<Filters>();
        app.init_resource::<DimTo>();
        app.init_resource::<Cut>();

        let galaxy = app
            .world_mut()
            .spawn((
                big_space::prelude::BigSpace::default(),
                crate::space::galaxy_grid(),
            ))
            .id();
        app.insert_resource(Galaxy(galaxy));
        app.world_mut().spawn((
            OrbitCamera::stood_back(100.),
            // The reach follows the camera through this, and `framed` reads
            // how wide the viewer sees off it.
            Projection::Perspective(PerspectiveProjection::default()),
            crate::systems::tests::seeing(),
        ));

        let world = app.world_mut();
        let reach = world.register_system(crate::systems::reach_with_camera);
        let plan = world.register_system(crate::systems::aggregate::plan);
        let fetch = world.register_system(crate::systems::bounded::fetch);
        let collect = world.register_system(crate::systems::bounded::collect);
        let reconcile =
            world.register_system(crate::systems::bounded::reconcile);
        let evict =
            world.register_system(crate::systems::bounded::evict_payloads);
        let drain = world.register_system(crate::systems::spawn::drain_spawns);
        let drop = world.register_system(crate::systems::drain_evictions);

        Flight {
            app,
            reach,
            plan,
            fetch,
            collect,
            reconcile,
            evict,
            drain,
            drop,
            asked: HashSet::new(),
            reasked: 0,
        }
    }

    /// Put the camera `back` light years from `at` and run one frame.
    ///
    /// The reads a frame starts are awaited before the frame after it, so the
    /// flight measures a map whose transport keeps up rather than one racing
    /// a task pool: what is being measured is the per-frame work, and a read
    /// that lands two frames late would move it to whichever frame caught it.
    fn frame(&mut self, at: DVec3, back: f32) -> Frame {
        {
            let world = self.app.world_mut();
            world.resource_mut::<Time<Real>>().advance_by(FRAME);
            let mut cameras = world.query::<&mut OrbitCamera>();
            for mut camera in cameras.iter_mut(world) {
                camera.radius = back;
                camera.target_radius = back;
                camera.looks_at(at);
                // The eye is a field the camera's own system writes, not
                // something the radius implies, so a flight that only set the
                // radius left the plan's key — and the walk — where it was.
                camera.stands_at(at + DVec3::new(0., 0., back as f64));
            }
        }

        let mut frame = Frame::default();
        let held_before = self.app.world().resource::<ResidentCells>().0.len();

        for (name, id) in [
            ("reach", self.reach),
            ("plan", self.plan),
            ("fetch", self.fetch),
            ("collect", self.collect),
            ("reconcile", self.reconcile),
            ("evict", self.evict),
            ("drain", self.drain),
            // Without this the map never loses an entity: the eviction queue
            // fills, every drawn system stays drawn, and `reconcile`'s own
            // scan grows over a flight until it is measuring the harness.
            ("drop", self.drop),
        ] {
            let at = Instant::now();
            self.app.world_mut().run_system(id).expect("the system runs");
            frame.took.push((name, at.elapsed()));

            // The reads this frame started, counted as they are asked for
            // rather than afterwards: a read that lands inside the frame is
            // gone from the task map by the time the frame ends.
            if name == "fetch" {
                let world = self.app.world();
                let asking: Vec<CellId> =
                    world.resource::<BoundedTasks>().cells().collect();
                frame.asked = asking.len();
                for id in asking {
                    if !self.asked.insert(id) {
                        self.reasked += 1;
                    }
                }
                // Await them, so `collect` has something to take and the
                // frame's own reads are the frame's own cost.
                self.await_reads();
            }
        }

        let world = self.app.world_mut();
        frame.marks = world
            .resource::<crate::systems::aggregate::Planned>()
            .0
            .marks
            .len();
        frame.held = world.resource::<ResidentCells>().0.len();
        frame.freed = held_before.saturating_sub(frame.held);
        frame.queued = world.resource::<PendingSpawns>().queued();
        frame.dropped = world.resource::<PendingEvictions>().0.len();
        frame.drawn = world.query::<&System>().iter(world).count();
        frame.despawned = world.resource::<crate::systems::Evictions>().total;
        frame.reads = frame.asked;
        frame.points = world
            .resource::<ResidentCells>()
            .0
            .iter()
            .map(|(_, cell)| cell.points.len())
            .sum();
        frame
    }

    /// Hold until every read started has landed, so a frame's reads are its
    /// own rather than the next frame's windfall.
    fn await_reads(&mut self) {
        let world = self.app.world_mut();
        while !world.resource::<BoundedTasks>().is_empty() {
            std::thread::sleep(Duration::from_millis(1));
            world.run_system(self.collect).expect("the system runs");
        }
    }
}

/// What a flight from a system out to the whole galaxy and back costs
///
/// The rails: out from Sol in a geometric ramp to fifty thousand light years,
/// then back in, then a pan across the disc at a middling zoom — a zoom, a
/// zoom back and a drag, which is the whole of how the map is flown.
#[test]
fn flying_stays_quick() {
    let Some(dir) = measured() else { return };
    // The phase spans, read through a subscriber of our own. Set once for the
    // process, which is why the guard is the only test that installs one.
    let phases = Phases::default();
    let _ = bevy::log::tracing::subscriber::set_global_default(
        Registry::default().with(phases.clone()),
    );
    let mut flight = Flight::over(&dir);

    // Where the map opens, which is also where the first frames pay for the
    // cells nearest Sol.
    let sol = DVec3::ZERO;

    // Each step held for [`DWELL`] frames, so a zoom takes about a second of
    // them as a hand on a wheel does. A step-a-frame flight is over inside
    // the grace a payload is kept for, which measures neither the grace nor
    // the ceiling under it.
    let mut rails: Vec<(DVec3, f32, &'static str)> = Vec::new();
    let mut back = 100f32;
    while back < 50_000. {
        rails.extend(std::iter::repeat_n((sol, back, "out"), DWELL));
        back *= 1.5;
    }
    while back > 100. {
        rails.extend(std::iter::repeat_n((sol, back, "in"), DWELL));
        back /= 1.5;
    }
    // A drag across the disc at a middling zoom, in five hundred light year
    // steps, which is about what a second of dragging covers.
    for step in 0..24 {
        let at = DVec3::new(step as f64 * 500., 0., 0.);
        rails.extend(std::iter::repeat_n((at, 2_000., "pan"), DWELL));
    }
    // And the hand comes off the mouse. What the map does with a view that has
    // stopped is the other half of the question the spawn budget answers, and
    // no amount of flying measures it: the fill-in is what a reader watches
    // arrive after they stop.
    // A drag through the densest sky there is: the core, twenty-five thousand
    // light years out, where a cell holds thousands of systems rather than
    // hundreds. A pan sheds one edge and takes on the other, so what it costs
    // is what the new edge weighs — and near the core an edge weighs far more.
    for step in 0..24 {
        let at = DVec3::new(step as f64 * 500., 0., 25_000.);
        rails.extend(std::iter::repeat_n((at, 2_000., "core"), DWELL));
    }
    // Back at Sol and wide, which is a view that wants tens of thousands of
    // systems rather than the few hundred the drag ends on: a stop nobody has
    // to wait for measures nothing.
    rails.extend(std::iter::repeat_n((sol, 5_000., "still"), SETTLE));

    println!(
        "\n{:>5} {:>6} {:>8} {:>6} {:>6} {:>6} {:>7} {:>6} {:>7}  {}",
        "#",
        "back",
        "frame",
        "marks",
        "asked",
        "held",
        "freed",
        "drawn",
        "dropped",
        "phases",
    );
    let mut whole = Duration::ZERO;
    let mut frames = Vec::new();
    for (n, (at, back, leg)) in rails.iter().enumerate() {
        let mut frame = flight.frame(*at, *back);
        whole += frame.whole();
        println!(
            "{n:>5} {back:>6.0} {:>8.2?} {:>6} {:>6} {:>6} {:>7} {:>6} {:>7}  \
             plan {:>7.2?} fetch {:>7.2?} collect {:>7.2?} reconcile {:>7.2?} \
             evict {:>7.2?} drain {:>7.2?} drop {:>7.2?}  {leg}",
            frame.whole(),
            frame.marks,
            frame.asked,
            frame.held,
            frame.freed,
            frame.drawn,
            frame.dropped,
            frame.of("plan"),
            frame.of("fetch"),
            frame.of("collect"),
            frame.of("reconcile"),
            frame.of("evict"),
            frame.of("drain"),
            frame.of("drop"),
        );
        frame.leg = leg;
        frames.push(frame);
    }

    let stages = [
        "reach",
        "plan",
        "fetch",
        "collect",
        "reconcile",
        "evict",
        "drain",
        "drop",
    ];
    println!("\nphase        total     mean      max");
    for phase in stages {
        let each: Vec<Duration> =
            frames.iter().map(|frame| frame.of(phase)).collect();
        let total: Duration = each.iter().sum();
        println!(
            "{phase:<10} {total:>8.2?} {:>8.2?} {:>8.2?}",
            total / each.len() as u32,
            each.iter().max().copied().unwrap_or_default(),
        );
    }
    // The fill-in: frames from the stop until the walk has nothing left to
    // offer, and what the map was drawing by then.
    // What the last frame held against what it could have drawn from: the
    // payload is magnitude-ordered and the draw takes a prefix of each cell,
    // but the fetch reads the whole file. The difference is read and held for
    // nothing.
    {
        let world = flight.app.world_mut();
        let mut cameras =
            world.query::<(&crate::camera::OrbitCamera, &Camera)>();
        let seen = cameras.iter(world).next().and_then(|(orbit, camera)| {
            crate::systems::aggregate::view(orbit, camera)
        });
        if let Some(view) = seen {
            let index = world.resource::<ResidentIndex>();
            let resident = world.resource::<ResidentCells>();
            let mut held = 0usize;
            let mut wanted = 0usize;
            let mut deepest = (0usize, 0usize);
            for (id, cell) in resident.0.iter() {
                let Some(indexed) = index.0.get(id) else { continue };
                let target = (galos_index::resolvable_count(
                    indexed,
                    &view,
                    galos_index::MARK_SEPARATION_PX,
                ) as usize)
                    .min(cell.points.len());
                held += cell.points.len();
                wanted += target;
                if cell.points.len() > deepest.0 {
                    deepest = (cell.points.len(), target);
                }
            }
            println!(
                "\nread amplification at the stop: {held} points held, \
                 {wanted} inside a prefix the draw would take ({:.0}× over)\
                 \n  deepest cell: {} points, {} of them drawable",
                held as f64 / wanted.max(1) as f64,
                deepest.0,
                deepest.1,
            );
        }
    }

    println!(
        "\nleg    frames   p50     p90     spawned  despawned   reads  drawn"
    );
    let mut legs: Vec<&'static str> = Vec::new();
    for frame in &frames {
        if !legs.contains(&frame.leg) {
            legs.push(frame.leg);
        }
    }
    for leg in legs {
        let of: Vec<&Frame> =
            frames.iter().filter(|frame| frame.leg == leg).collect();
        let mut each: Vec<Duration> = of.iter().map(|f| f.whole()).collect();
        each.sort_unstable();
        let despawned = of.last().map_or(0, |f| f.despawned)
            - of.first().map_or(0, |f| f.despawned);
        // What the leg spawned: everything it dropped, plus however much
        // larger the map ended than it started.
        let grew = of.last().map_or(0, |f| f.drawn) as i64
            - of.first().map_or(0, |f| f.drawn) as i64;
        let spawned = despawned as i64 + grew;
        println!(
            "{leg:<6} {:>6} {:>7.2?} {:>7.2?} {spawned:>8} {despawned:>10} \
             {:>7} {:>6}",
            of.len(),
            each[each.len() / 2],
            each[each.len() * 9 / 10],
            of.iter().map(|f| f.reads).sum::<usize>(),
            of.last().map_or(0, |f| f.drawn),
        );
    }

    // Frames from the stop until the map holds what the settled view wants,
    // taken as within one per cent of what it ends on. The queue's own depth
    // says nothing here: the walk's offers are a pass's, so what is left in it
    // at the end of a frame is what the budget did not take, not what is still
    // wanted.
    if let Some(stopped) = rails.iter().position(|(_, _, leg)| *leg == "still")
    {
        let ends_on = frames.last().map_or(0, |frame| frame.drawn);
        let filled = ends_on * 99 / 100;
        let after = frames[stopped..]
            .iter()
            .position(|frame| frame.drawn >= filled)
            .map(|at| at + 1);
        match after {
            Some(frames_after) => println!(
                "\nfill-in: {ends_on} drawn, within a per cent of it \
                 {frames_after} frames after the stop",
            ),
            None => println!("\nfill-in: never reached {filled} drawn"),
        }
    }
    let despawned =
        flight.app.world().resource::<crate::systems::Evictions>().total;
    let queued_peak =
        frames.iter().map(|frame| frame.queued).max().unwrap_or(0);
    let drawn_peak = frames.iter().map(|frame| frame.drawn).max().unwrap_or(0);
    println!(
        "\nchurn: {despawned} systems despawned, {drawn_peak} drawn at the \
         peak, {queued_peak} queued at the peak",
    );
    let peak = frames.iter().map(|frame| frame.points).max().unwrap_or(0);
    let held = frames.iter().map(|frame| frame.held).max().unwrap_or(0);
    println!(
        "\n{} frames, {whole:.2?} in the walk, {} cells read, {} of them read \
         again",
        frames.len(),
        flight.asked.len(),
        flight.reasked,
    );
    println!(
        "peak {held} cells, {peak} points, {:.1} MB of payload",
        (peak * std::mem::size_of::<galos_index::Point>()) as f64 / 1e6,
    );
    // Every row of the flight, as one line each, is a wall of text to read a
    // shape off; the percentiles are the shape.
    let mut whole_frames: Vec<Duration> =
        frames.iter().map(Frame::whole).collect();
    whole_frames.sort_unstable();
    let at =
        |share: f64| whole_frames[(whole_frames.len() as f64 * share) as usize];
    println!("\nphase span            total     mean    times");
    for (name, took, count) in phases.table() {
        println!(
            "{name:<18} {took:>9.2?} {:>8.2?} {count:>8}",
            took / count.max(1) as u32,
        );
    }
    println!(
        "\nframe p50 {:.2?}, p90 {:.2?}, p99 {:.2?}, max {:.2?}",
        at(0.5),
        at(0.9),
        at(0.99),
        whole_frames.last().copied().unwrap_or_default(),
    );
}
