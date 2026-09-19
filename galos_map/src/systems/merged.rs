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
//! ([`super::bounded::Blobs`]) and this is the one place that reads the
//! pointer against it.
//!
//! **What resolving one costs is one read, on the pointer resting.** A
//! blob's cell owns the brightest of its own subtree — the tree's slices
//! are magnitude-ordered from the root down — so the system a blob stands
//! for is the head of the cell's own payload and nothing deeper has to be
//! read for it. Cached by cell, since the pointer crossing a galaxy of
//! merged marks would otherwise ask for one a frame.

use crate::camera::OrbitCamera;
use crate::systems::bounded::{Blob, Blobs, build_from_point};
use crate::systems::labels::screen_position;
use crate::systems::pointing::{DRAG_THRESHOLD, DragDistance, PointedAt};
use crate::systems::selection::{Picked, Selection};
use crate::systems::{MapSet, System};
use crate::{Names, Populated, Transport};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use galos_index::{CellId, Point};
use rustc_hash::FxHashMap;

pub fn plugin(app: &mut App) {
    app.init_resource::<PointedBlob>();
    app.init_resource::<Prominent>();
    app.add_systems(
        Update,
        (point_at_blobs, name_blobs, click_blobs)
            .chain()
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
    );
    // Ringed in the same pass and the same layer the pointer's own ring is
    // drawn in, and after it, so a merged mark and a system are marked the
    // same way and neither is drawn over the other.
    // Gated on the index being read, as [`crate::schedule`] gates the
    // whole `Update` pipeline: this reads the tables that read delivers,
    // and the egui pass runs from the first frame, before there are any.
    app.add_systems(
        EguiPrimaryContextPass,
        ring_blob
            .after(super::pointing::ring)
            .before(super::labels::draw_names)
            .run_if(in_state(crate::loading::Opening::Drawn)),
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
    view: Res<super::scale::View>,
    scale_population: Res<super::scale::ScalePopulation>,
) -> Result {
    let Some(blob) = pointed.0 else { return Ok(()) };
    let Ok((orbit, camera)) = cameras.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else {
        return Ok(());
    };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let Some(at) = screen_position(
        orbit,
        cot_half_fov,
        viewport,
        DVec3::from(blob.at),
    ) else {
        return Ok(());
    };

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(super::labels::annotations_layer());
    let color = super::labels::color32(super::pointing::INDICATOR);
    painter.circle_stroke(
        egui::pos2(at.x, at.y),
        CATCH_PX,
        egui::Stroke::new(super::pointing::RING_STROKE, color),
    );

    let systems = blob.count;
    let by_population =
        super::scale::by_population(&view, &scale_population);
    let said = match prominent
        .of(blob.id, by_population, &populated)
        .map(|point| build_from_point(point, &populated, &names))
    {
        Some(system) => format!("{} · {systems} systems", system.name),
        None => format!("{systems} systems"),
    };
    painter.text(
        egui::pos2(at.x + CATCH_PX + super::labels::NAME_HEIGHT * 0.5, at.y),
        egui::Align2::LEFT_CENTER,
        said,
        egui::FontId::proportional(super::labels::NAME_HEIGHT),
        color,
    );
    Ok(())
}

/// How near the pointer has to come to a merged mark to be on it, in pixels
///
/// A blob is drawn at the field's smallest radius, which is under a pixel,
/// and a target under a pixel is a target nobody can hit. This is the same
/// figure the pointer catches an ordinary mark over
/// ([`super::pointing::INDICATOR_MIN_RADIUS`]) so that a merged mark and a
/// drawn system answer the pointer over the same area and the sky does not
/// change how it behaves as cells merge.
const CATCH_PX: f32 = super::pointing::INDICATOR_MIN_RADIUS;

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
    /// because there a mark's *size* is how many people live there and the
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
/// After [`super::pointing::point_at`], and nothing while that found
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
    gesture: crate::ui::Gesture,
    pointed: Res<PointedBlob>,
    prominent: Res<Prominent>,
    dragged: Query<&DragDistance>,
    keys: Res<ButtonInput<KeyCode>>,
    populated: Res<Populated>,
    names: Res<Names>,
    view: Res<super::scale::View>,
    scale_population: Res<super::scale::ScalePopulation>,
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
        super::scale::by_population(&view, &scale_population);
    let Some(point) = prominent.of(blob.id, by_population, &populated) else {
        return;
    };
    // Held down, a modifier gathers rather than replaces, exactly as it
    // does over a drawn system; see `super::spawn::select_on_click`.
    let gathering = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
    ]);
    let system: System = build_from_point(point, &populated, &names);
    selection.pick(Picked::System(system), gathering);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::seeing;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use galos_index::meta::StarKind;

    /// A merged mark standing `at`, of a cell at `level`.
    fn blob(at: [f64; 3], level: u8) -> Blob {
        Blob {
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
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
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
        assert_eq!(caught.map(|blob| blob.id), Some(CellId::of_point(aside, 8)));
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
                galos_index::meta::PopulatedSystem {
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
    /// A mark's *size* is how many people live there in that mode, so the
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
