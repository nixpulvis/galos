//! What flying the map costs a frame, measured against a real directory.
//!
//! The walk's four systems — [`crate::map::galaxy::walk::fetch`],
//! [`crate::map::galaxy::walk::collect`], [`crate::map::galaxy::walk::reconcile`],
//! [`crate::map::galaxy::walk::evict_payloads`] — and the queue drain behind them
//! ([`crate::map::galaxy::spawn::drain_spawns`]) are what a zoom or a pan runs through, and
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
//! [`crate::map::route::perf`] does:
//!
//! ```sh
//! GALOS_PERF_DIR=.galos_index cargo test -p galos_map --lib flight -- --nocapture
//! ```
//!
//! The clocks are loose, this running on whatever machine is to hand; the
//! counts are not.

use crate::map::camera::OrbitCamera;
use crate::map::filter::{Cut, DimTo, Filters};
use crate::map::galaxy::spawn::PendingSpawns;
use crate::map::galaxy::walk::{BoundedTasks, ResidentCells};
use crate::map::galaxy::{PendingEvictions, Spyglass, System};
use crate::map::index::{Names, Populated, ResidentIndex, Transport};
use crate::map::paint::sizing::{ScalePopulation, View};
use crate::map::space::Galaxy;
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
    weigh: SystemId,
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
        let (index, populated) = pollster::block_on(async {
            use galos_index::Source as _;
            (
                source.index().await.expect("the index should read"),
                source.populated().await.unwrap_or_default(),
            )
        });
        // The political table, rolled up the tree as the client rolls it:
        // what a merged mark's colour and a faction filter's verdict are
        // read off ([`crate::map::galaxy::blobs`]). Empty, the weighing pass
        // would measure the miss path over a galaxy nobody lives in.
        let settled = crate::map::index::Settled(std::sync::Arc::new(
            galos_index::Inhabitance::of(&index, populated.iter()),
        ));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        app.insert_resource(ResidentIndex(index));
        app.insert_resource(Transport(std::sync::Arc::new(source)));
        app.insert_resource(settled);
        app.insert_resource(Populated(std::sync::Arc::new(
            populated.into_iter().map(|it| (it.address, it)).collect(),
        )));
        app.insert_resource(crate::map::galaxy::spawn::ColorBy::Allegiance);
        app.init_resource::<crate::map::paint::glow::Gains>();
        app.insert_resource(Names::reaching(Vec::new(), Vec::new()));
        app.insert_resource(View::Map);
        app.insert_resource(ScalePopulation(false));
        app.insert_resource(Spyglass {
            radius: 100.,
            clear: true,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(crate::map::galaxy::plan::Planned(
            galos_index::Needed {
                mode: galos_index::Mode::Shell,
                marks: Vec::new(),
                blobs: Vec::new(),
                splats: Vec::new(),
            },
        ));
        app.init_resource::<ResidentCells>();
        app.init_resource::<BoundedTasks>();
        app.init_resource::<crate::map::galaxy::walk::PointOrders>();
        app.init_resource::<crate::map::galaxy::walk::Republished>();
        app.init_resource::<crate::map::galaxy::plan::Drawn>();
        app.init_resource::<crate::map::galaxy::walk::Keeping>();
        app.init_resource::<crate::map::galaxy::walk::Sampled>();
        app.init_resource::<crate::map::galaxy::walk::Blobs>();
        app.init_resource::<crate::map::galaxy::blobs::Standing>();
        app.init_resource::<crate::map::galaxy::blobs::Named>();
        app.init_resource::<crate::map::galaxy::populated::PopulatedOrder>();
        app.init_resource::<crate::map::index::refresh::Held>();
        app.init_resource::<PendingSpawns>();
        app.init_resource::<PendingEvictions>();
        app.init_resource::<crate::map::galaxy::Evictions>();
        app.init_resource::<crate::map::bodies::spawn::HeldSystem>();
        app.init_resource::<crate::map::selection::Selection>();
        app.init_resource::<Filters>();
        app.init_resource::<DimTo>();
        app.init_resource::<Cut>();

        let galaxy = app
            .world_mut()
            .spawn((
                big_space::prelude::BigSpace::default(),
                crate::map::space::galaxy_grid(),
            ))
            .id();
        app.insert_resource(Galaxy(galaxy));
        app.world_mut().spawn((
            OrbitCamera::stood_back(100.),
            // The reach follows the camera through this, and `framed` reads
            // how wide the viewer sees off it.
            Projection::Perspective(PerspectiveProjection::default()),
            crate::map::galaxy::tests::seeing(),
        ));

        let world = app.world_mut();
        let reach =
            world.register_system(crate::map::galaxy::reach_with_camera);
        let plan = world.register_system(crate::map::galaxy::plan::plan);
        let fetch = world.register_system(crate::map::galaxy::walk::fetch);
        let collect = world.register_system(crate::map::galaxy::walk::collect);
        // Who lives where, gathered once as the map gathers it at
        // startup: the population scale draws out of this and not out of
        // a payload. See [`crate::map::galaxy::populated`].
        let gather =
            world.register_system(crate::map::galaxy::populated::gather);
        world.run_system(gather).expect("the peopled table gathers");
        let weigh =
            world.register_system(crate::map::galaxy::blobs::weigh_blobs);
        let reconcile =
            world.register_system(crate::map::galaxy::walk::reconcile);
        let evict =
            world.register_system(crate::map::galaxy::walk::evict_payloads);
        let drain =
            world.register_system(crate::map::galaxy::spawn::drain_spawns);
        let drop = world.register_system(crate::map::galaxy::drain_evictions);

        Flight {
            app,
            reach,
            plan,
            fetch,
            collect,
            weigh,
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
        self.turned(at, back, 0.)
    }

    /// The same, with the eye carried `turn` radians round the orbit —
    /// which is what dragging does, and what no amount of standing
    /// still measures.
    fn turned(&mut self, at: DVec3, back: f32, turn: f64) -> Frame {
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
                camera.stands_at(
                    at + DVec3::new(
                        f64::from(back) * turn.sin(),
                        0.,
                        f64::from(back) * turn.cos(),
                    ),
                );
            }
        }

        let mut frame = Frame::default();
        let held_before = self.app.world().resource::<ResidentCells>().0.len();

        for (name, id) in [
            ("reach", self.reach),
            ("plan", self.plan),
            ("fetch", self.fetch),
            ("collect", self.collect),
            ("weigh", self.weigh),
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
        frame.marks =
            world.resource::<crate::map::galaxy::plan::Planned>().0.marks.len();
        frame.held = world.resource::<ResidentCells>().0.len();
        frame.freed = held_before.saturating_sub(frame.held);
        frame.queued = world.resource::<PendingSpawns>().queued();
        frame.dropped = world.resource::<PendingEvictions>().0.len();
        frame.drawn = world.query::<&System>().iter(world).count();
        frame.despawned =
            world.resource::<crate::map::galaxy::Evictions>().total;
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

/// Reading the sky as populations draws every system that has a patch of
/// screen to itself
///
/// **Every cell of the plan answers with its whole subtree's populated
/// systems, and the marked cells nest**: the root, every cell down to
/// the frontier, each drawing its own marks. So without taking each
/// system once the same busiest handful is offered over and over and
/// nothing further down is ever reached. Reported as `ALPHA CENTAURI`
/// vanishing when the scale was turned on — a population of a hundred
/// thousand four light years from the camera — while the pass spent
/// 2,496 marks where the ordinary sky spent 2,562.
///
/// What may be missing is what a mark ahead of it already covers: two
/// marks that would overlap are drawn as one, which is the rule the
/// merge frontier applies to the payload draw and
/// [`galos_index::screen::Crowded`] applies to this one. So the claim
/// here is the rule itself — nothing is dropped from a patch of screen
/// that is otherwise empty.
#[test]
fn the_populated_sky_draws_what_stands_alone() {
    let Some(dir) = measured() else { return };
    let mut flight = Flight::over(&dir);
    flight.app.insert_resource(ScalePopulation(true));
    let back = 200f32;
    for _ in 0..SETTLE {
        flight.frame(DVec3::ZERO, back);
    }

    let world = flight.app.world_mut();
    let mut cameras = world.query::<(&OrbitCamera, &Camera)>();
    let (orbit, camera) = cameras.single(world).expect("a camera");
    let view = crate::map::galaxy::plan::view(orbit, camera)
        .expect("the camera can see");
    let about = orbit.center().to_array();

    // What the frame drew, by the patch of screen each mark holds.
    let mut systems = world.query::<&System>();
    let drawn: Vec<&System> = systems.iter(world).collect();
    let mut held = galos_index::screen::Crowded::about(&view, about);
    for system in &drawn {
        held.claim(system.position);
    }
    let addresses: HashSet<i64> =
        drawn.iter().map(|system| system.address).collect();

    // Everyone within a few light years of the camera: close in there is
    // no thinning left to excuse a miss except an overlap, and an
    // overlap is what `Crowded` will own up to.
    let near: Vec<(u64, i64, [f64; 3])> = world
        .resource::<Populated>()
        .0
        .values()
        .map(|system| {
            (
                system.population,
                system.address,
                [
                    f64::from(system.position[0]),
                    f64::from(system.position[1]),
                    f64::from(system.position[2]),
                ],
            )
        })
        .filter(|(_, _, at)| DVec3::from(*at).length() <= 25.)
        .collect();
    assert!(near.len() > 5, "nobody lives within 25 ly of Sol");

    let missing: Vec<(u64, i64)> = near
        .iter()
        .filter(|(_, address, _)| !addresses.contains(address))
        // Claiming answers true only where nothing holds the tile, so
        // what is left is a system dropped from empty screen.
        .filter(|(_, _, at)| held.claim(*at))
        .map(|&(population, address, _)| (population, address))
        .collect();
    assert!(
        missing.is_empty(),
        "{} of the {} populated systems within 25 ly went undrawn from \
         {back:.0} ly out with nothing else on their patch of screen: \
         {:?}",
        missing.len(),
        near.len(),
        &missing[..missing.len().min(5)],
    );
}

/// Turning the view does not reshuffle the populated sky
///
/// **Reported as flicker while dragging.** The picture was never still
/// under rotation: marks winked out and others took their place for as
/// long as the hand was moving, and settling the camera settled the
/// picture on something slightly different each time.
///
/// The cause was the lattice that thins the marks being reckoned *from
/// the eye*. An orbit is not a turn — the eye swings a full radius
/// through the galaxy — so every distance and every line of sight from
/// it changed continuously as the hand dragged, and with them which
/// marks were merged away. Nothing about what the reader is looking at
/// changed. It is reckoned about the point the view turns on instead,
/// and sized by how far back the eye stands, neither of which an orbit
/// moves. See [`galos_index::screen::Crowded::about`].
///
/// A quarter turn at a time, all the way round, each held long enough
/// to settle: the drawn sky must come back the same set of systems it
/// started as, not merely the same number of them.
#[test]
fn turning_the_view_does_not_reshuffle_the_populated_sky() {
    let Some(dir) = measured() else { return };
    let mut flight = Flight::over(&dir);
    flight.app.insert_resource(ScalePopulation(true));
    {
        let mut glass = flight
            .app
            .world_mut()
            .resource_mut::<crate::map::galaxy::Spyglass>();
        glass.radius = 200.;
        glass.clear = true;
        glass.follow_camera = false;
    }
    let drawn = |flight: &mut Flight| -> HashSet<i64> {
        let world = flight.app.world_mut();
        let mut systems = world.query::<&System>();
        systems.iter(world).map(|system| system.address).collect()
    };

    for _ in 0..SETTLE {
        flight.frame(DVec3::ZERO, 540.);
    }
    let first = drawn(&mut flight);
    assert!(!first.is_empty(), "nothing was drawn, so nothing is tested");

    for step in 1..=4 {
        let turn = f64::from(step) * std::f64::consts::FRAC_PI_2;
        for _ in 0..SETTLE {
            flight.turned(DVec3::ZERO, 540., turn);
        }
        let now = drawn(&mut flight);
        let gone = first.difference(&now).count();
        let new = now.difference(&first).count();
        assert!(
            gone == 0 && new == 0,
            "a quarter turn ({step} of 4) changed the drawn sky: \
             {gone} of {} gone and {new} arrived",
            first.len(),
        );
    }
}

/// A still view of the populated sky costs nothing to hold
///
/// **What a mode draws is settled by the plan, not by the frame.** The
/// population scale weighs every populated system in reach against the
/// lattice that thins them, and it was doing that afresh sixty times a
/// second: measured over `.index/full` with the reach at five hundred
/// light years, **69 ms of every settled frame** — fourteen frames a
/// second on a view nobody was moving. Reported as the mode being slow
/// to load.
///
/// Two costs, and the test holds both at each of the two reaches the
/// picture is judged at. A settled frame owes only to mark what is
/// drawn as wanted, which is 0.59 ms at five hundred light years; the
/// frame the plan moves owes the choice itself, 4.1 ms, and not the
/// 76 ms it was when each of the plan's 831 cells rescanned its own
/// subtree — nested cells walking the same systems over and over,
/// 1,559,152 entries read to choose 25,744 marks. See
/// [`crate::map::galaxy::walk::choose_populated`].
#[test]
fn the_populated_sky_settles_cheap() {
    let Some(dir) = measured() else { return };
    for radius in [100f32, 500.] {
        let mut flight = Flight::over(&dir);
        flight.app.insert_resource(ScalePopulation(true));
        {
            let mut glass = flight
                .app
                .world_mut()
                .resource_mut::<crate::map::galaxy::Spyglass>();
            glass.radius = radius;
            glass.clear = true;
            glass.follow_camera = false;
        }
        let mut frames = Vec::new();
        for _ in 0..SETTLE {
            frames.push(flight.frame(DVec3::ZERO, radius * 2.7).whole());
        }
        let worst = frames.iter().max().copied().unwrap_or_default();
        let settled = frames.last().copied().unwrap_or_default();
        let drawn = {
            let world = flight.app.world_mut();
            let mut systems = world.query::<&System>();
            systems.iter(world).count()
        };
        println!(
            "  {radius} ly: {drawn} populated drawn, worst frame \
             {worst:.2?}, settled {settled:.2?}",
        );
        // Roomy against the 0.59 ms and 4.1 ms measured, the point
        // being the order of magnitude: a settled frame must not be
        // paying for the choice, and the frame that does must stay
        // inside a stutter.
        assert!(
            settled < Duration::from_millis(8),
            "a still populated view costs {settled:.2?} a frame",
        );
        assert!(
            worst < Duration::from_millis(40),
            "the frame the plan moves costs {worst:.2?}",
        );
    }
}

/// The same view draws the same sky, however the eye got there
///
/// **A view is a question about where the camera stands, and the answer
/// must not depend on the route taken to it.** Reported as: zoom in on a
/// filament, zoom back out to where you were, and the sky is fuller than
/// it was. Measured over `.index/full` at 2,300, 13, 5,300 with the
/// population scale on — two thousand light years back drew **57 systems
/// arriving and 92 after a zoom in and back out**, and twelve thousand
/// drew 87 and then 78, so it moved both ways.
///
/// The cause was the source: that mode draws the systems anybody lives
/// in, one payload point in forty-four is one of those, and a payload is
/// read as a magnitude-ordered prefix sized for the mark count — so the
/// busiest of the prefix was not the busiest of the cell, and which
/// prefix was resident depended on where the camera had been. It reads
/// the resident table instead; see [`crate::map::galaxy::populated::PopulatedOrder`].
///
/// Both modes, because the answer has to hold in each, and settled rather
/// than merely visited: the map fills in behind a moving eye, so what a
/// view *means* is what it comes to once the hand is off the mouse.
#[test]
fn a_view_is_what_it_is_however_it_was_reached() {
    let Some(dir) = measured() else { return };
    // Off the plane and out in the disc, where it was reported: a
    // filament of colonies rather than the crowd around Sol.
    let at = DVec3::new(2_300., 13., 5_300.);

    for (scale, back) in
        [(false, 2_000f32), (true, 2_000.), (true, 5_000.), (true, 12_000.)]
    {
        let mut flight = Flight::over(&dir);
        flight.app.insert_resource(ScalePopulation(scale));

        let settle = |flight: &mut Flight, back: f32| {
            let mut drawn = 0;
            for _ in 0..SETTLE {
                drawn = flight.frame(at, back).drawn;
            }
            drawn
        };
        let first = settle(&mut flight, back);
        // In, and back out, the way a hand on a wheel goes.
        let mut closer = back;
        while closer > back / 8. {
            closer /= 1.5;
            for _ in 0..DWELL {
                flight.frame(at, closer);
            }
        }
        while closer < back {
            closer *= 1.5;
            for _ in 0..DWELL {
                flight.frame(at, closer);
            }
        }
        let second = settle(&mut flight, back);

        println!(
            "  {back:>6.0} ly back, by population {scale:>5}: \
             {first:>6} drawn arriving, {second:>6} after a zoom in and out",
        );
        assert_eq!(
            first, second,
            "{back:.0} ly back by population {scale}: the same view drew \
             {first} systems arriving and {second} after a zoom in and out",
        );
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
             plan {:>7.2?} fetch {:>7.2?} collect {:>7.2?} weigh {:>7.2?} \
             reconcile {:>7.2?} evict {:>7.2?} drain {:>7.2?} drop {:>7.2?}  \
             {leg}",
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
            frame.of("weigh"),
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
        "weigh",
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
        let planned = world.resource::<crate::map::galaxy::plan::Planned>();
        let marked: rustc_hash::FxHashSet<galos_index::CellId> =
            planned.0.marks.iter().map(|mark| mark.id).collect();
        let resident = world.resource::<ResidentCells>();
        let mut held = 0usize;
        let mut wanted = 0usize;
        let mut deepest = (0usize, 0usize);
        for (id, cell) in resident.0.iter() {
            let target =
                if marked.contains(&id) { cell.points.len() } else { 0 };
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
        flight.app.world().resource::<crate::map::galaxy::Evictions>().total;
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
