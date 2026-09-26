//! A galaxy on disk, for the tests that route over one.
//!
//! The router reads the cell payloads where they lie
//! ([`galos_index::Sky`]), so a test that asks for a route needs a built
//! directory and not a list of places. Which is the right shape for a test
//! to have: what it exercises is then the same mapping, the same descent
//! and the same records the map reads, rather than a second implementation
//! of them that happens to agree.

use bevy_egui::egui;
use galos_index::{BuildParams, Sky, Snapshot, System};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A directory removed with the test that made it.
pub struct Scratch(pub PathBuf);

impl Scratch {
    /// An empty directory named for `what` and this process.
    pub fn new(what: &str) -> Scratch {
        static SEQ: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir()
            .join(format!("galos-map-{what}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at).expect("a scratch directory");
        Scratch(at)
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a directory holding these systems, and map it.
///
/// The magnitudes ascend with the address so the tree's ordering is
/// unambiguous, and nothing else about a system matters to a route.
pub fn sky(dir: &Path, places: &[(i64, [f64; 3])]) -> Arc<Sky> {
    built(dir, places, &BuildParams::default())
}

/// The same, divided until no two systems share a cell
///
/// The build divides on *count*: a cell splits when more systems fall in it
/// than the cap, and a cell keeps the brightest slice of what it owned. So
/// a handful of systems is one root payload under the published caps,
/// however far apart they lie — which leaves nothing to say about a query
/// that has to cross cells. Cut to one apiece, the same places build a tree
/// several levels deep and every system has a cell of its own.
pub fn sky_apart(dir: &Path, places: &[(i64, [f64; 3])]) -> Arc<Sky> {
    built(dir, places, &BuildParams { internal_slice: 1, leaf_cap: 1 })
}

/// Write the tree `params` makes of `places`, and map what was written.
fn built(
    dir: &Path,
    places: &[(i64, [f64; 3])],
    params: &BuildParams,
) -> Arc<Sky> {
    let systems: Vec<System> = places
        .iter()
        .map(|&(address, position)| System {
            id64: address as u64,
            position,
            absolute_magnitude: address as f64 * 0.001 - 3.0,
            temperature: 5000.0,
            age_bucket: 0,
            updated_at: 0,
            kind: galos_index::StarKind::G,
        })
        .collect();
    Snapshot::build(&systems, params).write(dir).expect("a built galaxy");
    Arc::new(Sky::open(dir).expect("the galaxy maps"))
}

/// The same over places given as the names table carries them, `f32`.
/// The address of the class `A` boxel `place` falls in
///
/// **A fixture places a system by giving it the right address.** The names
/// table holds no position since a name became a function of one, and what
/// it answers with is the middle of the boxel the address names — so a test
/// whose sky puts a system at a place and whose table gives it an unrelated
/// address is a test where the two disagree about where it is, and the
/// router resolves its ends against the wrong neighbourhood.
///
/// The real mapping, so any place has an address. The middle of the boxel
/// is within five light years of the place asked for, and the grid's period
/// is ten, so a fixture asking for multiples of ten gets its distances
/// exactly.
pub fn boxel_at(place: [f64; 3]) -> i64 {
    let axis = |at: f64, which: usize| {
        let from = at - elite_journal::boxel::ORIGIN[which];
        let sector = (from / elite_journal::boxel::SECTOR_LY).floor();
        let within = from - sector * elite_journal::boxel::SECTOR_LY;
        (sector as u8, (within / 10.0).floor() as u32)
    };
    let (sx, x) = axis(place[0], 0);
    let (sy, y) = axis(place[1], 1);
    let (sz, z) = axis(place[2], 2);
    elite_journal::Boxel {
        mass: 0,
        sector: [sx, sy, sz],
        ordinal: x + 128 * y + 128 * 128 * z,
        index: 0,
    }
    .address()
    .expect("a boxel inside the grid")
}

pub fn sky_of(dir: &Path, entries: &[galos_index::NameEntry]) -> Arc<Sky> {
    let places: Vec<(i64, [f64; 3])> = entries
        .iter()
        .map(|entry| {
            (
                entry.address,
                [
                    entry.position[0] as f64,
                    entry.position[1] as f64,
                    entry.position[2] as f64,
                ],
            )
        })
        .collect();
    sky(dir, &places)
}

// What a test draws egui through.

/// A context lettered as the map letters its own
///
/// What is drawn here is measured, and how wide a word comes out is the
/// font's answer. A test weighing a line against the room there is for it
/// in a face the map does not use is a test about some other map.
///
/// **And it complains about an id clash whether or not this is a debug
/// build.** Egui's `warn_on_id_clash` defaults to
/// `cfg!(debug_assertions)`, so in release it reports nothing — and the
/// eighteen tests that draw a piece of the bar twice and assert egui
/// said nothing were all *vacuously* green under `--release`, along
/// with the two that check the instruments themselves can hear a real
/// clash, which failed outright and are how it was noticed. It is a
/// runtime option and not a compile-time one, so the answer is to ask
/// for it: an id clash is a fault of the code and not of the profile it
/// was built in.
pub(crate) fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(crate::ui::styled);
    ctx.options_mut(|options| options.warn_on_id_clash = true);
    ctx
}

/// Draw `contents` into a bare context and tessellate what it made
///
/// Laying a widget out is not the half of it that goes wrong. Egui defers
/// a color the caller did not give as a placeholder for the painter to
/// answer, and one answered by another placeholder is caught nowhere until
/// epaint meets it and panics. So the shapes are turned into triangles
/// here, which is the step that looks.
///
/// Shared by the chrome and the panels, which paint their rows the same
/// way and can go wrong in it the same way.
pub(crate) fn painted(mut contents: impl FnMut(&mut egui::Ui)) {
    let ctx = context();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));
    ctx.tessellate(output.shapes, output.pixels_per_point);
}

/// Every piece of text `contents` painted, in the order it was painted
///
/// What a widget draws is the whole of what the user is told, and a row
/// that lays its text out itself has no label to be asked what it says.
/// So this reads it back off the shapes.
pub(crate) fn words(mut contents: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let ctx = context();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

    fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().into()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text_of(shape, into);
                }
            }
            _ => {}
        }
    }

    let mut said = Vec::new();
    for shape in &output.shapes {
        text_of(&shape.shape, &mut said);
    }
    said
}

/// What egui said about `first` being redrawn as `second`
///
/// Egui checks between one pass and the next whether a rectangle kept its
/// place while everything in it changed identity, which is how a widget
/// taking another's state shows up. It says so through `log` and nowhere
/// else, so this listens for that rather than reading the painted output
/// the way [`complaints`] does.
///
/// Two passes over one context, since a warning about what changed
/// between them cannot be had from either alone.
///
/// What is heard is kept per thread, and a test hears its own thread and
/// no other. A logger is installed once for the whole process and the
/// tests it hears run at the same time as the rest, so a warning from
/// somewhere else would otherwise be read as this pass having complained.
/// Egui logs from whichever thread called it, which is this one.
///
/// **Debug builds only, because the check itself is.** Egui's
/// `warn_if_rect_changes_id` is `#[cfg(debug_assertions)]` — compiled
/// out of a release build rather than switched off by an option, as
/// `warn_on_id_clash` is ([`context`]) — so under `--release` there is
/// nothing to listen for and every caller would assert that nothing
/// was said about a check that never ran. Gated rather than left to
/// pass vacuously: a test that cannot observe its subject should not
/// be counted as having observed it.
#[cfg(debug_assertions)]
pub(crate) fn between_passes(
    first: impl FnMut(&mut egui::Ui),
    second: impl FnMut(&mut egui::Ui),
) -> Vec<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::thread::{self, ThreadId};

    /// What the logger has heard, by the thread that said it. Tests share
    /// a process, and a logger may be installed once in one.
    static HEARD: Mutex<Option<HashMap<ThreadId, Vec<String>>>> =
        Mutex::new(None);
    static LOGGER: OnceLock<()> = OnceLock::new();

    /// What `HEARD` has for `thread`, made if it has none
    fn heard_by<R>(
        thread: ThreadId,
        act: impl FnOnce(&mut Vec<String>) -> R,
    ) -> R {
        // Poisoning says some other test panicked mid-pass, which is that
        // test's news to break rather than this one's.
        let mut heard =
            HEARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        act(heard.get_or_insert_with(HashMap::new).entry(thread).or_default())
    }

    struct Listener;
    impl log::Log for Listener {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            if record.level() <= log::Level::Warn {
                let said = record.args().to_string();
                heard_by(thread::current().id(), |heard| heard.push(said));
            }
        }
        fn flush(&self) {}
    }

    LOGGER.get_or_init(|| {
        let _ = log::set_boxed_logger(Box::new(Listener));
        log::set_max_level(log::LevelFilter::Warn);
    });

    let mine = thread::current().id();
    heard_by(mine, Vec::clear);

    let ctx = context();
    for pass in
        [Box::new(first) as Box<dyn FnMut(&mut egui::Ui)>, Box::new(second)]
    {
        let mut pass = pass;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| pass(ui));
    }

    heard_by(mine, std::mem::take)
}

/// What egui complained about in the margins while `contents` was drawn
///
/// Egui reports two widgets sharing an id by painting the offending
/// rectangle in its error color and writing what happened beside it. It
/// says so nowhere else, so this picks it out of what was painted.
pub(crate) fn complaints(contents: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let mut said = words(contents);
    said.retain(|line| line.contains("Double use") || line.contains("use of"));
    said
}
