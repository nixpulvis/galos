//! Where the galaxy lands on screen, worked out on the processor
//!
//! A mark built at a system's true coordinate goes through the clip transform
//! in `f32` at the scale of the camera's galaxy cell, where one part in 2²⁴ is
//! millions of kilometres, so it tears. Everything the map paints flat — the
//! star field, the names, the rings, the ruler's readouts — projects to a
//! pixel here in `f64` instead, and one set of this rather than one per
//! painter is the point: a ring and the star it rings are placed by the same
//! arithmetic.

use crate::map::camera::OrbitCamera;
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;
use bevy_egui::egui;

/// How far in front of the camera a point is, in metres
///
/// Depth into the view, which is not the same as the distance to the camera.
/// A point at the corner of the screen is further from the eye than one at
/// the center at the same depth, so sizing by distance draws the corner one
/// larger. At the corner of a 16:9 viewport with a quarter-turn field of
/// view, distance is about 1.31 times the depth.
///
/// In metres although `point` is given in light years, because every caller
/// wants it in order to work out a size to draw something at, and what is
/// drawn is measured in metres. [`screen_position`] takes the same projection
/// on its own rather than through this, since a place on screen is a ratio of
/// two lengths and does not care which unit either is in.
///
/// Both ends come from [`OrbitCamera`], which publishes a rotation and where
/// it stands during `Update`. The camera's `GlobalTransform` answers neither
/// question: it is written in `PostUpdate`, so it lags a frame, and it holds
/// a position relative to the floating origin rather than to the galaxy.
/// Negative behind the camera.
///
/// Asked of the camera as how far it stands from `point` rather than by
/// subtracting two galactic positions, so that a point the camera is
/// standing at — the star of the system it has descended into — is answered
/// exactly rather than to one rounding of a galactic light year. See
/// [`OrbitCamera::eye_from`].
pub(crate) fn depth(camera: &OrbitCamera, point: DVec3) -> f32 {
    depth_of(camera, crate::map::space::metres(-camera.eye_from(point)))
}

/// How far in front of the camera something `offset` from the eye is
///
/// The same measurement as [`depth`], for whatever already knows where it is
/// relative to the eye rather than where it is in the galaxy. Everything
/// inside a system does: [`big_space`] writes a `GlobalTransform` measured
/// from the floating origin, which is the camera, and it is exact near it.
///
/// Answers in whatever unit `offset` is given in.
pub(crate) fn depth_of(camera: &OrbitCamera, offset: DVec3) -> f32 {
    let forward = (camera.rotation * Vec3::NEG_Z).as_dvec3();
    offset.dot(forward) as f32
}

/// How much world one logical pixel covers, at a given depth
///
/// A perspective view widens with depth, so a pixel spans more world the
/// further in it is measured. Multiplying a size in pixels by this gives the
/// world size that draws at it, which is what makes a label hold its size on
/// screen however far away the system is.
///
/// What is handed in as `depth` is a choice, and everything that sizes a mark
/// has to make the same one. A place on screen is a ratio, so
/// [`screen_offset`] divides by the depth into the view; a size is not, and
/// the two callers that convert one measure along the line to the thing
/// instead — [`crate::map::paint::field::drawn_radius`], where the field builds its
/// quads, and [`crate::map::pointing::size_indicators`], which rings that mark and
/// catches the pointer over it. The two lengths differ by `1/cos θ` off the
/// middle of the frame: nothing at the centre, and about a third again at the
/// corner of a 16:9 viewport with a quarter-turn field of view (see
/// [`depth`]). So a mark converted from one and the ring around it converted
/// from the other are drawn apart by that factor towards the edges, which is
/// what happened — the indicator worked from the depth into the view while
/// the field worked from the distance to the system, and the ring sat off its
/// star.
///
/// `cot_half_fov` is `Camera::clip_from_view().y_axis.y`, which glam fills
/// with `1 / tan(fov_y / 2)`. The vertical field of view is what the
/// viewport's height is divided into; aspect ratio lives in the matrix's x
/// axis and does not enter.
pub(crate) fn world_per_pixel(
    cot_half_fov: f32,
    viewport_height: f32,
    depth: f32,
) -> f32 {
    2. * depth / (cot_half_fov * viewport_height)
}

/// The egui layer the map's own annotations are painted into
///
/// One background layer for the readouts, the rings, the names, their grounds
/// and the leaders alike: a single painter list, filled in the order the
/// systems writing into it run. There are four of them, and that run order is
/// the stacking — [`crate::map::grid::draw_readouts`] first and under everything,
/// being the ruling the map is read against rather than anything picked out
/// on it; then [`crate::map::pointing::ring`], and
/// [`crate::map::selection::ring`] over that, a selection being the
/// standing mark and a hover the passing one; then [`draw_names`] over the
/// top, so that no ring or readout row crosses the words. All four are pinned
/// against one another where they are registered, so none of the stacking is
/// left to how egui happens to order separate layers or to which painter the
/// executor happens to reach first.
///
/// A pair left unordered is not a matter of taste: two rings that overlap at
/// close zoom would stack one way this frame and the other way the next. So a
/// fifth painter added to this list needs a constraint against all four, not
/// only against the names it was written to sit under. `crate::ui`'s own
/// `lettering` and `chrome` follow the four, chained there.
///
/// `Background`, so the whole of it sits under the chrome and over the map.
/// Which takes the chrome being somewhere else: a layer that is not an area —
/// this is one painter list, not a window — is drained after every area of
/// its own order, so while the chrome shared `Background` a ring and a name
/// were painted over the settings pane. The chrome is `Order::Middle` and the
/// panels `Order::Foreground`; see `crate::ui::zone`.
pub(crate) fn annotations_layer() -> egui::LayerId {
    egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("map-annotations"),
    )
}

/// Where a point lands on screen, in logical pixels from the top left
///
/// [`None`] for anything level with the camera or behind it, which has no
/// place on screen to land on.
///
/// The camera's own axes turn the offset to a point into how far right, how
/// far up, and how far in it lies. The first two divided by what a pixel
/// covers at that depth are the offset from the middle of the viewport, in
/// pixels. Screen y counts downwards, where the camera's counts up.
pub(crate) fn screen_position(
    camera: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    point: DVec3,
) -> Option<Vec2> {
    screen_offset(camera, cot_half_fov, viewport, -camera.eye_from(point))
}

/// Where something `offset` from the eye lands on screen
///
/// What [`screen_position`] is written on, and what anything already holding
/// its own place relative to the camera asks directly.
///
/// The unit does not matter so long as it is one unit: a place on screen is a
/// length over a length, and the two cancel. So a system may ask in light
/// years and a body in metres, and both are answered in pixels.
pub(crate) fn screen_offset(
    camera: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    offset: DVec3,
) -> Option<Vec2> {
    let depth = offset.dot((camera.rotation * Vec3::NEG_Z).as_dvec3()) as f32;
    if depth <= 0. {
        return None;
    }

    let right = offset.dot((camera.rotation * Vec3::X).as_dvec3()) as f32;
    let up = offset.dot((camera.rotation * Vec3::Y).as_dvec3()) as f32;
    let per_pixel = world_per_pixel(cot_half_fov, viewport.y, depth);

    Some(viewport / 2. + Vec2::new(right, -up) / per_pixel)
}

/// The outline a ball draws on screen
///
/// An ellipse, and held as one: where its middle lands, how far it reaches
/// along and across, and which way the long axis lies. Everything the map
/// draws around a body is this shape — the ring, the area the pointer
/// catches it in, and the room a name is laid out clear of.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Silhouette {
    /// The middle of the outline, in pixels from the top left
    pub(crate) at: Vec2,
    /// How far it reaches: `x` along [`Silhouette::axis`], `y` across it
    pub(crate) across: Vec2,
    /// Which way the long axis lies, which is out from the middle of the
    /// view
    ///
    /// A ball dead ahead draws a circle and has no long axis; it is given
    /// `X`, either being as true as the other.
    pub(crate) axis: Vec2,
}

impl Silhouette {
    /// How many points a ring is painted with
    ///
    /// An ellipse is painted as a closed run of chords, and a chord cuts
    /// inside the curve by the square of its length — the same arithmetic the
    /// orbit lines are laid on. Sixty-four of them are under a tenth of a
    /// pixel out on a ring filling a tall screen, which is finer than the
    /// stroke it is drawn with.
    pub(crate) const POINTS: usize = 64;

    /// The same outline with `air` pixels of room around it
    pub(crate) fn grown(self, air: f32) -> Silhouette {
        Silhouette { across: self.across + Vec2::splat(air), ..self }
    }

    /// And held to `least` pixels at its narrowest
    pub(crate) fn at_least(self, least: f32) -> Silhouette {
        Silhouette { across: self.across.max(Vec2::splat(least)), ..self }
    }

    /// How far across it is at its widest
    ///
    /// What anything wanting one number for it asks: how much room a name
    /// has to be laid clear of, and how large the mark reads as. The long
    /// axis, so the answer clears the whole of it whichever way it lies.
    pub(crate) fn widest(&self) -> f32 {
        self.across.x.max(self.across.y)
    }

    /// Whether `point` falls inside it
    ///
    /// What the pointer is tested against, so that what can be clicked is
    /// exactly the shape that was drawn.
    pub(crate) fn holds(&self, point: Vec2) -> bool {
        let from = point - self.at;
        let across = self.across.max(Vec2::splat(f32::MIN_POSITIVE));
        let along = Vec2::new(
            from.dot(self.axis) / across.x,
            from.dot(self.axis.perp()) / across.y,
        );

        along.length_squared() <= 1.
    }

    /// The points it is painted as, once round
    pub(crate) fn path(&self) -> impl Iterator<Item = Vec2> + '_ {
        let across = self.axis.perp();
        (0..Self::POINTS).map(move |step| {
            let turn =
                step as f32 * std::f32::consts::TAU / Self::POINTS as f32;
            self.at
                + self.axis * (self.across.x * turn.cos())
                + across * (self.across.y * turn.sin())
        })
    }

    /// The ring egui paints for it
    ///
    /// Held here rather than at each painter so that the ring, the area the
    /// pointer is tested against ([`Silhouette::holds`]) and the room a name is
    /// laid clear of are one shape read three ways.
    pub(crate) fn painted(&self, stroke: egui::Stroke) -> egui::Shape {
        egui::Shape::closed_line(
            self.path().map(|at| egui::pos2(at.x, at.y)).collect(),
            stroke,
        )
    }
}

/// The outline a ball `radius` metres across, standing `offset` light years
/// from the eye, draws on screen
///
/// A body is drawn as a ball, and a ball is not a billboard. What the eye
/// sees of one is the cone of rays that graze it, and where that cone meets
/// the screen is an ellipse. Three things follow, and every one of them was
/// got wrong by treating the ball as a disc of its own radius:
///
/// - It is wider than that disc, by one over the cosine of its own grazing
///   angle: a star seen from three of its own radii is drawn a sixteenth
///   larger than its radius over the depth says.
/// - Off to one side of the view its middle is not where the ball's middle
///   projects, but further out. Sol from three radii, a third of the way off
///   the middle of the view, draws it fifty-five pixels further out — which
///   is a ring painted through the side of the star.
/// - And it is longer the way it leans than across it, by as much as a fifth
///   at the edge of the view. A circle drawn to the long way round stands
///   clear of the ball at the top and bottom by that much, which is a ring
///   that does not fit what it is around.
///
/// All three only show once a body fills a good part of the view, and that is
/// exactly where a body is looked at. None of them is a precision fault: they
/// happen as readily at Sol as at the rim.
///
/// The units are the two the map keeps — light years out to a thing, metres
/// across it — and the ratio of them is the whole of this, so they are
/// spoken into one here.
///
/// [`None`] for a ball level with the camera or behind it, for one the camera
/// is inside, and for one whose grazing cone runs parallel to the screen:
/// none of the three has an outline to draw.
pub(crate) fn outline(
    camera: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    offset: DVec3,
    radius: f32,
) -> Option<Silhouette> {
    let away = crate::map::space::metres(offset);
    let depth = away.dot((camera.rotation * Vec3::NEG_Z).as_dvec3());
    if depth <= 0. {
        return None;
    }
    let (distance, radius) = (away.length(), radius.max(0.) as f64);
    if distance <= radius {
        return None;
    }

    // How many pixels a unit at unit depth covers, which is the whole of the
    // lens: a point `aside` of its depth to one side lands `focal * aside`
    // pixels from the middle of the view.
    let focal = cot_half_fov as f64 * viewport.y as f64 / 2.;
    let right = away.dot((camera.rotation * Vec3::X).as_dvec3());
    let up = away.dot((camera.rotation * Vec3::Y).as_dvec3());
    // Where the ball's own middle projects, in pixels from the middle of the
    // view, which is what [`screen_offset`] answers and all this corrects.
    let projected = DVec2::new(right, -up) * focal / depth;

    // The tangent of the grazing cone's half angle, and of how far off the
    // view's own axis the ball lies. Tangents rather than angles: the whole
    // of what follows is the sum and difference of two angles taken through
    // theirs, and nothing here has to know an angle.
    let grazing = radius / (distance * distance - radius * radius).sqrt();
    let aside = projected.length() / focal;
    // How far the cone leans across the screen, and how much of its own
    // width that lean spends. The two ends of the long axis land at `focal`
    // times the tangent of `aside ± grazing`, and halving their sum and
    // their difference is where the rest of this comes from.
    let lean = 1. + aside * aside;
    let swell = 1. - aside * aside * grazing * grazing;
    if swell <= 0. {
        return None;
    }

    let middle = projected * (1. + grazing * grazing) / swell;
    let along = focal * grazing * lean / swell;
    // Across the lean the cone is cut square, so it spends the square root
    // of what the long way does.
    let across = focal * grazing * (lean / swell).sqrt();

    Some(Silhouette {
        at: viewport / 2. + middle.as_vec2(),
        across: Vec2::new(along as f32, across as f32),
        axis: projected.as_vec2().try_normalize().unwrap_or(Vec2::X),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera at the origin, looking the way `Quat::IDENTITY` faces
    fn camera(rotation: Quat) -> OrbitCamera {
        let mut camera = OrbitCamera::default();
        camera.rotation = rotation;
        camera
    }

    /// Two points at the same depth measure the same, however far apart
    ///
    /// This is the whole distinction. Both points below sit a hundred light
    /// years into the view, but the second is a third further from the eye,
    /// and measuring the distance would size it a third larger for it.
    #[test]
    fn depth_ignores_how_far_off_axis_a_point_is() {
        let camera = camera(Quat::IDENTITY);
        let ahead = DVec3::new(0., 0., -100.);
        let corner = DVec3::new(80., 45., -100.);
        // A hundred light years, in the metres depth answers in.
        let hundred = (100. * crate::map::space::LIGHT_YEAR) as f32;

        assert!((depth(&camera, ahead) - hundred).abs() < hundred * 1e-5);
        assert!((depth(&camera, corner) - hundred).abs() < hundred * 1e-5);
        assert!(
            corner.length() > 130.,
            "the corner point is only {} away, too close to tell the two apart",
            corner.length()
        );
    }

    /// A pixel covers the visible world at that depth, divided by the height
    #[test]
    fn a_pixel_covers_its_share_of_the_view() {
        let fov = std::f32::consts::FRAC_PI_4;
        let cot = 1. / (fov / 2.).tan();
        let depth = 100.;

        // What the viewport spans at that depth, from the field of view
        // alone, is what its pixels divide between them.
        let visible = 2. * depth * (fov / 2.).tan();
        let expected = visible / 1080.;

        assert!(
            (world_per_pixel(cot, 1080., depth) - expected).abs() < 1e-6,
            "a pixel covered {}, not {expected}",
            world_per_pixel(cot, 1080., depth)
        );
    }

    /// A label sized this way holds its apparent size at any depth
    ///
    /// This is what #58 asks for. What the eye sees is the world size over
    /// the depth, and the world size is proportional to depth, so the depth
    /// cancels and the same number of pixels is left at every range.
    #[test]
    fn a_label_holds_its_apparent_size_at_any_depth() {
        let (cot, height, pixels) = (2.414, 1080., 16.);
        let apparent = |d: f32| pixels * world_per_pixel(cot, height, d) / d;

        let near = apparent(1.);
        for depth in [10., 1_000., 100_000.] {
            assert!(
                (apparent(depth) - near).abs() < 1e-9,
                "{depth}ly away it subtended {}, against {near} at one",
                apparent(depth)
            );
        }
    }

    /// A point behind the camera measures negative
    #[test]
    fn depth_is_negative_behind_the_camera() {
        let camera = camera(Quat::IDENTITY);
        assert!(depth(&camera, DVec3::new(0., 0., 100.)) < 0.);
    }

    /// Whatever the camera orbits sits exactly its own radius deep
    ///
    /// `orbit_camera` places the eye at `center + rotation * Z * radius`, so
    /// this pins the helper to the convention the camera is written to. A
    /// forward of `+Z` would put the center behind the camera instead.
    #[test]
    fn depth_agrees_with_where_the_camera_puts_its_eye() {
        let rotation = Quat::from_euler(EulerRot::YXZ, 0.9, -0.4, 0.);
        let center = DVec3::new(1234.5, -678.9, 4321.);
        let radius = 250f32;
        let eye = center + (rotation * Vec3::Z * radius).as_dvec3();

        let mut camera = OrbitCamera::standing_at(eye);
        camera.rotation = rotation;
        // The radius is a distance the camera is set up in, which is light
        // years; the depth comes back in the metres it is drawn in.
        let expected = (radius as f64 * crate::map::space::LIGHT_YEAR) as f32;
        assert!(
            (depth(&camera, center) - expected).abs() < expected * 1e-5,
            "the center measured {} deep, not the {expected} the camera sits at",
            depth(&camera, center)
        );
    }

    /// The projection these helpers are written on is the one bevy renders
    /// through
    ///
    /// Everything painted flat over the map is placed by [`screen_offset`],
    /// and everything drawn in the scene is placed by the camera's own
    /// matrix. The two have to be one projection or every annotation sits
    /// off what it annotates — and the way that would read is a mark in the
    /// wrong place, which is the one symptom this module's own notes warn
    /// is hard to tell from a precision fault.
    ///
    /// So the helper is weighed against `Camera::world_to_viewport`, bevy's
    /// own answer for the same point: the camera at the origin looking down
    /// `-Z`, which is where [`big_space`] leaves the floating origin, and
    /// points off to every side of it.
    #[test]
    fn the_flat_projection_is_the_one_the_scene_is_drawn_with() {
        let (width, height) = (1280f32, 720f32);
        let lens =
            PerspectiveProjection { aspect_ratio: width / height, ..default() };
        let clip_from_view = Projection::Perspective(lens).get_clip_from_view();
        let rendered = Camera {
            computed: bevy::camera::ComputedCameraValues {
                target_info: Some(bevy::camera::RenderTargetInfo {
                    physical_size: UVec2::new(width as u32, height as u32),
                    scale_factor: 1.,
                }),
                clip_from_view,
                ..default()
            },
            ..default()
        };

        let rotation = Quat::from_euler(EulerRot::YXZ, 0.7, -0.3, 0.);
        let mut camera = OrbitCamera::default();
        camera.rotation = rotation;
        let eye = GlobalTransform::from(Transform::from_rotation(rotation));
        let viewport = Vec2::new(width, height);
        let cot_half_fov = clip_from_view.y_axis.y;

        for offset in [
            Vec3::new(0., 0., -1e9),
            Vec3::new(2e8, 0., -1e9),
            Vec3::new(-3e8, 1.5e8, -1e9),
            Vec3::new(1e8, -2.5e8, -5e8),
        ] {
            // What the renderer puts on screen, off the camera's own matrix
            // and its own transform.
            let drawn = rendered
                .world_to_viewport(&eye, rotation * offset)
                .expect("a point in front of the camera");
            // And what the flat painters put there, off the same offset in
            // the light years the map talks in.
            let painted = screen_offset(
                &camera,
                cot_half_fov,
                viewport,
                (rotation * offset).as_dvec3() / crate::map::space::LIGHT_YEAR,
            )
            .expect("the same point");

            assert!(
                drawn.distance(painted) < 1e-2,
                "the scene draws {offset} at {drawn} and the annotations \
                 paint it at {painted}",
            );
        }
    }

    /// Where the outline of a ball really lands, ray by ray
    ///
    /// Every ray that grazes a sphere touches it on one circle: the one lying
    /// in the plane square to the eye's line at `radius²/distance` short of
    /// the middle, `radius·√(1 - (radius/distance)²)` across. Projecting that
    /// circle point by point is the outline itself, which is what [`outline`]
    /// is meant to answer without the sampling.
    fn grazing_rays(
        camera: &OrbitCamera,
        cot_half_fov: f32,
        viewport: Vec2,
        offset: DVec3,
        radius: f64,
    ) -> Vec<Vec2> {
        let away = crate::map::space::metres(offset);
        let distance = away.length();
        let (toward, share) =
            (away / distance, 1. - radius * radius / (distance * distance));
        // The grazing circle: where it sits, and the two ways across it
        let middle = away * share;
        let across = radius * share.sqrt();
        let one = toward.cross(DVec3::Y).normalize();
        let other = toward.cross(one);

        (0..2048)
            .map(|step| {
                let turn = step as f64 * std::f64::consts::TAU / 2048.;
                let grazed =
                    middle + (one * turn.cos() + other * turn.sin()) * across;

                screen_offset(
                    camera,
                    cot_half_fov,
                    viewport,
                    grazed / crate::map::space::LIGHT_YEAR,
                )
                .expect("a grazing point in front of the camera")
            })
            .collect()
    }

    /// The outline of a ball is the ellipse its grazing rays land on
    ///
    /// The reported trouble, and the whole of it. A ball is not a billboard:
    /// its outline is wider than a disc of its own radius, its middle is not
    /// where the ball's middle projects, and it is longer the way it leans
    /// than across it. A circle drawn to any one of those three is a ring
    /// that does not fit the body — drawn through the side of it, or standing
    /// clear of it at the top and bottom.
    ///
    /// So the whole ring is weighed, and not one axis of it: every one of two
    /// thousand rays that graze the ball has to land on the outline's own
    /// boundary. That pins the middle, both half-widths and which way the
    /// long one lies, all at once, and nothing about the shape is left for a
    /// well-chosen sample to agree with by accident.
    #[test]
    fn a_balls_outline_is_the_ellipse_its_grazing_rays_land_on() {
        let viewport = Vec2::new(2004., 1143.);
        let cot = 2.4142137;
        let camera = camera(Quat::IDENTITY);
        let radius = 6.957e8f32;

        for (name, distance, aside) in [
            ("dead ahead, filling the view", 3., 0.),
            ("filling the view, well aside", 3., 0.29),
            ("filling the view, at the corner", 3., 0.6),
            ("a few radii off, aside", 8., 0.2),
            // Small enough that the three corrections all but vanish, and
            // still wide enough for an `f32` pixel to say where its edge is:
            // a speck a hundredth of a pixel across is measured to a percent
            // by the rounding alone.
            ("far enough to be small", 100., 0.3),
        ] {
            // Down `-Z` and off to one side, which is what the camera at
            // `Quat::IDENTITY` looks along.
            let depth = radius as f64 * distance;
            let offset = DVec3::new(depth * aside, 0., -depth)
                / crate::map::space::LIGHT_YEAR;

            let drawn = outline(&camera, cot, viewport, offset, radius)
                .expect("a ball in front of the camera has an outline");
            // How far out along the outline each grazing ray lands, where
            // the outline itself is one: what [`Silhouette::holds`] measures,
            // read as a number rather than as a yes.
            let along = |at: Vec2| {
                let from = at - drawn.at;
                Vec2::new(
                    from.dot(drawn.axis) / drawn.across.x,
                    from.dot(drawn.axis.perp()) / drawn.across.y,
                )
                .length()
            };

            let rays =
                grazing_rays(&camera, cot, viewport, offset, radius as f64);
            let (mut nearest, mut furthest) = (f32::MAX, 0f32);
            for ray in &rays {
                let out = along(*ray);
                nearest = nearest.min(out);
                furthest = furthest.max(out);
            }

            assert!(
                (nearest - 1.).abs() < 1e-3 && (furthest - 1.).abs() < 1e-3,
                "{name}: the grazing rays land between {nearest} and \
                 {furthest} of the way out along an outline that is meant to \
                 be exactly where they are",
            );
            // And the ring the pointer is tested against is that same shape.
            assert!(
                rays.iter().all(|ray| drawn.grown(1.).holds(*ray)),
                "{name}: a grazing ray fell outside the outline it is on",
            );
        }
    }

    /// And a ball off to one side draws its outline past its own middle
    ///
    /// Which is the whole of what was wrong: the ring went where the middle
    /// projects. Sol from three radii, a third of the way off the middle of
    /// the view, draws it an eighth further out.
    #[test]
    fn a_ball_aside_draws_its_outline_past_its_middle() {
        let viewport = Vec2::new(2004., 1143.);
        let cot = 2.4142137;
        let camera = camera(Quat::IDENTITY);
        let radius = 6.957e8;
        let depth = radius as f64 * 3.;
        let offset = DVec3::new(depth * 0.29, 0., -depth)
            / crate::map::space::LIGHT_YEAR;

        let flat = screen_offset(&camera, cot, viewport, offset)
            .expect("the ball's own middle");
        let at = outline(&camera, cot, viewport, offset, radius)
            .expect("an outline")
            .at;

        let (flat, seen) =
            ((flat - viewport / 2.).length(), (at - viewport / 2.).length());
        assert!(
            seen > flat * 1.1,
            "the outline stood {seen} px out where the middle projects to \
             {flat}, which is no correction at all",
        );
    }
}
