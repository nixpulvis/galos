//! Pointing at a merged mark.
//!
//! **A merged mark is a system, and which one follows what the map is
//! drawing.** A blob stands for everything under one cell of the tree
//! ([`galos_index::BlobRef`]) because all of it falls inside one mark, so
//! there is no second mark to tell it apart from — but there is always one
//! system under it the eye is really being pointed at, and out at galaxy
//! scale it is the only thing there is to click. Brightest by default;
//! busiest while the map is drawing marks by population, since then size is
//! what a mark says and the biggest is what is being aimed at. The same
//! ordering the marks themselves are drawn in, read off the same payload.
//!
//! **Not entities, which is why none of this goes through the picker.**
//! Bevy's picking answers over components and there are tens of thousands
//! of blobs a frame, rewritten whenever the eye moves; spawning them to be
//! pointed at is a transform, a visibility and a picking test apiece for a
//! set that is gone next frame. So they are a list
//! ([`crate::map::galaxy::walk::Blobs`]) and this is the one place that reads the
//! pointer against it.
//!
//! **What resolving one costs is one read, on the pointer resting.** A
//! blob's cell owns the brightest of its own subtree — the tree's slices
//! are magnitude-ordered from the root down — so the system a blob stands
//! for is the head of the cell's own payload and nothing deeper has to be
//! read for it. Cached by cell, since the pointer crossing a galaxy of
//! merged marks would otherwise ask for one a frame.

use crate::map::camera::OrbitCamera;
use crate::map::galaxy::walk::{Blob, Blobs, build_from_point};
use crate::map::galaxy::{MapSet, System};
use crate::map::index::{Names, Populated, Transport};
use crate::map::pointing::{DRAG_THRESHOLD, DragDistance, PointedAt};
use crate::map::schedule::PaintSet;
use crate::map::screen::screen_position;
use crate::map::selection::{Picked, Selection};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use galos_index::{CellId, Point};
use rustc_hash::FxHashMap;

pub fn plugin(app: &mut App) {
    app.init_resource::<PointedBlob>();
    app.init_resource::<Prominent>();
    app.init_resource::<Standing>();
    app.init_resource::<Named>();
    // Before the draw reads it, and in the phase that builds the drawn set:
    // what a merged mark stands for is settled per plan, not per frame.
    app.add_systems(
        Update,
        weigh_blobs
            .in_set(MapSet::Populate)
            .before(crate::map::galaxy::walk::reconcile),
    );
    app.add_systems(
        Update,
        (point_at_blobs, name_blobs, click_blobs)
            .chain()
            .in_set(MapSet::Present)
            .after(crate::map::pointing::point_at),
    );
    // Ringed in the same pass and the same layer the pointer's own ring is
    // drawn in, and after it, so a merged mark and a system are marked the
    // same way and neither is drawn over the other.
    // Gated on the index being read, as [`crate::map::schedule`] gates the
    // whole `Update` pipeline: this reads the tables that read delivers,
    // and the egui pass runs from the first frame, before there are any.
    app.add_systems(
        EguiPrimaryContextPass,
        ring_blob
            .after(crate::map::pointing::ring)
            .before(crate::map::labels::draw_names)
            .in_set(PaintSet::Map)
            .run_if(in_state(crate::map::index::load::Opening::Drawn)),
    );
}

/// Ring the merged mark under the pointer and say what it stands for
///
/// **A ring, because that is what being pointed at looks like here.** The
/// mark itself is a speck a fraction of a pixel across — one merged mark
/// is one mark however much it stands for, since drawing it wider would
/// say the density with size — so what says the pointer has caught it is
/// the same circle a system gets.
///
/// What it says is what a blob is: how many systems are under it, and the
/// one it stands for once that has been read. The count comes off the
/// aggregate and is there on the first frame; the name arrives a read
/// later and is added to the same line rather than replacing it, so the
/// readout does not jump as it lands.
fn ring_blob(
    mut contexts: EguiContexts,
    cameras: Query<(&OrbitCamera, &Camera)>,
    pointed: Res<PointedBlob>,
    prominent: Res<Prominent>,
    names: Res<Names>,
    populated: Res<Populated>,
    view: Res<crate::map::paint::sizing::View>,
    scale_population: Res<crate::map::paint::sizing::ScalePopulation>,
) -> Result {
    let Some(blob) = pointed.0 else { return Ok(()) };
    let Ok((orbit, camera)) = cameras.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else {
        return Ok(());
    };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let Some(at) =
        screen_position(orbit, cot_half_fov, viewport, DVec3::from(blob.at))
    else {
        return Ok(());
    };

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(crate::map::screen::annotations_layer());
    let color =
        crate::style::color32(crate::map::labels::marked_tint(true, false));
    painter.circle_stroke(
        egui::pos2(at.x, at.y),
        CATCH_PX,
        egui::Stroke::new(crate::map::pointing::RING_STROKE, color),
    );

    // Laid out as the map lays a name, because that is what it is: the
    // same face, the same dark ground, and the same gap up and to the
    // right of the mark it belongs to. Written by hand and it read as a
    // caption stuck beside a ring — a proportional face over the stars,
    // with the field of them filling its counters. See
    // [`crate::map::labels::draw_names`], whose figures these are.
    //
    // No leader, as a name marked out has none: the ring already says
    // which mark this is about.
    let by_population =
        crate::map::paint::sizing::by_population(&view, &scale_population);
    let said = match prominent
        .of(blob.id, by_population, &populated)
        .map(|point| build_from_point(point, &populated, &names))
    {
        Some(system) => format!("{} · {} systems", system.name, blob.count),
        None => format!("{} systems", blob.count),
    };
    let galley =
        painter.layout_no_wrap(said, crate::map::labels::naming(), color);
    let origin = egui::pos2(
        at.x + CATCH_PX
            + crate::map::labels::NAME_HEIGHT * crate::map::labels::GAP,
        at.y - crate::map::labels::NAME_HEIGHT * crate::map::labels::RISE
            - galley.size().y / 2.,
    );
    let pad = crate::map::labels::NAME_HEIGHT * crate::map::labels::GROUND_PAD;
    painter.rect_filled(
        egui::Rect::from_min_size(origin, galley.size()).expand(pad),
        0.,
        crate::style::color32(crate::map::labels::GROUND),
    );
    painter.galley(origin, galley, color);
    Ok(())
}

/// How near the pointer has to come to a merged mark to be on it, in pixels
///
/// A blob is drawn at the field's smallest radius, which is under a pixel,
/// and a target under a pixel is a target nobody can hit. This is the same
/// figure the pointer catches an ordinary mark over
/// ([`crate::map::pointing::INDICATOR_MIN_RADIUS`]) so that a merged mark and a
/// drawn system answer the pointer over the same area and the sky does not
/// change how it behaves as cells merge.
const CATCH_PX: f32 = crate::map::pointing::INDICATOR_MIN_RADIUS;

/// The merged mark under the pointer, if one is and nothing else is
///
/// Behind everything drawn: a blob is what the map has instead of the
/// systems it stands for, so a real system under the pointer is always the
/// better answer and this holds nothing while one is.
#[derive(Resource, Default)]
pub struct PointedBlob(pub Option<Blob>);

/// The system each merged mark stands for, once anything has asked
///
/// Keyed by cell and kept across frames: the pointer crossing a galaxy of
/// merged marks passes over thousands of them, and reading a payload for
/// each would be a file a frame for an answer that never changes while the
/// cell is on the map.
///
/// [`None`] against a cell is an answer too — a cell whose payload holds
/// nothing readable — so that it is asked once rather than every frame the
/// pointer rests on it.
#[derive(Resource, Default)]
pub struct Prominent {
    known: FxHashMap<CellId, Vec<Point>>,
    reading: FxHashMap<CellId, Task<Option<Vec<Point>>>>,
}

impl Prominent {
    /// The point a cell's mark stands for, where it has been read
    ///
    /// The head of the prefix while the map is drawing the sky as light:
    /// the payload is in magnitude order, so the first is the brightest
    /// thing under the cell.
    ///
    /// The busiest of the prefix where marks are drawn by population,
    /// because there a mark's *size* is the population it carries and the
    /// biggest is what the eye is aiming at — the same rule
    /// `bounded::busiest_first` draws them in. Of the cell's own brightest
    /// and not of its whole subtree: the systems further down live in the
    /// payloads of cells the walk never asked for, and reading a subtree
    /// to name one mark is a galaxy read to answer a hover.
    ///
    /// Kept as the prefix rather than the choice, so changing what the map
    /// draws changes the answer without reading anything again.
    pub fn of(
        &self,
        id: CellId,
        by_population: bool,
        populated: &Populated,
    ) -> Option<&Point> {
        let read = self.known.get(&id)?;
        if !by_population {
            return read.first();
        }
        read.iter()
            .max_by_key(|point| {
                populated
                    .get(point.id64 as i64)
                    .map_or(0, |system| system.population)
            })
            .or_else(|| read.first())
    }
}

/// Which merged mark the pointer is on
///
/// After [`crate::map::pointing::point_at`], and nothing while that found
/// something: a blob is drawn where the systems it stands for are, so a
/// system drawn over one is one of the very systems the blob is standing
/// in for and pointing past it at the crowd behind would be pointing at
/// the same thing twice.
///
/// The nearest to the pointer rather than the nearest to the eye. Blobs
/// carry no depth worth sorting on — each is a whole subtree, and two
/// overlapping on screen are two regions of sky in the same direction —
/// so what the pointer is on is what it is closest to.
fn point_at_blobs(
    cameras: Query<(&OrbitCamera, &Camera)>,
    blobs: Res<Blobs>,
    pointed_at: Query<(), With<PointedAt>>,
    dragged: Query<&DragDistance>,
    pointer: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut pointed: ResMut<PointedBlob>,
) {
    let found = 'found: {
        if !pointed_at.is_empty() {
            break 'found None;
        }
        if dragged.iter().any(|far| far.0 > DRAG_THRESHOLD) {
            break 'found None;
        }
        let Ok((orbit, camera)) = cameras.single() else { break 'found None };
        let Some(viewport) = camera.logical_viewport_size() else {
            break 'found None;
        };
        let Ok(window) = pointer.single() else { break 'found None };
        let Some(at) = window.cursor_position() else { break 'found None };
        let cot_half_fov = camera.clip_from_view().y_axis.y;

        let mut nearest: Option<(f32, Blob)> = None;
        for blob in &blobs.0 {
            let Some(on_screen) = screen_position(
                orbit,
                cot_half_fov,
                viewport,
                DVec3::from(blob.at),
            ) else {
                continue;
            };
            let away = on_screen.distance(at);
            if away > CATCH_PX {
                continue;
            }
            if nearest.is_none_or(|(near, _)| away < near) {
                nearest = Some((away, *blob));
            }
        }
        nearest.map(|(_, blob)| blob)
    };

    // Written only where it changed, as the pointer's own mark is: a
    // resource written every frame wakes everything reading it every frame.
    if pointed.0.map(|blob| blob.id) != found.map(|blob| blob.id) {
        pointed.0 = found;
    }
}

/// Read the system the pointed-at merged mark stands for
///
/// One cell's payload prefix, off the same transport the walk's own reads
/// go through, and only for the mark under the pointer. What comes back is
/// kept in [`Prominent`] against the cell for as long as the map runs.
///
/// A prefix and not the whole cell: the payload is in magnitude order, so
/// the brightest is its head and [`PREFIX`] is enough to choose the busiest
/// among the cell's own brightest too. Reading a whole slice to rank it
/// would be hundreds of systems read to name one.
///
/// A cell that reads back nothing is remembered as nothing — an empty
/// prefix is an answer — so it is asked once rather than every frame the
/// pointer rests on it.
fn name_blobs(
    pointed: Res<PointedBlob>,
    transport: Res<Transport>,
    mut prominent: ResMut<Prominent>,
) {
    // What comes back, taken before anything new is asked so a resolved
    // cell is answered on the frame it lands.
    let landed: Vec<CellId> = prominent
        .reading
        .iter_mut()
        .filter_map(|(id, task)| {
            block_on(poll_once(task)).map(|read| (*id, read))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(id, read)| {
            prominent.known.insert(id, read.unwrap_or_default());
            id
        })
        .collect();
    for id in landed {
        prominent.reading.remove(&id);
    }

    let Some(blob) = pointed.0 else { return };
    if prominent.known.contains_key(&blob.id)
        || prominent.reading.contains_key(&blob.id)
    {
        return;
    }
    let source = transport.0.clone();
    let id = blob.id;
    let task = AsyncComputeTaskPool::get()
        .spawn(async move { source.payload_prefix(id, PREFIX).await.ok() });
    prominent.reading.insert(id, task);
}

/// How much of a merged cell's payload is read to name it
///
/// The head of it is the answer outright while the map is drawing the sky
/// as light — the tree's slices are magnitude-ordered from the root down,
/// so a cell's first point is the brightest thing under it — and this is
/// the same `READ_LEAST` the walk's own reads are floored at, so naming a
/// blob asks the transport for no more than any other cell does.
const PREFIX: usize = 16;

/// Pick out the system a merged mark stands for
///
/// The same gesture that picks a drawn system out, answered the same way:
/// the system is built off the point that was read and handed to the
/// selection, which spawns it, rings it and flies to it exactly as it
/// would one that had been on the map all along.
fn click_blobs(
    gesture: crate::input::Gesture,
    pointed: Res<PointedBlob>,
    prominent: Res<Prominent>,
    dragged: Query<&DragDistance>,
    keys: Res<ButtonInput<KeyCode>>,
    populated: Res<Populated>,
    names: Res<Names>,
    view: Res<crate::map::paint::sizing::View>,
    scale_population: Res<crate::map::paint::sizing::ScalePopulation>,
    mut selection: ResMut<Selection>,
) {
    if !gesture.on_map() {
        return;
    }
    if dragged.iter().any(|far| far.0 > DRAG_THRESHOLD) {
        return;
    }
    let Some(blob) = pointed.0 else { return };
    let by_population =
        crate::map::paint::sizing::by_population(&view, &scale_population);
    let Some(point) = prominent.of(blob.id, by_population, &populated) else {
        return;
    };
    // Held down, a modifier gathers rather than replaces, exactly as it
    // does over a drawn system; see `crate::map::galaxy::spawn::select_on_click`.
    let gathering = crate::input::gathering(&keys);
    let system: System = build_from_point(point, &populated, &names);
    selection.pick(Picked::System(system), gathering);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::galaxy::tests::seeing;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use galos_index::StarKind;

    /// A merged mark over `count` systems, of which `colonies` are
    /// imperial: what the aggregate's political histogram would say.
    fn inhabited(colonies: u32) -> galos_index::read::inhabited::Inhabited {
        use elite_journal::prelude::Allegiance;
        let mut held = galos_index::read::inhabited::Inhabited::ZERO;
        for n in 0..colonies {
            held =
                held.merge(galos_index::read::inhabited::Inhabited::of_system(
                    [f64::from(n), 0., 0.],
                    Some(Allegiance::Empire),
                    None,
                    None,
                ));
        }
        held
    }

    /// A merged mark is the average of the marks it stands for
    ///
    /// **The merge boundary is the test.** A cell that splits as the camera
    /// comes in must not change colour as it splits, so the one mark it was
    /// drawn as has to be what its systems' own marks come to — summed as
    /// light, divided by how many there are. A cell painted by its loudest
    /// colony would flip colour at the boundary; a cell painted grey
    /// whatever it holds — which is what this did — is the field tinted by
    /// a region's politics with grey marks standing over it.
    #[test]
    fn a_merged_mark_averages_the_marks_it_stands_for() {
        use crate::map::galaxy::spawn::{ColorBy, Hue};
        let gains = crate::map::paint::glow::Gains::default();
        let grey = Hue::Grey.light()
            * crate::map::paint::glow::mark_light(Hue::Grey, false, &gains);

        // A cell with nothing living in it is painted exactly what a merged
        // mark was always painted: the palette's grey at an uninhabited
        // system's level.
        let empty = average_mark(None, 10_000, ColorBy::Allegiance, &gains);
        assert!(
            (empty - grey).length() < 1e-9,
            "a cell with no colonies moved: {empty:?} against {grey:?}",
        );

        // A cell that is nothing but imperial colonies is painted the
        // imperial mark's own colour.
        let all = inhabited(8);
        let imperial = average_mark(Some(&all), 8, ColorBy::Allegiance, &gains);
        assert!(
            imperial.length() > empty.length(),
            "a cell of colonies came out no brighter than empty sky",
        );
        assert!(
            imperial.x > imperial.z * 1.2 || imperial.z > imperial.x * 1.2,
            "a cell of colonies came out neutral: {imperial:?}",
        );

        // And a dozen colonies in ten thousand systems is nearly grey,
        // because that is what the ten thousand marks look like.
        let trace = average_mark(
            Some(&inhabited(12)),
            10_000,
            ColorBy::Allegiance,
            &gains,
        );
        assert!(
            (trace - grey).length() < (imperial - grey).length() * 0.05,
            "a trace of colonies painted the whole cell: {trace:?}",
        );
    }

    /// A merged mark stands for the systems the mode draws, and for
    /// nothing where the mode draws none of what it holds
    ///
    /// Reported as a lattice of grey fills over sky whose own systems
    /// were not being drawn: reading the sky as populations draws the
    /// systems anybody lives in, and a merged mark went on standing for
    /// its whole subtree — one grey mark a cell, which is a grid.
    #[test]
    fn a_merged_mark_stands_for_what_the_mode_draws() {
        use crate::map::galaxy::spawn::{ColorBy, Hue};
        let gains = crate::map::paint::glow::Gains::default();
        let empty = galos_index::read::inhabited::Inhabited::ZERO;

        // A cell of ten thousand systems with eight colonies in it.
        let held = inhabited(8);
        let crowd =
            average_mark(Some(&held), 10_000, ColorBy::Allegiance, &gains);
        let colonies =
            average_mark(Some(&held), 8, ColorBy::Allegiance, &gains);
        assert!(
            colonies.length() > crowd.length() * 2.,
            "the colonies were drowned in a crowd the mode does not draw: \
             {colonies:?} against {crowd:?}",
        );

        // And a cell nobody lives in stands for nothing at all in that
        // mode, where ordinarily it stands for its whole subtree.
        let grey = Hue::Grey.light()
            * crate::map::paint::glow::mark_light(Hue::Grey, false, &gains);
        let alone =
            average_mark(Some(&empty), 10_000, ColorBy::Allegiance, &gains);
        assert!((alone - grey).length() < 1e-9, "{alone:?}");
        assert_eq!(empty.count(), 0, "nothing to stand for");
    }

    /// A merged mark standing `at`, of a cell at `level`.
    fn blob(at: [f64; 3], level: u8) -> Blob {
        Blob {
            light: Vec3::splat(0.1),
            fade: 1.,
            id: CellId::of_point(at, level),
            count: 1_240,
            at,
            m_min: Some(2.0),
        }
    }

    /// A world with a camera at the origin looking down its negative Z, a
    /// window the size of that camera's frame, and the map's own pointer
    /// pass over whatever blobs are handed in.
    fn pointing(blobs: Vec<Blob>, cursor: Vec2) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<PointedBlob>();
        app.insert_resource(Blobs(blobs));
        app.world_mut().spawn((OrbitCamera::default(), seeing()));
        let mut window =
            Window { resolution: WindowResolution::new(800, 600), ..default() };
        window.set_cursor_position(Some(cursor));
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_systems(Update, point_at_blobs);
        app
    }

    /// Where a merged mark has to stand to land in the middle of that
    /// camera's frame: straight ahead, which is down negative Z.
    const AHEAD: [f64; 3] = [0., 0., -400.];

    /// The pointer catches a merged mark, though nothing on the map wears a
    /// component for it
    ///
    /// The whole reason this exists: blobs are a list and not entities, so
    /// the picker never sees them, and out at galaxy scale they are all
    /// there is to point at.
    #[test]
    fn the_pointer_catches_a_merged_mark() {
        let mut app = pointing(vec![blob(AHEAD, 8)], Vec2::new(400., 300.));
        app.update();
        let caught = app.world().resource::<PointedBlob>();
        assert_eq!(
            caught.0.map(|blob| blob.id),
            Some(CellId::of_point(AHEAD, 8)),
            "the mark under the pointer was not caught",
        );
    }

    /// And catches nothing while the pointer is off it
    #[test]
    fn the_pointer_off_a_mark_catches_nothing() {
        let mut app = pointing(vec![blob(AHEAD, 8)], Vec2::new(40., 30.));
        app.update();
        assert!(app.world().resource::<PointedBlob>().0.is_none());
    }

    /// Of two marks over one another, the nearer to the pointer wins
    ///
    /// Nearer to the *pointer*, not to the eye: each blob is a whole
    /// subtree, so two overlapping are two regions of sky in the same
    /// direction and depth says nothing about which is being aimed at.
    #[test]
    fn the_nearest_to_the_pointer_wins() {
        let aside = [6., 0., -400.];
        let mut app = pointing(
            vec![blob(AHEAD, 8), blob(aside, 8)],
            // A few pixels right of the middle, which is toward `aside`.
            Vec2::new(404., 300.),
        );
        app.update();
        let caught = app.world().resource::<PointedBlob>().0;
        assert_eq!(
            caught.map(|blob| blob.id),
            Some(CellId::of_point(aside, 8))
        );
    }

    /// A drawn system under the pointer wins over the merged mark behind it
    ///
    /// A blob is drawn where the systems it stands for are, so a system
    /// drawn over one is one of the very systems it is standing in for:
    /// answering with the blob as well would be the same sky pointed at
    /// twice.
    #[test]
    fn a_drawn_system_wins_over_the_mark_behind_it() {
        let mut app = pointing(vec![blob(AHEAD, 8)], Vec2::new(400., 300.));
        app.world_mut().spawn(PointedAt::reached(0.));
        app.update();
        assert!(app.world().resource::<PointedBlob>().0.is_none());
    }

    /// The system a merged mark stands for is the head of its cell's own
    /// payload, which is the brightest thing under it
    ///
    /// The tree's slices are magnitude-ordered from the root down, so a
    /// cell owns the brightest of its whole subtree and nothing deeper has
    /// to be read to name what the mark stands for.
    #[test]
    fn a_mark_is_named_by_the_head_of_its_cell() {
        let id = CellId::of_point(AHEAD, 8);
        let empty = Populated::default();
        let mut prominent = Prominent::default();
        assert!(
            prominent.of(id, false, &empty).is_none(),
            "nothing is known unasked",
        );

        let head = Point {
            id64: 7,
            pos: [1., 2., 3.],
            magnitude: 1.5,
            temp_bucket: 4,
            updated_at: 0,
            kind: StarKind::Unknown,
        };
        prominent.known.insert(id, vec![head]);
        assert_eq!(
            prominent.of(id, false, &empty).map(|point| point.id64),
            Some(7),
        );
    }

    /// One system of a merged mark's prefix, at `magnitude`.
    fn point(id64: u64, magnitude: f32) -> Point {
        Point {
            id64,
            pos: [1., 2., 3.],
            magnitude,
            temp_bucket: 4,
            updated_at: 0,
            kind: StarKind::Unknown,
        }
    }

    /// Everyone lives on `busiest`, and nobody anywhere else.
    fn lived_on(busiest: i64) -> Populated {
        Populated(std::sync::Arc::new(
            [(
                busiest,
                galos_index::records::PopulatedSystem {
                    address: busiest,
                    name: "Busy".into(),
                    position: [0.; 3],
                    population: 40_000,
                    security: None,
                    government: None,
                    allegiance: None,
                    primary_economy: None,
                    secondary_economy: None,
                    factions: Vec::new(),
                    body_count: None,
                    non_body_count: None,
                },
            )]
            .into_iter()
            .collect(),
        ))
    }

    /// While marks are drawn by population, the mark stands for the
    /// busiest system under it and not the brightest
    ///
    /// A mark's *size* is the population it carries in that mode, so the
    /// biggest is what the eye is aiming at — the same rule the marks
    /// themselves are drawn in. Off the light, the brightest is what a
    /// mark says and what it answers with.
    #[test]
    fn what_a_mark_stands_for_follows_what_is_drawn() {
        let id = CellId::of_point(AHEAD, 8);
        let mut prominent = Prominent::default();
        // In magnitude order, as a payload is: the brightest heads it.
        prominent.known.insert(id, vec![point(1, 0.5), point(2, 6.0)]);
        let populated = lived_on(2);

        assert_eq!(
            prominent.of(id, false, &populated).map(|point| point.id64),
            Some(1),
            "drawing light, a mark is the brightest under it",
        );
        assert_eq!(
            prominent.of(id, true, &populated).map(|point| point.id64),
            Some(2),
            "drawing population, a mark is the busiest under it",
        );
    }

    /// A cell whose payload read back nothing answers nothing, and is not
    /// asked again: an empty prefix is an answer.
    #[test]
    fn a_mark_over_nothing_readable_is_asked_once() {
        let id = CellId::of_point(AHEAD, 8);
        let mut prominent = Prominent::default();
        prominent.known.insert(id, Vec::new());
        assert!(prominent.of(id, false, &Populated::default()).is_none());
        assert!(
            prominent.known.contains_key(&id),
            "the answer was forgotten, so it would be asked again",
        );
    }
}

/// What each merged mark of the plan stands for: the colour it is painted in
/// and whether the filters admit anything under it
///
/// **A merged mark is the marks it replaces, and it has to behave like
/// them.** Before this it was painted the palette's grey whatever it stood
/// over and ignored the filters outright, so at galaxy scale — where nearly
/// every mark on the map is a merged one — the colour axis and every filter
/// stopped at the frontier: the field behind the marks was tinted by the
/// politics of a region while the marks in front of it were grey, and a
/// faction filter dimmed the handful of drawn stars and left the galaxy
/// standing.
///
/// **Once a plan, not once a frame.** Both answers are functions of the
/// plan, the political table, the colour axis and the filters, none of
/// which a still camera changes; the plan itself is rebuilt only when the
/// eye moves enough to change it. Measured over `.index/full`, a wide view
/// holds some twenty thousand merged marks, and a lookup apiece in the
/// political table every frame is the kind of per-frame probe that cost
/// seven milliseconds a frame when the blobs read their own centroids that
/// way.
///
/// Indexed by the plan's own blob order, so a reader walks the two
/// together.
#[derive(Resource, Default)]
pub struct Standing {
    marks: Vec<Mark>,
    /// The filter revision these were weighed against.
    revision: u32,
}

/// One merged mark, weighed.
#[derive(Copy, Clone, Default)]
struct Mark {
    /// The average of the marks it stands for, in linear light: already
    /// premultiplied, as a system's own mark colour is.
    light: Vec3,
    /// What share of what it stands for the filters admit, `0.0..=1.0`;
    /// see [`crate::map::filter::Filters::admitted_share`].
    share: f32,
    /// How many systems it stands for, which is what it is thinned
    /// against — the whole subtree ordinarily, and only the systems
    /// anybody lives in while the sky is read as populations, that mode
    /// drawing none of the rest.
    stands_for: u64,
}

impl Standing {
    /// Weighed by hand, for a test that is checking what the draw does with
    /// the share rather than how it was reached.
    #[cfg(test)]
    pub(crate) fn weighed(marks: Vec<(Vec3, f32, u64)>) -> Standing {
        Standing {
            marks: marks
                .into_iter()
                .map(|(light, share, stands_for)| Mark {
                    light,
                    share,
                    stands_for,
                })
                .collect(),
            revision: 0,
        }
    }

    /// The light the plan's `offer`th merged mark is painted at, and what
    /// share of what it stands for the filters admit
    ///
    /// Nothing where it has not been weighed, which is the frame a plan
    /// lands on: the draw takes the mark whole and grey for that one frame
    /// rather than dropping it, a mark blinking out for a frame as the eye
    /// moves being worse than a mark a frame behind on its colour.
    pub(crate) fn of(&self, offer: usize) -> Option<(Vec3, f32, u64)> {
        self.marks
            .get(offer)
            .map(|mark| (mark.light, mark.share, mark.stands_for))
    }
}

/// Weigh every merged mark of the plan: its colour, and the filters' verdict
///
/// Skipped whole where nothing it reads has moved. The filters are watched
/// by revision rather than by change detection: `Filters` is written by the
/// panel every frame it is open, and a rebuild a frame while a form is up is
/// twenty thousand lookups for an answer nobody changed.
pub(crate) fn weigh_blobs(
    planned: Res<crate::map::galaxy::plan::Planned>,
    settled: Res<crate::map::index::Settled>,
    index: Res<crate::map::index::ResidentIndex>,
    populated: Res<Populated>,
    names: Res<Names>,
    filtering: crate::map::filter::Filtering,
    color_by: Res<crate::map::galaxy::spawn::ColorBy>,
    view: Res<crate::map::paint::sizing::View>,
    scale_population: Res<crate::map::paint::sizing::ScalePopulation>,
    gains: Res<crate::map::paint::glow::Gains>,
    mut standing: ResMut<Standing>,
    mut named: ResMut<Named>,
) {
    let revision = filtering.filters.revision();
    let moved = planned.is_changed()
        || settled.is_changed()
        || view.is_changed()
        || scale_population.is_changed()
        || color_by.is_changed()
        || gains.is_changed()
        || revision != standing.revision;
    if !moved {
        return;
    }
    if named.revision != revision {
        named.rebuild(
            revision,
            &filtering.filters,
            &index.0,
            &populated,
            &names,
        );
    }

    // What a merged mark stands for. Ordinarily its whole subtree; while
    // the sky is read as populations, only the systems anybody lives in —
    // that mode draws none of the others, so a merged mark that went on
    // standing for the crowd was a grey mark over a sky whose own systems
    // are not drawn. Reported as a lattice of grey fills where the mode
    // had drawn nothing but colonies.
    let populated_only =
        crate::map::paint::sizing::by_population(&view, &scale_population);
    standing.revision = revision;
    standing.marks.clear();
    standing.marks.extend(planned.0.blobs.iter().map(|blob| {
        let held = settled.0.get(blob.id);
        let stands_for = match populated_only {
            true => {
                held.map_or(0, galos_index::read::inhabited::Inhabited::count)
            }
            false => blob.count,
        };
        Mark {
            light: average_mark(held, stands_for, *color_by, &gains),
            share: filtering.filters.admitted_share(
                &blob.aged,
                named.held(blob.id).whole(),
                stands_for,
            ),
            stands_for,
        }
    }));
}

/// How many of a cell's systems the picking filters name, and how many of
/// those anybody lives in
///
/// **A faction, a route and a hand-picked set each name a set of
/// addresses**, and every one of those addresses sits in a known place, so
/// the cells that hold them are the tree's own descent from the root to
/// each: [`galos_index::Index::descend`]. That is the whole of what a
/// merged mark can be asked about them, and it is exact — a mark is
/// admitted if and only if something it stands for is.
///
/// Built once a filter revision. A faction of a few thousand systems is a
/// few thousand descents of a dozen steps; the alternative is a set of
/// addresses no merged mark could ever be weighed against, which is what
/// the map did.
#[derive(Resource, Default)]
pub struct Named {
    cells: rustc_hash::FxHashMap<CellId, Held>,
    revision: u32,
}

/// What a cell holds of what the filters name: the systems anybody lives
/// in, and the rest
///
/// Split, because the field's two channels stand for different halves of a
/// cell. A faction names none but populated systems, so the colony channel
/// keeps its share of the light while the grey backdrop — which holds no
/// faction member at all — falls to the dim. Answered together, the
/// backdrop would keep light for systems no such filter could ever admit.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// Named systems with a population.
    pub populated: u32,
    /// Named systems with none.
    pub alone: u32,
}

impl Held {
    /// Named either way, which is what a merged mark stands over.
    pub fn whole(self) -> u32 {
        self.populated + self.alone
    }
}

impl Named {
    fn rebuild(
        &mut self,
        revision: u32,
        filters: &crate::map::filter::Filters,
        index: &galos_index::Index,
        populated: &Populated,
        names: &Names,
    ) {
        let _zone = info_span!("picked cells").entered();
        self.revision = revision;
        self.cells.clear();
        let cells = &mut self.cells;
        let mut hold = |at: [f64; 3], populated: bool| {
            index.descend(at, |id| {
                let held = cells.entry(id).or_default();
                match populated {
                    true => held.populated += 1,
                    false => held.alone += 1,
                }
            });
        };
        for filter in filters.picking() {
            match filter {
                crate::map::filter::Filter::Faction { id, .. } => {
                    for system in populated.0.values() {
                        if system.factions.contains(&id) {
                            hold(
                                [
                                    f64::from(system.position[0]),
                                    f64::from(system.position[1]),
                                    f64::from(system.position[2]),
                                ],
                                true,
                            );
                        }
                    }
                }
                crate::map::filter::Filter::Route { systems, .. }
                | crate::map::filter::Filter::Systems { systems, .. } => {
                    for &address in systems {
                        let (at, lived_in) = match populated.get(address) {
                            Some(known) => (
                                [
                                    f64::from(known.position[0]),
                                    f64::from(known.position[1]),
                                    f64::from(known.position[2]),
                                ],
                                known.population > 0,
                            ),
                            None => (names.placed(address).into(), false),
                        };
                        hold(at, lived_in);
                    }
                }
                // Answered off the aggregate's own age column instead; see
                // [`crate::map::filter::Filters::admitted_share`].
                crate::map::filter::Filter::Recency { .. } => {}
            }
        }
    }

    /// What this cell holds of what the picking filters name.
    pub fn held(&self, id: CellId) -> Held {
        self.cells.get(&id).copied().unwrap_or_default()
    }
}

/// The average of the marks a merged mark stands for, in linear light
///
/// **A merged mark is one mark because everything under it falls inside
/// one, so what it is painted is what those marks come to.** Summed as
/// light and divided by the count: a cell of ten thousand systems with a
/// dozen imperial colonies in it is grey with a trace of imperial in it,
/// which is what the same patch looks like when the camera comes in far
/// enough to draw the ten thousand. The merge boundary is the test, and it
/// is the reason this is an average and not the dominant bucket — a cell
/// painted by its loudest colony would change colour the moment it split.
///
/// Reduces to exactly what a merged mark was painted before — the palette's
/// grey at an uninhabited system's level — for a cell with no colonies in
/// it, which is most of the galaxy.
fn average_mark(
    held: Option<&galos_index::read::inhabited::Inhabited>,
    count: u64,
    color_by: crate::map::galaxy::spawn::ColorBy,
    gains: &crate::map::paint::glow::Gains,
) -> Vec3 {
    let mut light = Vec3::ZERO;
    let mut counted = 0u64;
    if let Some(held) = held {
        crate::map::paint::glow::political(held, color_by, |hue, systems| {
            light += hue.light()
                * crate::map::paint::glow::mark_light(hue, true, gains)
                * systems as f32;
            counted += u64::from(systems);
        });
    }
    let alone = count.saturating_sub(counted) as f32;
    let grey = crate::map::galaxy::spawn::Hue::Grey;
    light += grey.light()
        * crate::map::paint::glow::mark_light(grey, false, gains)
        * alone;
    light / count.max(1) as f32
}
