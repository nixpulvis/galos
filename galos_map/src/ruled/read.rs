//! What is drawn over a ruled plane rather than into it
//!
//! A plane's own lines and the numbers along them are painted by the shader.
//! What stands over it is here: the crosses that mark a place worth locating,
//! the lines dropped to it from whatever is off it, and the three numbers about
//! each of those places.
//!
//! Everything here fades out towards the plane's horizon with the ruling, so a
//! number standing over the plane goes as the plane goes. What it does not do
//! is follow the pitch: see [`faded`].
use super::{
    BARE, Face, INK, MAJOR, NONE, Numbered, Plane, Unit, off_plane, told,
};
use bevy::ecs::system::SystemParam;
use bevy::math::{DMat3, DVec3, Vec2};
use bevy::prelude::*;
use bevy_rich_text3d::{
    Text3d, Text3dSegment, Text3dStyling, TextAnchor, TextAtlas,
};
use big_space::prelude::*;

/// How sharply a plane goes as it is turned edge on
///
/// The cosine below which the ruling has gone entirely, which loses the plane
/// as the camera comes level with it. What [`super::Plane::edge_on`] is set to
/// unless a caller says otherwise, and the shader's own term. Nothing standing
/// over the plane takes it; see [`faded`].
pub const EDGE_ON: f32 = 0.25;

/// How long each arm of a cross marking a place on the plane is, in pixels
///
/// The numbers at the middle are about one point on the plane, and a number
/// written over a plane with nothing under it is a number floating loose. So
/// the point is marked, along the plane's own axes, and they stand beside it.
///
/// The arms are laid in the plane rather than across the screen, so a cross
/// out towards the horizon is foreshortened the way the cells around it are.
/// It is a mark scratched on the plane and not a pointer laid over it.
pub const CROSS: f32 = 11.;

/// How tall a number standing over the plane draws, in logical pixels
///
/// The line box, which for one line of text is the size the face is set at.
/// The size the chrome's smallest lettering is set at: these are read at a
/// glance off a map rather than pored over, and there are up to a dozen of
/// them on screen at once.
pub const READS: f32 = 8.;

/// How far off the plane the middle's numbers are hung, in pixels
///
/// The two rulers lie in the plane and their numbers run along them. The third
/// is about the plane itself, so it is hung along the one direction on screen
/// that neither ruler runs in. Drawn where they cross it reads as one more
/// number in the row.
///
/// Under the plane rather than over it, which is the opposite side from the one
/// a pair on the plane is written on. The two are then on either side of the
/// lines they are both about, and the middle is read against a clear row rather
/// than into a number.
///
/// Far enough down to clear what is drawn around the place itself. The arms of
/// the cross reach [`CROSS`] from it and the ring around a thing picked out
/// reaches further still, so a row hung to clear the cross alone lands inside
/// the ring of a selection the camera is looking straight at.
pub const LIFT: f32 = 24.;

/// And how far to the side of a dropped line its own number stands, in pixels
///
/// Beside the line rather than over it, for the same reason a pair on the plane
/// stands beside its crossing: a number with a rule through it is a number to
/// be worked out rather than read.
pub const ASIDE: f32 = 6.;

/// How far a row of numbers reaches around the point it is about, in pixels
///
/// About the row the map writes there: three numbers each with its own power, a
/// unit and two commas comes to some forty characters of a [`READS`] tall
/// monospaced face, centred on the point, so it runs about ninety five either
/// side. Across it the [`LIFT`] that hangs it off the plane and half its own
/// height.
///
/// In pixels rather than in the plane's own units because the row holds one
/// size on screen and the plane does not. A unit of plane covers most of a
/// digit's width on screen with the camera overhead and a fraction of one with
/// the camera down near the plane, so a reach fixed in units is a reach that
/// means something different at every pitch. [`stand_clear`] converts.
pub const CROWDS: Vec2 = Vec2::new(96., 30.);

/// How much is left at a point on the plane, as the ruling fades out
///
/// The plane is unbounded, so everything on it fades away towards its horizon.
/// `reach` is how far that runs. Whatever stands over the plane carries the
/// same fade or it goes on standing over a ruling that has gone.
///
/// What sets a number apart from a line is the ink it starts in and nothing
/// else, [`super::INK`] against [`super::MINOR`] or [`super::MAJOR`], so the
/// numbers hold on well after the lines have gone. Which is the right way
/// round, a ruler being read off its numbers.
///
/// The pitch does not enter. The lines themselves are lost as the plane is
/// turned edge on, because a ruling at a grazing angle is moire rather than
/// lines, and [`EDGE_ON`] is where the shader gives up on them. Nothing here
/// has that trouble: a number is drawn facing the camera and a dropped line
/// stands across the plane rather than along it. Fading them with the lines
/// would take the numbers away exactly when the view is too flat to read
/// anything else off the plane, which is when they are the only thing left
/// worth reading.
pub fn faded(from_eye: DVec3, reach: f64) -> f32 {
    let far = from_eye.length();
    if far <= 0. || reach <= 0. {
        return 1.;
    }
    (1. - far / reach).clamp(0., 1.) as f32
}

/// How strongly something the ruling draws comes out, once the caller has had
/// its say
///
/// One knob over the whole of it. The lines and the numbers along them are one
/// thing seen at once, and a ruler whose lines dimmed while its numbers did not
/// would read as two.
///
/// Never past whole, an alpha having nowhere above one to go.
pub fn drawn_at(strength: f32, bright: f32) -> f32 {
    (strength * bright).clamp(0., 1.)
}
/// Where a plane's rulers stand, and what they are said in
///
/// Written by whoever rules the plane, every frame, before anything here is
/// read. How wide a cell is and how far apart the numbers go are [`ladder`]'s
/// to answer and the caller's to ask; this is where the answers land.
///
/// Nothing is drawn over a plane whose [`Reading::strength`] has run out. A
/// number standing over a ruling that has gone is a number about nothing.
#[derive(Component, Clone, Copy, Debug)]
pub struct Reading {
    /// Where the camera is looking, in [`Reading::unit`] along the plane's own
    /// axes from its origin
    ///
    /// The middle of the view, which is where the plane hangs and where the
    /// three numbers of the place being looked at are said. Not snapped, so it
    /// sits still while the plane slides under it. Its `y` is therefore the
    /// altitude the plane itself is hung at.
    pub at: DVec3,
    /// Where the camera's eye is, likewise
    ///
    /// What turns a place on this plane into an offset from the eye, which is
    /// where everything drawn over it is hung.
    pub eye: DVec3,
    /// How far apart two numbers are, in [`Reading::unit`]
    pub step: f64,
    pub unit: Unit,
    /// How much of the ruling is drawn, which everything over it follows
    pub strength: f32,
    /// How loudly the whole of it is asked for, over that
    pub bright: f32,
    /// Whether the place the camera is looking at is said out loud
    pub middle: bool,
}

impl Default for Reading {
    fn default() -> Self {
        Reading {
            at: DVec3::ZERO,
            eye: DVec3::ZERO,
            step: 0.,
            unit: Unit { metres: 1., mark: "m" },
            strength: 0.,
            bright: 1.,
            middle: false,
        }
    }
}

impl Reading {
    /// Where something on this plane lies from the camera's eye, in whatever
    /// the world is drawn in and along the world's own axes
    ///
    /// `facing` is the plane's, [`Plane::facing`]. The rulers lie in the plane
    /// and count along its axes; everything drawn stands in the world and is
    /// placed along its. A plane lying flat makes the two the same, and one
    /// tilted does not, so the turn is done here rather than left out.
    pub fn seen_from_eye(&self, facing: Quat, place: DVec3) -> DVec3 {
        facing.as_dquat() * ((place - self.eye) * self.unit.metres)
    }

    /// And where the middle of the view is, likewise
    ///
    /// The place the plane is hung through, which is on it by construction.
    pub fn middle_from_eye(&self, facing: Quat) -> DVec3 {
        self.seen_from_eye(facing, self.at)
    }
}

/// Something a plane should locate
///
/// A line is dropped to the plane from wherever it stands, with the three
/// numbers the plane can say about it under the foot and how far off the plane
/// it went beside the line. What is worth pointing out is the caller's to
/// decide; put this on it and the plane says where it is.
///
/// Placed by whatever grid it hangs from, which need not be the plane's. The
/// two are crossed in `f64` through [`super::seen`], so a thing in one grid is
/// located against a plane in another exactly.
#[derive(Component)]
pub struct Located;

/// What the lines dropped to a plane are about
///
/// One entry per [`Located`] thing that is drawn. Worked out by [`locate`],
/// and read by [`readouts`], which writes the numbers, and [`marks`], which
/// draws the lines.
#[derive(Component, Default)]
pub struct Dropped(Vec<Drop>);

/// One line dropped to the plane, and what it is about
struct Drop {
    /// Where the thing itself stands, as an offset from the camera's eye
    ///
    /// The head of the line, the other end being straight below it on the
    /// plane.
    top: DVec3,
    /// Where its foot stands on the plane, likewise
    ///
    /// Where the thing is marked and where the two numbers the plane can locate
    /// it by are written.
    foot: DVec3,
    /// And where its middle stands, likewise, which is where the third is
    middle: DVec3,
    /// Where the thing stands, in [`Reading::unit`]
    at: DVec3,
}

/// Everything a located thing is asked for
type Mark = (
    Entity,
    &'static CellCoord,
    &'static Transform,
    &'static ViewVisibility,
);

/// Work out what each plane is worth marking, and where those places stand
///
/// The two rulers give a place on the plane, and a line dropped to it gives
/// the height above it. That is the third of the three numbers and the one a
/// plane on its own cannot say. Dropped from everything [`Located`]; the point
/// the camera is looking at needs none, the plane running through it.
///
/// Where a thing stands is asked of the thing itself rather than measured out
/// from the camera. A cell is an `i64` count and a transform is the offset
/// inside it, so this is the position and has nothing of the camera in it.
///
/// Measured out from the camera it would have: the camera is at one float's
/// remove and the thing at another, and neither remove cancels the other. It
/// comes to a ten thousandth of the last place written, which is nothing at all
/// until the number sits on a rounding boundary, and a coordinate stored in
/// thirty seconds of a light year sits on one about a third of the time. Then
/// it turns over and back as the camera swings.
pub(super) fn locate(
    mut planes: Query<(
        Entity,
        &Plane,
        &Reading,
        &CellCoord,
        &Transform,
        &mut Dropped,
    )>,
    // Whether a thing is drawn as well as located. Something the caller has
    // hidden is not there to be located, and a line dropped from where it
    // would have stood is a line about nothing. Settled in `PostUpdate`, so
    // this is last frame's answer.
    located: Query<Mark, With<Located>>,
    grids: Grids,
) {
    for (entity, plane, reading, cell, transform, mut dropped) in &mut planes {
        dropped.0.clear();
        let Some(grid) = grids.parent_grid(entity) else { continue };
        if reading.strength <= 0. || reading.unit.metres <= 0. {
            continue;
        }
        // Where the plane stands, in the frame everything is drawn in, and
        // where it stands in the space its own numbers count: at the origin
        // sideways and at the ruled altitude in `y`, which is what makes a
        // crossing count absolutely. `at.y` is that altitude.
        let stands = super::seen(grid, cell, transform);
        let hangs = DVec3::new(0., reading.at.y, 0.);
        let square = DMat3::from_quat(plane.facing.as_dquat().inverse());

        for (marked, cell, transform, shown) in &located {
            if !shown.get() {
                continue;
            }
            let Some(grid) = grids.parent_grid(marked) else { continue };
            let world = super::seen(grid, cell, transform);
            // Onto the plane's own axes, which is what the rulers count along
            // and so what the numbers are about.
            let at = hangs
                + square * (world - stands) / reading.unit.metres;

            // Measured from the eye through the reading rather than through
            // the floating origin. `big_space` settles where each grid thinks
            // the origin stands in `PostUpdate`, so in `Update` that answer is
            // last frame's, and an offset taken from it against a camera that
            // has moved this frame is an offset that swings as the camera
            // does. It cancels out of `at`, both ends of that difference being
            // crossed through the same one; it does not cancel here.
            let top = reading.seen_from_eye(plane.facing, at);
            // Straight below it on the plane, which is the same place at the
            // plane's own altitude. Said in the space rather than by dropping
            // a component of the offset, because the two agree only while the
            // plane lies flat.
            let under = DVec3::new(at.x, reading.at.y, at.z);
            let foot = reading.seen_from_eye(plane.facing, under);
            dropped.0.push(Drop { top, foot, middle: (top + foot) / 2., at });
        }
    }
}

/// The world size a readout's text mesh is built at
///
/// The line box, which whatever places it scales down to the height it wants
/// it drawn at. Any figure does; this one keeps the scale near one.
const SIZE: f32 = 64.;

/// The nearest a readout is sized against, in whatever the world is drawn in
///
/// Size follows depth, so a place level with the camera would draw at nothing
/// and one behind it at a mirrored negative. Anything this close is inside the
/// near plane regardless.
const MIN_DEPTH: f32 = 1.;

/// One of the numbers standing over a plane
///
/// Text in the world rather than painted on the screen afterwards, so that a
/// number is drawn into the same pass the plane is, goes through the same
/// tonemapping, and comes out at the strength it was asked for. It is also
/// what lets whatever is in front of one hide it.
#[derive(Component)]
pub struct Readout;

/// The readouts there are, in the order they are handed work
///
/// An ordered list rather than a query, so that the same readout goes on
/// saying the same thing from one frame to the next. Walked in query order the
/// numbers would swap between entities whenever an archetype moved, and a
/// number that changes which mesh it is drawn from is a number that flickers.
///
/// As long as there is anything to say and no longer. It follows what is
/// located rather than what was said about it, so a thing drifting onto the
/// plane and off it again costs nothing, and a selection let go of gives its
/// readouts back rather than leaving them resident for the session.
#[derive(Resource, Default)]
pub struct Readouts(Vec<Entity>);

/// One number standing over a plane, and where it stands
struct Says {
    /// The place it is about, as an offset from the camera's eye
    from_eye: DVec3,
    /// Which way it is hung off that place, and how far, in pixels
    ///
    /// A direction through the world and a length on screen. What the length
    /// comes to in the world is worked out where the readout is placed, from
    /// how deep into the view the place lies.
    hung: Vec3,
    /// Which side of the place it stands on
    anchor: TextAnchor,
    /// And what it is drawn in, being the plane's own
    ///
    /// A number over a ruling and a line of that ruling are one piece of
    /// chrome. Drawn in two colours they read as two.
    hue: Color,
    said: String,
    /// The ink it is written in, before the ruling's own strength and the
    /// caller's knob have had their say
    ink: f32,
}

/// Everything a plane has to say about itself this frame
///
/// The middle of the view first, then each thing located: where it stands, and
/// how far off the plane it went. In a settled order, so that a readout goes
/// on saying what it said last frame while nothing has changed.
///
/// `sideways` is which way the right of the view runs through the world.
fn spoken(
    plane: &Plane,
    reading: &Reading,
    dropped: &Dropped,
    sideways: Vec3,
) -> Vec<Says> {
    let reach = plane.reach;
    // What the plane can say about one place on it, hung under the mark there.
    // The middle of the view and the foot of every dropped line are the same
    // kind of thing, a place on the plane worth locating, so they are said the
    // same way.
    let placed = |from_eye: DVec3, at: DVec3| Says {
        from_eye,
        // Along the one direction neither ruler runs in, and under the plane
        // rather than over it, which is the opposite side from the one a pair
        // on the plane is written on. The two are then on either side of the
        // lines they are both about. Squared up on the plane it comes to
        // nothing on screen and the row sits on its own mark, which is a view
        // with no room for a third number in it anyway.
        hung: plane.facing * Vec3::NEG_Y * LIFT,
        anchor: TextAnchor::CENTER,
        hue: plane.color,
        said: format!("{} {}", told(at, reading.step), reading.unit.mark),
        ink: INK * faded(from_eye, reach),
    };

    let mut says = Vec::new();

    // The place the camera is looking at, all three of it, held at the middle
    // of the view. Not snapped to anything, so it sits still while the plane
    // slides under it, and said to the same step the plane is numbered in so
    // that it reads against those numbers.
    if reading.middle {
        says.push(placed(reading.middle_from_eye(plane.facing), reading.at));
    }

    // And every line dropped to the plane, which is the same three numbers
    // about something that is not at the middle. They are said in full under
    // the mark at the line's foot, where the two rulers can be read against
    // them; the line itself carries only how far off the plane it went, which
    // is the one thing about it neither ruler nor mark can show.
    for drop in &dropped.0 {
        says.push(placed(drop.foot, drop.at));

        // A slot whether or not there is anything to put in it. A thing
        // sitting on the plane says nothing about how far off it is, and a
        // slot that came and went would slide every readout below it along one
        // place, so a number would change which mesh it is drawn from as a
        // selection drifted across the plane.
        let said = off_plane(drop.at.y - reading.at.y, reading.step, reading.unit);
        let silent = said.is_none();
        says.push(Says {
            from_eye: drop.middle,
            // Beside the line rather than over it, for the same reason a pair
            // on the plane stands beside its crossing: a number with a rule
            // through it is a number to be worked out rather than read.
            hung: sideways * ASIDE,
            anchor: TextAnchor::CENTER_RIGHT,
            hue: plane.color,
            said: said.unwrap_or_default(),
            ink: if silent { 0. } else { INK * faded(drop.foot, reach) },
        });
    }

    says
}

/// Where the camera stands
type Eye = (&'static Transform, &'static Camera);

/// And what it is not
///
/// `Without<Readout>` is already true of any camera. It is spelled out so the
/// scheduler can prove the query disjoint from the one that writes a readout's
/// `Transform`, which it would otherwise take to overlap.
type NotARow = (With<FloatingOrigin>, Without<Readout>);

/// Everything one readout is written through
///
/// Where it stands, what it says, which side of its place it says it on,
/// whether it says anything at all, and what it is drawn in.
type Written = (
    &'static mut Transform,
    &'static mut Text3d,
    &'static mut Text3dStyling,
    &'static mut Visibility,
    &'static MeshMaterial3d<StandardMaterial>,
);

/// Everything it takes to stand a plane's numbers over it
#[derive(SystemParam)]
pub(super) struct Standing<'w, 's> {
    planes: Query<'w, 's, (&'static Plane, &'static Reading, &'static Dropped)>,
    eyes: Query<'w, 's, Eye, NotARow>,
    pool: ResMut<'w, Readouts>,
    written: Query<'w, 's, Written, With<Readout>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    commands: Commands<'w, 's>,
}

/// Stand the numbers each plane is worth over the places they are about
///
/// Each is a child of the camera turned no further, which is what makes it
/// face the camera however the camera swings, and held at [`READS`] pixels
/// tall whatever it is standing over.
pub(super) fn readouts(face: Face) -> impl FnMut(Standing) {
    move |it: Standing| {
        let Standing {
            planes,
            eyes,
            mut pool,
            mut written,
            mut materials,
            mut commands,
        } = it;
        // Which way the camera is standing, and what it can see. The first is
        // its own transform and is always there; the second is the render
        // target's to answer and is not. So how many readouts there are
        // follows the first, and only where they stand waits on the second: a
        // window that cannot say how large it is is a window with nothing
        // drawn in it, not a reason to unmake what it is about.
        let eye = eyes.single().ok();
        let facing = eye.map(|(at, _)| at.rotation);
        let seen = eye.and_then(|(at, camera)| {
            let viewport = camera.logical_viewport_size()?;
            let cot_half_fov = camera.clip_from_view().y_axis.y;
            Some((at.translation, at.rotation, viewport, cot_half_fov))
        });

        // Every plane at once, though only one is normally drawn. Two rulings on
        // screen together beat against each other, which is a thing for whoever
        // rules them to avoid; what is drawn over them follows what is drawn.
        let mut says = Vec::new();
        let mut inks = Vec::new();
        if let Some(facing) = facing {
            for (plane, reading, dropped) in &planes {
                if reading.strength <= 0. {
                    continue;
                }
                let sideways = facing * Vec3::X;
                for one in spoken(plane, reading, dropped, sideways) {
                    inks.push(drawn_at(one.ink * reading.strength, reading.bright));
                    says.push(one);
                }
            }
        }

        // As many readouts as there is anything to say, and no more. One made
        // now is not in the world until the commands are flushed, so it says
        // nothing until the next frame; what is located changes far more slowly
        // than the world is drawn.
        //
        // And the rest unmade, which is what gives back a selection's worth of
        // meshes and materials when the selection goes. Held instead, a session
        // would carry every readout the largest selection it ever had wanted.
        while pool.0.len() < says.len() {
            pool.0.push(commands.spawn(readout(&mut materials, &face)).id());
        }
        for entity in pool.0.drain(says.len()..) {
            commands.entity(entity).despawn();
        }

        for (nth, entity) in pool.0.iter().enumerate() {
            let Ok((mut place, mut text, mut styling, mut visible, painted)) =
                written.get_mut(*entity)
            else {
                continue;
            };
            // Nothing to say, or a plane so far faded where it would have stood
            // that saying it would be saying it about nothing.
            let ink = inks.get(nth).copied().unwrap_or(0.);
            let Some((says, (eye, facing, viewport, cot_half_fov))) =
                says.get(nth).zip(seen).filter(|_| ink > 0.)
            else {
                visible.set_if_neq(Visibility::Hidden);
                continue;
            };
            visible.set_if_neq(Visibility::Inherited);

            let depth = into_view(facing, says.from_eye).max(MIN_DEPTH);
            let per_pixel = world_per_pixel(cot_half_fov, viewport.y, depth);
            // In the world, from the eye, which is where the readout is about.
            // The floating origin's own transform is its place in the frame
            // everything is drawn in, so this is a world position and not an
            // offset from one.
            place.translation =
                eye + says.from_eye.as_vec3() + says.hung * per_pixel;
            // Facing the camera, a row of text read edge on being no row at
            // all.
            place.rotation = facing;
            // The line box is exactly `SIZE` tall, so this is the height the row
            // draws at, in pixels, whatever the camera is doing.
            place.scale = Vec3::splat(READS * per_pixel / SIZE);

            // Both of these are read before they are written, so that a readout
            // saying what it said last frame does not have its mesh rebuilt for
            // it. Most frames say what the last one did.
            if lettered(&text) != Some(says.said.as_str()) {
                *text = Text3d::new(says.said.clone());
            }
            if styling.anchor.0 != says.anchor.0 {
                styling.anchor = says.anchor;
            }

            if let Some(mut painted) = materials.get_mut(&painted.0) {
                painted.base_color = says.hue.with_alpha(ink);
            }
        }
    }
}

/// What one readout is made of
///
/// A material apiece rather than a handful shared out. What a readout is drawn
/// at follows how far the plane has faded where it stands, so no two of them
/// are alike and a shared one would come out however the last to write it left
/// it. Unmaking a readout drops its handle, and with it the material.
///
/// Standing on its own rather than hung off the camera. A readout is placed in
/// the world every frame and has nothing to inherit, and a camera that went
/// away would otherwise take every readout with it and leave the pool holding
/// entities that are not there.
fn readout(
    materials: &mut Assets<StandardMaterial>,
    face: &Face,
) -> impl Bundle {
    (
        Readout,
        Text3d::new(String::new()),
        Text3dStyling {
            size: SIZE,
            font: face.family.into(),
            color: Srgba::WHITE,
            anchor: TextAnchor::CENTER,
            ..default()
        },
        Mesh3d::default(),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE.with_alpha(0.),
            // The glyphs are drawn white and unlit, so the base color
            // multiplies straight through them and is what a readout comes
            // out.
            base_color_texture: Some(TextAtlas::DEFAULT_IMAGE.clone()),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })),
        // Nothing is said until `readouts` has been round.
        Visibility::Hidden,
        Transform::default(),
    )
}

/// What a readout says, if it says anything this wrote
///
/// A readout is one run of text, [`readout`] having set it that way, and
/// anything else is not a readout this put up.
fn lettered(text: &Text3d) -> Option<&str> {
    match text.segments.as_slice() {
        [(Text3dSegment::String(said), _)] => Some(said),
        _ => None,
    }
}

/// Scratch each plane where a place on it is worth locating
///
/// A cross at the middle of the view and at the foot of every dropped line,
/// laid along the plane's own axes, and the dropped lines themselves. A number
/// written over a plane with nothing under it is a number floating loose.
///
/// Gizmos rather than meshes, every one of them moving every frame. Which puts
/// them in the same pass as the plane, so a line dropped to the ruling is drawn
/// exactly as the ruling's own lines are.
pub(super) fn marks(
    planes: Query<(&Plane, &Reading, &Dropped)>,
    eyes: Query<(&Transform, &Camera), With<FloatingOrigin>>,
    mut gizmos: Gizmos,
) {
    let Ok((at, camera)) = eyes.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    let facing = at.rotation;
    let eye = at.translation;

    for (plane, reading, dropped) in &planes {
        if reading.strength <= 0. {
            continue;
        }
        let hue = plane.color;
        // Where a place on the plane is in the world, how long an arm of its
        // cross comes to there, and what the two are drawn in.
        let scratched = |from_eye: DVec3, ink: f32| {
            let depth = into_view(facing, from_eye).max(MIN_DEPTH);
            (
                eye + from_eye.as_vec3(),
                CROSS * world_per_pixel(cot_half_fov, viewport.y, depth),
                hue.with_alpha(drawn_at(
                    ink * reading.strength,
                    reading.bright,
                )),
            )
        };

        if reading.middle {
            let middle = reading.middle_from_eye(plane.facing);
            let left = faded(middle, plane.reach);
            let (at, arm, color) = scratched(middle, INK * left);
            cross(&mut gizmos, at, plane.facing, arm, color);
        }

        for drop in &dropped.0 {
            let left = faded(drop.foot, plane.reach);
            let (foot, arm, color) = scratched(drop.foot, INK * left);
            cross(&mut gizmos, foot, plane.facing, arm, color);
            // The line at the ink the ruling's widest lines are drawn in, so
            // that it reads as one of the plane's rather than as something
            // laid over it.
            gizmos.line(
                eye + drop.top.as_vec3(),
                foot,
                hue.with_alpha(drawn_at(
                    MAJOR * reading.strength * left,
                    reading.bright,
                )),
            );
        }
    }
}

/// Two arms along the plane's own axes, crossing at `at`
///
/// Laid in the plane rather than across the screen, so a cross out towards the
/// horizon is foreshortened the way the cells around it are, and a cross on a
/// tilted plane lies in that plane.
fn cross(gizmos: &mut Gizmos, at: Vec3, facing: Quat, arm: f32, color: Color) {
    for axis in [Vec3::X, Vec3::Z] {
        let along = facing * axis * arm;
        gizmos.line(at - along, at + along, color);
    }
}

/// Give up the crossing the middle of the view is written over
///
/// The three numbers said at the middle are the same two the crossing beneath
/// them would be, and better: they carry the third, and they are not rounded
/// to a crossing. So where the two would land on each other the crossing gives
/// way.
///
/// The one it is written over, and only while it is. Each crossing owns a block
/// of the plane and the blocks tile it, so the middle stands in one of them and
/// that one gives way. Away from a row of lettering it stands in none and the
/// plane is left whole.
///
/// Nothing else is asked about. A plane that gave up a crossing for everything
/// a caller draws over the same sky would be a plane pocked with holes wherever
/// that sky is busy, which is where its numbers are most wanted, and what a
/// name needs is to stand out rather than for everything else to move.
pub(super) fn stand_clear(
    mut planes: Query<(&mut Plane, &Numbered, &Reading)>,
    eyes: Query<(&Transform, &Camera), With<FloatingOrigin>>,
) {
    let seen = eyes.single().ok().and_then(|(at, camera)| {
        let viewport = camera.logical_viewport_size()?;
        Some((at.rotation, viewport, camera.clip_from_view().y_axis.y))
    });

    for (mut plane, spoken, reading) in &mut planes {
        let mut bare = [NONE; BARE];

        if reading.middle
            && let Some(seen) = seen
            && let Some(room) = reaches(plane.as_ref(), reading, seen)
            && let Some(crossing) = plane.crossing_near(
                spoken,
                reading.middle_from_eye(plane.facing).as_vec3(),
                room,
            )
        {
            bare[0] = crossing;
        }

        if plane.numbers.bare != bare {
            plane.numbers.bare = bare;
        }
    }
}

/// [`CROWDS`] in the units `plane`'s lettering is laid out in
///
/// Measured at the middle of the view, by stepping a whole spacing along each
/// of the plane's own axes and seeing how far that carries on screen. Which
/// takes the pitch with it: the axis running away towards the horizon is
/// squashed to nothing as the camera comes down level with the plane, and a
/// row of pixels there covers a great many units of plane.
///
/// Nothing while the plane has no lettering to measure, or while the middle is
/// somewhere neither axis can be projected from.
fn reaches(
    plane: &Plane,
    reading: &Reading,
    (facing, viewport, cot_half_fov): (Quat, Vec2, f32),
) -> Option<Vec2> {
    let middle = reading.middle_from_eye(plane.facing);
    let at = on_screen(facing, cot_half_fov, viewport, middle)?;

    // One numbered spacing, in whatever the world is drawn in, which is the
    // length the axes are stepped by. Long enough that the two ends do not
    // land on the same float out at the rim, and short enough to stay inside
    // the view.
    let spacing = plane.numbers.apart as f64 * plane.cell;
    let unit = plane.numbers.tall / 5.;
    if !spacing.is_finite() || spacing <= 0. || unit <= 0. {
        return None;
    }
    let across = |axis: Vec2| -> Option<f32> {
        let along = plane.facing * Vec3::new(axis.x, 0., axis.y);
        let to = on_screen(
            facing,
            cot_half_fov,
            viewport,
            middle + along.as_dvec3() * spacing,
        )?;
        // Pixels to a spacing, and a spacing is `apart / unit` of them.
        let pixels = (to - at).length() * unit / plane.numbers.apart;
        (pixels > 0.).then_some(pixels)
    };

    Some(Vec2::new(
        CROWDS.x / across(plane.numbers.upright)?,
        CROWDS.y / across(plane.numbers.downward)?,
    ))
}

/// How far in front of a camera facing `facing` something `offset` from its eye
/// lies
///
/// Depth into the view, which is not the distance to the eye: a point at the
/// corner of the screen is further off than one at the middle at the same
/// depth, and sizing by distance draws the corner one larger.
fn into_view(facing: Quat, offset: DVec3) -> f32 {
    offset.dot((facing * Vec3::NEG_Z).as_dvec3()) as f32
}

/// How much world one logical pixel covers, at a given depth
///
/// A perspective view widens with depth, so a pixel spans more world the
/// further in it is measured. Multiplying a size in pixels by this gives the
/// world size that draws at it, which is what holds a readout at one size on
/// screen however far off the place it is about stands.
///
/// `cot_half_fov` is `Camera::clip_from_view().y_axis.y`, which glam fills with
/// `1 / tan(fov_y / 2)`. The vertical field of view is what the viewport's
/// height is divided into; aspect ratio lives in the matrix's x axis and does
/// not enter.
fn world_per_pixel(cot_half_fov: f32, viewport_height: f32, depth: f32) -> f32 {
    2. * depth / (cot_half_fov * viewport_height)
}

/// Where something `offset` from the eye lands on screen, in logical pixels
///
/// Nothing for anything level with the camera or behind it, which has no place
/// on screen to land on. The unit does not matter so long as it is one unit: a
/// place on screen is a length over a length, and the two cancel.
fn on_screen(
    facing: Quat,
    cot_half_fov: f32,
    viewport: Vec2,
    offset: DVec3,
) -> Option<Vec2> {
    let depth = into_view(facing, offset);
    if depth <= 0. {
        return None;
    }
    let right = offset.dot((facing * Vec3::X).as_dvec3()) as f32;
    let up = offset.dot((facing * Vec3::Y).as_dvec3()) as f32;
    let per_pixel = world_per_pixel(cot_half_fov, viewport.y, depth);

    Some(viewport / 2. + Vec2::new(right, -up) / per_pixel)
}

#[cfg(test)]
mod tests {
    use super::super::Painted;
    use super::*;

    const LIGHT_YEARS: Unit = Unit { metres: 9.4607304725808e15, mark: "Ly" };

    /// A plane and a reading of it, standing `back` off what it is looking at
    ///
    /// Neither square on nor edge on, so that there is a fade in it to tell one
    /// ink from another by.
    fn looking(back: f64) -> (Plane, Reading) {
        let square = 0.5_f64;
        let along = DVec3::new((1. - square * square).sqrt(), -square, 0.);
        let at = DVec3::new(3., 1., -2.);
        // Painted at the ink a caller writing `drawn_at(INK * strength, bright)`
        // arrives at, with both of those whole.
        let plane = Plane {
            reach: back * 6. * LIGHT_YEARS.metres,
            numbers: Painted { strength: INK, ..default() },
            ..Plane::default()
        };
        let reading = Reading {
            at,
            eye: at - along * back,
            step: 2.,
            unit: LIGHT_YEARS,
            strength: 1.,
            bright: 1.,
            middle: true,
        };
        (plane, reading)
    }

    /// What is drawn over the plane fades out towards the plane's horizon
    ///
    /// Whole where the camera stands, gone at the reach, and evenly between.
    #[test]
    fn what_is_written_fades_out_towards_the_horizon() {
        let reach = 60.;
        assert_eq!(faded(DVec3::ZERO, reach), 1.);
        assert_eq!(faded(DVec3::new(0., -reach, 0.), reach), 0.);
        assert_eq!(faded(DVec3::new(0., -reach / 2., 0.), reach), 0.5);
        // And nothing is left past it.
        assert_eq!(faded(DVec3::new(0., -reach * 2., 0.), reach), 0.);
    }

    /// And not by how the plane is pitched
    ///
    /// The lines go as the plane is turned edge on, a ruling at a grazing
    /// angle being moire rather than lines. Nothing standing over it has that
    /// trouble, and fading it with them would take the numbers away exactly
    /// when the view is too flat to read anything else off the plane.
    #[test]
    fn what_is_written_does_not_fade_by_pitch() {
        let reach = 60.;
        let far = 10.;
        // Straight down onto the plane, and along it as near as makes no
        // difference, at the one distance.
        let square = faded(DVec3::new(0., -far, 0.), reach);
        let grazing = faded(DVec3::new(far, 0., 0.), reach);
        assert_eq!(square, grazing);
        assert!(square > 0., "nothing was left to tell apart");
    }

    /// A number standing over the plane is the ink the plane paints its own in
    ///
    /// The same numbers about the same plane, drawn into the same pass, so an
    /// equal ink reaches the eye equally. What is left between them is where
    /// each stands: the plane's are painted all over it and fade wherever they
    /// lie, and one standing over it takes the fade at its own place.
    #[test]
    fn what_stands_over_the_plane_is_the_ink_painted_on_it() {
        let (plane, reading) = looking(100.);
        let painted = plane.numbers.strength;

        let says = spoken(&plane, &reading, &Dropped::default(), Vec3::X);
        let middle = says.first().expect("the middle is said");

        let stood = drawn_at(middle.ink * reading.strength, reading.bright);
        let left = faded(reading.middle_from_eye(plane.facing), plane.reach);
        assert!(left > 0. && left < 1., "nothing to tell apart at {left}");
        assert!(
            (stood - painted * left).abs() < 1e-6,
            "stood at {stood}, painted {painted} with {left} of the plane left"
        );
    }

    /// A thing off the plane is said at its foot and again on its line
    ///
    /// Three numbers under the mark where the line meets the plane, which is
    /// where the two rulers can be read against them, and how far off the
    /// plane it went beside the line itself. That last is the one thing about
    /// it neither ruler nor mark can show.
    #[test]
    fn a_dropped_line_is_said_at_both_ends() {
        let (plane, reading) = looking(100.);

        // A step off the middle both ways, so that neither number reads as
        // nought and the offset is worth saying out loud.
        let at = reading.at + DVec3::new(reading.step, reading.step, 0.);
        let top = reading.seen_from_eye(plane.facing, at);
        let under = DVec3::new(at.x, reading.at.y, at.z);
        let foot = reading.seen_from_eye(plane.facing, under);
        let dropped = Dropped(vec![Drop {
            top,
            foot,
            middle: (top + foot) / 2.,
            at,
        }]);

        let says = spoken(&plane, &reading, &dropped, Vec3::X);
        assert_eq!(says.len(), 3, "the middle, the foot and the offset");
        // The foot says all three, and it says them where the foot stands.
        assert_eq!(says[1].from_eye, foot);
        assert_eq!(says[1].said.matches(',').count(), 2);
        // The line says only how far off the plane it went, and says it
        // halfway up itself where there is a line to stand beside.
        assert_eq!(says[2].from_eye, (top + foot) / 2.);
        assert!(
            says[2].said.starts_with('+'),
            "a step above the plane came out {}",
            says[2].said
        );
        assert!(says[2].said.ends_with(reading.unit.mark));
    }

    /// And nothing is said about the middle when the middle is not asked for
    #[test]
    fn the_middle_goes_quiet_when_it_is_not_asked_for() {
        let (plane, mut reading) = looking(100.);
        assert_eq!(spoken(&plane, &reading, &Dropped::default(), Vec3::X).len(), 1);

        reading.middle = false;
        assert!(spoken(&plane, &reading, &Dropped::default(), Vec3::X).is_empty());
    }

    /// What is drawn over a plane can all be scheduled together
    ///
    /// A readout carries a `Transform`, and so does the camera it hangs off.
    /// Nothing but a filter says the two queries cannot land on the one entity,
    /// and bevy will not run a system whose parameters it cannot prove
    /// disjoint. It says so on the first frame rather than at compile time, so
    /// this asks at build time instead.
    #[test]
    fn what_is_drawn_over_a_plane_can_be_scheduled() {
        let face = Face { bytes: &[], family: "Hack" };
        let mut world = World::new();
        IntoSystem::into_system(locate).initialize(&mut world);
        IntoSystem::into_system(readouts(face)).initialize(&mut world);
        IntoSystem::into_system(marks).initialize(&mut world);
        IntoSystem::into_system(stand_clear).initialize(&mut world);
    }

    /// A number over a plane is drawn in the plane's own color
    ///
    /// The lines, the numbers painted along them and the numbers standing over
    /// them are one piece of chrome. Drawn in two colors they read as two.
    #[test]
    fn what_stands_over_a_plane_is_the_color_of_it() {
        let (mut plane, reading) = looking(100.);
        plane.color = Color::srgb(0.2, 0.4, 0.6);

        for one in spoken(&plane, &reading, &Dropped::default(), Vec3::X) {
            assert_eq!(one.hue, plane.color, "{} came out wrong", one.said);
        }
    }

    /// A tilted plane is read along its own axes, not the world's
    ///
    /// The rulers lie in the plane and count along it. Everything drawn about
    /// them stands in the world and is placed along its axes. A plane lying
    /// flat makes the two the same and hides every place the turn was left
    /// out, which is why this asks about one that does not.
    #[test]
    fn a_tilted_plane_is_read_along_its_own_axes() {
        let (mut plane, reading) = looking(100.);
        // On its side, so the plane's own `y` runs along the world's `x`.
        plane.facing = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

        // A step above the plane in its own terms is a step along its normal
        // in the world, and nothing at all up the world's own `y`.
        let over = reading.at + DVec3::Y * reading.step;
        let seen = reading.seen_from_eye(plane.facing, over)
            - reading.middle_from_eye(plane.facing);
        let step = reading.step * reading.unit.metres;
        assert!(
            (seen - DVec3::NEG_X * step).length() < step * 1e-6,
            "a step off the plane came out {seen}"
        );

        // And the row of numbers is hung under the plane the same way, which
        // is the other way along the same line.
        let says = spoken(&plane, &reading, &Dropped::default(), Vec3::X);
        let hung = says.first().expect("the middle is said").hung;
        assert!(
            (hung - Vec3::X * LIFT).length() < LIFT * 1e-6,
            "the row was hung {hung}"
        );
    }

    /// An app holding a plane, a camera and the systems that draw over it
    ///
    /// Everything `readouts` reads and nothing else. The face is empty: the
    /// pool is what is being asked about, and no glyph is cut or set here.
    fn drawing() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<StandardMaterial>>();
        app.init_resource::<Readouts>();

        let (plane, reading) = looking(100.);
        app.world_mut().spawn((plane, reading, Dropped::default()));
        // With a viewport of its own. A camera answers nothing for its size
        // otherwise, that being the render target's to say, and nothing here
        // brings up a render target.
        app.world_mut().spawn((
            Camera {
                viewport: Some(bevy::camera::Viewport {
                    physical_size: UVec2::new(800, 600),
                    ..default()
                }),
                ..default()
            },
            Transform::default(),
            FloatingOrigin,
            CellCoord::default(),
        ));

        app.add_systems(
            Update,
            readouts(Face { bytes: &[], family: "Hack" }),
        );
        app
    }

    /// How many readouts there are
    fn pooled(app: &App) -> usize {
        app.world().resource::<Readouts>().0.len()
    }

    /// What is located sets how many readouts there are, both ways
    ///
    /// Grown to what there is to say, and unmade when there is less. Held at
    /// the largest a session ever asked for, a map would carry every readout a
    /// selection wanted long after the selection went.
    #[test]
    fn the_readouts_follow_what_is_located() {
        let mut app = drawing();
        app.update();
        // The middle of the view, and nothing located.
        assert_eq!(pooled(&app), 1);

        // Three things off the plane: a mark at each foot and how far off each
        // went.
        let (_, reading) = looking(100.);
        let drop = || Drop {
            top: DVec3::ONE,
            foot: DVec3::X,
            middle: DVec3::X,
            at: reading.at + DVec3::ONE,
        };
        let mut planes = app.world_mut().query::<&mut Dropped>();
        for mut dropped in planes.iter_mut(app.world_mut()) {
            dropped.0 = (0..3).map(|_| drop()).collect();
        }
        app.update();
        assert_eq!(pooled(&app), 7);

        // And let go of again once they are.
        let mut planes = app.world_mut().query::<&mut Dropped>();
        for mut dropped in planes.iter_mut(app.world_mut()) {
            dropped.0.clear();
        }
        app.update();
        assert_eq!(pooled(&app), 1);
        // And gone from the world, not merely dropped from the list. The
        // material goes with the entity, one handle apiece.
        let mut standing = app.world_mut().query::<&Readout>();
        assert_eq!(standing.iter(app.world()).count(), 1);
    }
}
