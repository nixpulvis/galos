//! A plane ruled into cells, drawn at any scale a [`big_space`] world reaches
//!
//! A fullscreen pass that meets the view ray with a plane per pixel and rules
//! it there. Antialiased by the screen space derivative, depth written so that
//! what stands in front of it occludes it, and one draw call however much of
//! the plane is on screen.
//!
//! # Why not [`bevy::dev_tools::infinite_grid`]
//!
//! That one rules by `fract` of the absolute position, in metres, in `f32`.
//! Two things follow. A position far from the origin has no fractional part
//! left, which is why a plane drawn that way has to be shuffled under the
//! camera every frame. And normalising the vector from the camera to it
//! squares it, which overflows a float past `1.8446744e19` — in a world drawn
//! in metres that is 1949 light years, past which the term is not a number and
//! the fragment is dropped, leaving a circle of ruling around the camera with
//! a hard edge on it.
//!
//! # What this does instead
//!
//! Counts in cells. [`Plane::eye`] is where the camera stands from the
//! ruling's origin, divided by the cell before it ever reaches the GPU, and a
//! ray is a direction, so the distance to the plane comes out in cells and so
//! does the point it lands on. A few dozen either way, whatever the world is
//! measured in and however wide a cell is.
//!
//! # How finely a plane may be ruled
//!
//! [`big_space`] holds a position as an `i64` cell and an `f32` remainder
//! within it, and that goes for where it thinks the floating origin is. Each
//! grid stores its own answer, re-split into that pair as it is handed from
//! grid to grid, so a grid knows where the floating origin is to about its own
//! cell edge over `2^24` — [`finest`].
//!
//! The floating origin's own grid is the exception: its answer is seeded rather
//! than handed on, and is exact. So a plane sharing a grid with the camera is
//! exact, and one further off in the tree is out by the coarsest cell edge
//! between them. Which is to say: rule from the grid the camera is standing
//! in, and hand over as the camera moves between them. Nesting costs nothing —
//! the crossing is done in `f64` by [`big_space`] itself before any of this
//! reads it.
pub(crate) mod cut;
pub(crate) mod ladder;
pub(crate) mod read;
pub(crate) mod said;

// What a caller has business with. The rest is how a ruling is laid out and
// how far apart its figures stand, which is the module's own affair: a caller
// asks for a plane rather than for the pixels between two numbers.
pub use cut::Face;
pub use ladder::{Decade, FIGURES_ACROSS, numbering, ruling, snapped_to};
pub use read::{EDGE_ON, Located, Reading, drawn_at, faded};
pub use said::{Unit, off_plane, power, ticked, told};

use bevy::asset::{AssetServer, Handle, embedded_asset, load_embedded_asset};
use bevy::camera::visibility::{self, NoFrustumCulling, VisibilityClass};
use bevy::color::ColorToComponents;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d};
use bevy::ecs::query::ROQueryItem;
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::math::{DMat3, DVec2, DVec3};
use bevy::prelude::*;
use bevy::image::Image;
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_asset::RenderAssets;
use bevy::render::texture::GpuImage;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass,
    ViewSortedRenderPhases,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    BlendState, ColorTargetState, ColorWrites, CompareFunction, DepthStencilState,
    DynamicUniformBuffer, FragmentState, MultisampleState, PipelineCache,
    PrimitiveState, RenderPipelineDescriptor, SamplerBindingType, ShaderStages,
    ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines,
    TextureFormat, TextureSampleType,
    binding_types::{sampler, texture_2d, uniform_buffer},
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::{RenderEntity, SyncToRenderWorld};
use bevy::render::view::{
    ExtractedView, RenderVisibleEntities, ViewUniform, ViewUniformOffset,
    ViewUniforms,
};
use bevy::render::{Extract, Render, RenderApp, RenderSystems};
use bevy::shader::Shader;
use bevy_rich_text3d::Text3dPlugin;
use big_space::prelude::*;

/// How many spacings a plane may be ruled at once
///
/// Enough for a decade crossfade with two to spare. Several at once rather
/// than one spacing and its tenths, because two spacings written onto the one
/// plane share an origin and an altitude by construction, and two planes
/// carrying one spacing each have to be placed into agreeing.
pub const FAMILIES: usize = 4;

/// What the lettering painted on a plane is drawn from, in the order an atlas
/// must hold it
///
/// Everything a coordinate is written with and nothing else. [`cut`] rasterises
/// these into one strip of equal cells from the face the caller handed over.
///
/// Equal cells because the layout counts characters rather than measuring
/// them, which is right for a monospaced face and wrong for any other.
pub const LETTERS: [char; 14] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '-', ',', '.', 'e',
];

/// How far to the right of its crossing a pair of numbers is written, and how
/// far above the line, in units of a fifth of a digit's height
///
/// The shader's own, said here as well because working out whether a pair
/// would be written over something wants both.
pub const BESIDE: f32 = 2.;
pub const ABOVE: f32 = 1.;

/// And the widest it can run, likewise
///
/// Twenty four characters at four units apiece: a sign, a figure, a point and
/// three places, then an `e` and a signed power, twice over with a comma
/// between. Longer than most pairs come out, and it is the longest that
/// matters — a caller spacing its crossings by this can be sure no pair ever
/// runs into the next.
pub const SPAN: f32 = 96.;

/// How many crossings may be left bare at once
///
/// The plane carries a few numbers on screen at a time, so this is a cap on
/// how many of them can be asked to give way rather than on how many names
/// are drawn: forty names crowded around one star fall in the one crossing
/// and ask for it once.
pub const BARE: usize = 16;

/// A crossing that is not to be numbered
///
/// Anything past the end of the list is this, which no real crossing is.
pub const NONE: IVec2 = IVec2::MAX;

/// How many crossings along each ruler carry a number that has been written out
///
/// The window [`Numbered`] holds, about the crossing at the middle of the view.
/// The ruling reaches a few times the distance the camera is standing back and
/// its crossings are spaced by a share of what is on screen, so how many fall
/// within reach is the ratio of those two and does not grow with the zoom.
/// Enough for that with room over; a crossing past the end goes unnumbered.
pub const NUMBERED: usize = 256;

/// And how many characters one of those numbers may take
///
/// Six to a packed word and three words to a number. Longer than a coordinate
/// written out in thousands comes to, which is a sign, a few figures, a point,
/// a few places and a letter.
pub const CHARS: usize = 18;

/// The strip of glyphs a plane's numbers are painted from
///
/// Cut by [`cut::cut_lettering`] at startup, from the face handed to
/// [`RuledPlugin`]. Nothing is painted until it is here.
#[derive(Resource, Clone)]
pub struct Lettering(pub Handle<Image>);

/// A number written out, as places into [`LETTERS`]
///
/// Written on the processor and painted on the card, which is what keeps the
/// two from having to agree about what a number looks like. The shader is handed
/// characters and lays them out; nothing in it knows about decades or thousands
/// or where a point goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Word {
    codes: [u8; CHARS],
    letters: u8,
}

impl Word {
    /// What `text` comes to, taking each character it has in [`LETTERS`]
    ///
    /// Anything else is passed over, and anything past [`CHARS`] is dropped.
    /// The alphabet is the whole of what a plane can paint, so a caller writing
    /// outside it is asking for a character that does not exist.
    pub fn say(text: &str) -> Word {
        let mut word = Word::default();
        for letter in text.chars() {
            let Some(code) = LETTERS.iter().position(|it| *it == letter) else {
                continue;
            };
            if word.letters as usize >= CHARS {
                break;
            }
            word.codes[word.letters as usize] = code as u8;
            word.letters += 1;
        }
        word
    }

    /// How many characters it takes
    pub fn letters(&self) -> i32 {
        self.letters as i32
    }

    /// Packed as the shader reads it
    ///
    /// Six characters to a word at five bits apiece, which is what seventeen
    /// letters and a blank take, and the count in the fourth. Six rather than a
    /// straddle across the join, so reading one is a shift and a mask.
    fn packed(&self) -> UVec4 {
        let mut packed = [0_u32; 3];
        for (place, code) in
            self.codes[..self.letters as usize].iter().enumerate()
        {
            packed[place / 6] |= u32::from(*code) << ((place % 6) * 5);
        }
        UVec4::new(packed[0], packed[1], packed[2], u32::from(self.letters))
    }
}

/// What the numbers along a plane's two rulers say
///
/// One [`Word`] to a numbered crossing, [`NUMBERED`] of them along each ruler
/// counting from [`Numbered::base`]. The pair painted at a crossing is the two
/// looked up with a comma between them.
///
/// Filled by whoever owns the plane, from [`said::ticked`] and the step
/// [`ladder::numbering`] chose. What the shader is handed is characters to lay
/// out; which power a crossing is said in and how many places it runs to are
/// worked out before they ever reach it.
#[derive(Component, Clone)]
pub struct Numbered {
    /// Which crossing the first word of each ruler is, counted from the space's
    /// own origin
    pub base: IVec2,
    /// What is written along the lettering, one word to a crossing
    pub along: [Word; NUMBERED],
    /// And what is written down it
    pub across: [Word; NUMBERED],
}

impl Default for Numbered {
    fn default() -> Self {
        Numbered {
            base: IVec2::ZERO,
            along: [Word::default(); NUMBERED],
            across: [Word::default(); NUMBERED],
        }
    }
}

impl Numbered {
    /// How far the pair at crossing `at` runs, in the lettering's own units
    ///
    /// Nothing for a crossing outside the window, or one either of whose
    /// numbers was left unwritten. Neither is painted, so neither takes up any
    /// room.
    pub fn written(&self, at: IVec2) -> Option<f32> {
        let along = self.word(&self.along, at.x - self.base.x)?.letters();
        let across = self.word(&self.across, at.y - self.base.y)?.letters();
        if along == 0 || across == 0 {
            return None;
        }
        // Four units to a character, less the air after the last of them, and
        // a comma between the two numbers and nothing else. A space after it
        // is a gap the eye reads as the end of one thing rather than the join
        // of two.
        Some((along + 1 + across) as f32 * 4. - 1.)
    }

    fn word<'a>(&self, ruler: &'a [Word; NUMBERED], into: i32) -> Option<&'a Word> {
        ruler.get(usize::try_from(into).ok()?)
    }
}

/// How finely a plane hanging in `grid` may be ruled, in whatever the world is
/// drawn in
///
/// [`big_space`] holds where the floating origin stands, in each grid, as that
/// grid's `i64` cell and an `f32` remainder inside it. A remainder is bounded
/// by half a cell, so it resolves the cell edge over `2^24`, and a plane ruled
/// finer than that is ruled from an origin that cannot be placed within a cell
/// of it — the whole ruling swims as the camera moves.
///
/// Going finer means hanging the plane in a grid whose cells are smaller,
/// which is what nesting is for.
pub fn finest(grid: &Grid) -> f64 {
    grid.cell_edge_length() as f64 * 2f64.powi(-24)
}

/// A plane ruled into cells
///
/// Placed by whatever grid it hangs from, the same as anything else. Where it
/// sits and which way it faces are its own [`Transform`] and [`CellCoord`];
/// what it is ruled in is [`Plane`].
#[derive(Component, Default, Reflect, Copy, Clone, Debug)]
#[reflect(Component, Default)]
#[require(
    read::Reading,
    read::Dropped,
    Plane,
    Numbered,
    Transform,
    CellCoord,
    Visibility,
    VisibilityClass,
    NoFrustumCulling,
    SyncToRenderWorld
)]
#[component(on_add = visibility::add_visibility_class::<Ruled>)]
pub struct Ruled;

/// How strongly a cell's lines and its tenth lines are drawn
///
/// Faint, both of them, and fainter than what a caller is likely to draw in the
/// sky rather than under it. A ruling crosses the whole of whatever it is laid
/// over and is meant to be glanced at rather than looked at, so anything it
/// competes with wins.
///
/// The numbers are not dropped with them. They are the part of the ruling that
/// is actually read, and they are already as small as a drawn face will go.
pub const MINOR: f32 = 0.05;
pub const MAJOR: f32 = 0.13;

/// And how strongly the numbers are
///
/// Above the lines, being the part of a ruling that is actually read. The one
/// figure for all of them: the numbers painted along a plane's own lines and
/// the numbers standing over it. Both are drawn into the same pass and so come
/// out at the same strength for the same ink.
pub const INK: f32 = 0.75;

/// One row of lines across a [`Plane`]
#[derive(Reflect, Copy, Clone, Debug, Default, PartialEq)]
pub struct Family {
    /// How far apart the lines are, as a multiple of [`Plane::cell`]
    ///
    /// Nothing for a family that is not drawn.
    pub apart: f32,
    /// How strongly they are drawn, against [`Plane::color`]'s own alpha
    pub strength: f32,
}

/// How a [`Ruled`] plane is ruled
#[derive(Component, Reflect, Copy, Clone, Debug)]
#[reflect(Component, Default)]
pub struct Plane {
    /// How wide a cell is, in whatever the world is drawn in
    ///
    /// Everything else here is a multiple of this. No finer than [`finest`] of
    /// the grid the plane hangs in.
    pub cell: f64,
    /// The rows of lines drawn across it
    pub families: [Family; FAMILIES],
    /// How far the ruling reaches before it has faded out, in whatever the
    /// world is drawn in
    pub reach: f64,
    /// How sharply the ruling goes as the plane is turned edge on
    ///
    /// The cosine below which it has gone entirely, so a small number keeps
    /// the plane until the camera is nearly level with it.
    pub edge_on: f32,
    /// What it is drawn in. The alpha stands over every family's own strength.
    pub color: Color,
    /// Where the camera stands from the ruling's origin, in cells, on the
    /// plane's own axes
    ///
    /// Written by [`place`] every frame and read by nothing else. Held on the
    /// component rather than worked out in the render world so that crossing
    /// the grid hierarchy, which is where the precision lives, happens once
    /// and in one place.
    pub eye: Vec3,
    /// Which way the plane's own axes lie in the world it is drawn in
    ///
    /// Written by [`place`] alongside [`Plane::eye`].
    pub facing: Quat,
    /// How the crossings of the ruling are numbered
    pub numbers: Painted,
}

/// Numbers painted onto a [`Plane`], at the crossings of its lines
///
/// Part of the ruling rather than laid over it: they lie in the plane, turn
/// with it, shrink with it and go the way it goes. What each says is a [`Word`]
/// out of [`Numbered`], written on the processor.
#[derive(Reflect, Copy, Clone, Debug, PartialEq)]
pub struct Painted {
    /// How far apart the numbered crossings are, in cells
    ///
    /// Nothing for a plane whose crossings are not numbered. The ruling's
    /// origin is laid on one of these, which is what makes
    /// [`Painted::from`] a whole number.
    pub apart: f32,
    /// How tall a digit is, in cells
    pub tall: f32,
    /// How strongly they are drawn, against [`Plane::color`]'s own alpha
    pub strength: f32,
    /// Which crossing the ruling's origin is, counted from the space's origin
    ///
    /// Written by [`place`], which is where the ruling's origin is settled.
    pub from: IVec2,
    /// Which way the lettering runs, on the plane's own axes
    ///
    /// One of the four quarter turns, whichever runs nearest to across the
    /// view. Written by [`place`].
    pub upright: Vec2,
    /// And which way its rows go, likewise
    ///
    /// The perpendicular of [`Painted::upright`] that runs down the view. Not
    /// derivable from it in the shader: which of the two perpendiculars points
    /// down turns over as the camera crosses the plane, and a plane seen from
    /// underneath would have its lettering upside down. Written by [`place`].
    pub downward: Vec2,
    /// Which crossings are to be left bare, [`NONE`] for the rest
    ///
    /// A plane knows nothing of what else is drawn over the same sky, and a
    /// number written under a name is a number nobody can read. So whoever
    /// draws the names says which crossings they have taken, and the ruling
    /// gives those up. [`Plane::crossing_near`] is how a caller works out
    /// which.
    pub bare: [IVec2; BARE],
}

impl Default for Painted {
    /// Nothing numbered, and every crossing left to be numbered
    ///
    /// Written out rather than derived because an empty list of crossings to
    /// give up is [`NONE`] throughout, and a derived one is nought throughout,
    /// which is a list asking for the crossing at the origin to be left bare.
    fn default() -> Self {
        Painted {
            apart: 0.,
            tall: 0.,
            strength: 0.,
            from: IVec2::ZERO,
            upright: Vec2::ZERO,
            downward: Vec2::ZERO,
            bare: [NONE; BARE],
        }
    }
}

impl Default for Plane {
    fn default() -> Self {
        Plane {
            cell: 1.,
            families: [
                Family { apart: 1., strength: 0.1 },
                Family { apart: 10., strength: 0.3 },
                Family::default(),
                Family::default(),
            ],
            reach: 1e3,
            edge_on: read::EDGE_ON,
            color: Color::WHITE,
            eye: Vec3::ZERO,
            facing: Quat::IDENTITY,
            numbers: Painted::default(),
        }
    }
}

impl Plane {
    /// The one crossing whose pair a thing standing `from_camera` is written
    /// over, counted from the space's own origin
    ///
    /// `room` is how far the thing reaches around itself, along the lettering
    /// and across it, in the units the lettering is laid out in: four to a
    /// character and five to a digit's height. A pair within that gives way;
    /// nothing else does.
    ///
    /// One at most, because a pair is not written where it is drawn. It stands
    /// up and to the right of its own crossing and runs on from there, so each
    /// crossing owns a block of the plane and the blocks tile it. A point falls
    /// in the writing of one block, or in the air after one pair and before the
    /// next, and the air belongs to whichever of the two it is nearer.
    ///
    /// Measured against what a pair actually runs to, from
    /// [`Numbered::written`], rather than against the block it is written in. A
    /// block is as wide as the crossings are spaced and a pair is usually a
    /// fraction of that, so bounding by the block holds a number given up for
    /// the whole width of its block to the right of it and gives it back at
    /// once to the left.
    pub fn crossing_near(
        &self,
        said: &Numbered,
        from_camera: Vec3,
        room: Vec2,
    ) -> Option<IVec2> {
        let unit = self.numbers.tall / 5.;
        if self.numbers.apart <= 0. || unit <= 0. {
            return None;
        }
        // Everything below is in the lettering's own units, which is what
        // `room`, the writing and the spacing are all said in.
        let step = self.numbers.apart / unit;

        let over = self.facing.inverse() * from_camera;
        let cells = self.eye + over / self.cell as f32;
        let flat = Vec2::new(cells.x, cells.z) / unit;
        let along = flat.dot(self.numbers.upright);
        let across = flat.dot(self.numbers.downward);

        // Across the lettering the rows stand a whole spacing apart, so the
        // nearest row is the only one within reach of anything.
        let row = (across / step).round();
        let line = across - row * step;
        let off = (-(ABOVE + 5.) - line).max(line + ABOVE).max(0.);
        if off > room.y {
            return None;
        }

        // And along it, which block the point is in and how far it is from the
        // writing: none while it is on it, and otherwise the shorter of the way
        // back to the end of this pair and the way on to the start of the next.
        let blocks = (along - BESIDE) / step;
        let mut which = blocks.floor();
        let column = (blocks - which) * step;
        let after =
            column - said.written(self.counted(which, row)).unwrap_or(0.);
        let before = step - column;
        let outside = if after <= 0. {
            0.
        } else if after <= before {
            after
        } else {
            which += 1.;
            before
        };
        if outside > room.x {
            return None;
        }

        Some(self.counted(which, row))
    }

    /// The crossing `along` blocks along the lettering and `across` rows down
    /// it, counted from the space's own origin
    fn counted(&self, along: f32, across: f32) -> IVec2 {
        let which =
            self.numbers.upright * along + self.numbers.downward * across;
        self.numbers.from
            + IVec2::new(which.x.round() as i32, which.y.round() as i32)
    }

    /// The widest spacing drawn, as a multiple of a cell
    ///
    /// What the ruling's origin is laid on. Snapped to anything finer, the
    /// wider families' lines would fall somewhere new every time the origin
    /// moved.
    fn widest(&self) -> f64 {
        self.families
            .iter()
            .filter(|it| it.apart > 0. && it.strength > 0.)
            .map(|it| it.apart as f64)
            .fold(1., f64::max)
    }
}

/// Work out where the camera stands from each plane, in cells
///
/// The one piece of arithmetic that has to be exact, and the reason this is a
/// module rather than a shader. See [`finest`] for what bounds it.
fn place(
    mut planes: Query<(Entity, &mut Plane, &CellCoord, &Transform, &GlobalTransform)>,
    eyes: Query<&Transform, With<FloatingOrigin>>,
    grids: Grids,
) {
    let Ok(camera) = eyes.single() else { return };
    // The floating origin's own transform is its offset from the cell it
    // stands in, and that cell is where everything drawn is measured from. So
    // this is where the camera is, in the space the shader works in.
    let eye = camera.translation.as_dvec3();
    let sideways = camera.rotation * Vec3::X;

    for (entity, mut plane, cell, transform, global) in &mut planes {
        // The grid a plane hangs from is the one that places it.
        let Some(grid) = grids.parent_grid(entity) else { continue };
        // A cell of nothing has no lattice to lay a ruling on and would put an
        // infinity through everything below.
        if !plane.cell.is_finite() || plane.cell <= 0. {
            continue;
        }
        let origin = grid.local_floating_origin();

        // Where the plane stands from the same cell, in `f64`. The cell
        // difference is exact in `i64` and the remainders are exactly the
        // positions the world holds, so nothing is lost that was ever there.
        // This is what `Grid::global_transform` works out and then spends on
        // an `f32`.
        let cells = *cell - origin.cell();
        let at = origin.grid_transform().transform_point3(
            grid.cell_to_float(&cells) + transform.translation.as_dvec3(),
        );

        // Onto the plane's own axes, where it lies flat in y.
        let facing = global.rotation();
        let square = DMat3::from_quat(facing.as_dquat().inverse());
        let over = square * (eye - at);

        // The ruling is laid on the lattice point nearest the camera, so that
        // what the shader is handed stays the size of the view. Nearest on a
        // multiple of the widest family, so that every line falls where it
        // would have fallen wherever the camera has since wandered — and of
        // the numbered crossings too, which is what makes which crossing the
        // origin is a whole number.
        let step =
            plane.cell * plane.widest().max(plane.numbers.apart as f64);
        let origin = DVec3::new(round_to(over.x, step), 0., round_to(over.z, step));

        plane.eye = ((over - origin) / plane.cell).as_vec3();
        plane.facing = facing;

        // Which crossing the ruling's origin is, counted from the space's own
        // origin. Whole, because the origin is laid on a multiple of the
        // widest family and the numbered crossings are a family of their own.
        let apart = plane.cell * plane.numbers.apart as f64;
        if apart > 0. {
            plane.numbers.from = IVec2::new(
                (origin.x / apart).round() as i32,
                (origin.z / apart).round() as i32,
            );
        }

        // And which way up the lettering goes. One of the four quarter turns,
        // whichever runs nearest to across the view from where the camera is
        // standing, so that a number is read left to right rather than
        // backwards. It snaps as the camera swings past a diagonal, which is
        // the price of lettering that is painted on rather than turned to
        // face.
        let across = square * sideways.as_dvec3();
        let right = if across.x.abs() >= across.z.abs() {
            DVec2::new(across.x.signum(), 0.)
        } else {
            DVec2::new(0., across.z.signum())
        };

        // Which of the two ways round that leaves is down the view. Along the
        // view the plane runs away towards its horizon, which is up the screen
        // from above the plane and down it from below, so this turns over as
        // the camera crosses. Left to the shader to work out from `right`
        // alone it cannot: a plane read from underneath would have every
        // number upside down.
        let ahead = square * (camera.rotation * Vec3::NEG_Z).as_dvec3();
        let sinking = DVec2::new(ahead.x, ahead.z) * -over.y.signum();
        let turned = DVec2::new(right.y, -right.x);

        plane.numbers.upright = right.as_vec2();
        plane.numbers.downward = if turned.dot(sinking) >= 0. {
            turned.as_vec2()
        } else {
            -turned.as_vec2()
        };
    }
}

/// Where something placed by `grid` stands, from the cell the floating origin
/// is in
///
/// The frame everything drawn is measured in. Exact: the cell difference is an
/// `i64` count and the remainders are the positions the world holds, so nothing
/// is lost that was ever there. What `Grid::global_transform` works out and
/// then spends on an `f32` on its last line.
///
/// Which is what lets a thing in one grid be located against a plane in
/// another: both are crossed into this frame in `f64` and subtracted there.
pub fn seen(grid: &Grid, cell: &CellCoord, transform: &Transform) -> DVec3 {
    let origin = grid.local_floating_origin();
    origin.grid_transform().transform_point3(
        grid.cell_to_float(&(*cell - origin.cell()))
            + transform.translation.as_dvec3(),
    )
}

/// Round `value` onto the nearest multiple of `step`
fn round_to(value: f64, step: f64) -> f64 {
    if step > 0. && step.is_finite() { (value / step).round() * step } else { value }
}

/// What the shader is handed, per plane
///
/// Field order is the WGSL struct's, which is what makes the two agree.
#[derive(Debug, ShaderType)]
struct PlaneUniform {

    eye: Vec3,
    reach: f32,
    rot: Mat3,
    cell: f32,
    edge_on: f32,
    families: [Vec4; FAMILIES],
    color: Vec4,
    numbers: Vec4,
    upright: Vec2,
    downward: Vec2,
    from: IVec2,
    base: IVec2,
    bare: [IVec4; BARE],
    along: [UVec4; NUMBERED],
    across: [UVec4; NUMBERED],
}

impl PlaneUniform {
    fn of(plane: &Plane, said: &Numbered) -> Self {
        PlaneUniform {
            eye: plane.eye,
            // In cells, the shader counting nothing else.
            reach: (plane.reach / plane.cell) as f32,
            rot: Mat3::from_quat(plane.facing.inverse()),
            cell: plane.cell as f32,
            edge_on: plane.edge_on.max(f32::MIN_POSITIVE),
            families: plane
                .families
                .map(|it| Vec4::new(it.apart, it.strength, 0., 0.)),
            color: plane.color.to_linear().to_vec4(),
            numbers: Vec4::new(
                plane.numbers.apart,
                plane.numbers.tall,
                plane.numbers.strength,
                0.,
            ),
            upright: plane.numbers.upright,
            downward: plane.numbers.downward,
            from: plane.numbers.from,
            base: said.base,
            // A `vec2` in an array is padded out to sixteen bytes anyway, so
            // it is said as a `vec4` rather than left to look like a mistake.
            bare: plane.numbers.bare.map(|it| IVec4::new(it.x, it.y, 0, 0)),
            along: said.along.map(|it| it.packed()),
            across: said.across.map(|it| it.packed()),
        }
    }
}

/// When a plane is told where it stands
///
/// A caller that has to answer [`Plane::crossing_at`] runs after this, which
/// is where the ruling's origin and the lettering's turn are settled.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Placing;

/// And when a caller says what its planes are ruled in
///
/// Everything drawn over a plane reads its [`Reading`], so whatever writes one
/// belongs in here. In `Update`, which is early enough for the text meshes to
/// be built and placed in the same frame.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ruling;

/// Everything it takes to draw a [`Ruled`] plane
///
/// A struct rather than the bare function the rest of this crate adds, because
/// the pipeline is built in [`Plugin::finish`] rather than [`Plugin::build`],
/// and a function has no `finish`.
pub struct RuledPlugin {
    /// The face a plane's numbers are painted in, and what it is called
    ///
    /// Monospaced. The strip painted onto a plane is cut from it at startup,
    /// see [`cut`], and the numbers standing over the plane are set in it.
    pub face: Face,
}

impl Plugin for RuledPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ruled.wgsl");
        app.register_type::<Ruled>().register_type::<Plane>();
        // The face reaches everything that sets a character by being handed
        // to it, so nothing here reads a font out of the world. What the world
        // does have to carry is the text stack itself, which is a plugin and a
        // list of faces to load rather than something that can be passed.
        //
        // Only if it is not already there. A caller that draws its own text in
        // the same face has added it, and a plugin added twice is a panic.
        if !app.is_plugin_added::<Text3dPlugin>() {
            app.add_plugins(Text3dPlugin {
                load_system_fonts: false,
                ..default()
            });
        }
        cut::wanted(app, self.face.bytes);
        app.init_resource::<read::Readouts>();
        app.add_systems(Startup, cut::cut_lettering(self.face.clone()));
        // The text standing over a plane is built into meshes in `PostUpdate`
        // before the transforms are propagated, so where it stands has to be
        // settled before then. A transform written after it is a readout a
        // frame behind the plane it stands on.
        app.add_systems(
            Update,
            (
                read::locate,
                read::readouts(self.face.clone()),
                read::marks,
            )
                .chain()
                .after(Ruling),
        );
        app.add_systems(PostUpdate, read::stand_clear.after(Placing));
        // After the transforms, which is where `big_space` settles where each
        // grid thinks the floating origin is. Read any earlier and a plane is
        // ruled from where the camera stood last frame.
        app.add_systems(
            PostUpdate,
            place.in_set(Placing).after(TransformSystems::Propagate),
        );
    }

    // The pipeline wants the render world's `FullscreenShader` and
    // `AssetServer`, and neither is there until the render plugin has finished.
    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<PlaneUniforms>()
            .init_resource::<RuledPipeline>()
            .init_resource::<SpecializedRenderPipelines<RuledPipeline>>()
            .add_render_command::<Transparent3d, DrawRuled>()
            .add_systems(ExtractSchedule, (extract, extract_lettering))
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Render,
                (bind_planes, bind_views).in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(Render, queue.in_set(RenderSystems::Queue));
    }
}

#[derive(Resource, Default)]
struct PlaneUniforms {
    uniforms: DynamicUniformBuffer<PlaneUniform>,
}

#[derive(Component)]
struct PlaneOffset(u32);

#[derive(Resource)]
struct PlaneBindGroup(BindGroup);

#[derive(Component)]
struct ViewBindGroup(BindGroup);

fn extract_lettering(
    mut commands: Commands,
    lettering: Extract<Option<Res<Lettering>>>,
) {
    if let Some(lettering) = lettering.as_deref() {
        commands.insert_resource(lettering.clone());
    }
}

fn extract(
    mut commands: Commands,
    planes: Extract<Query<(RenderEntity, &Plane, &Numbered)>>,
) {
    let extracted: Vec<_> = planes
        .iter()
        .map(|(entity, plane, said)| (entity, (*plane, said.clone())))
        .collect();
    commands.try_insert_batch(extracted);
}

fn prepare(
    mut commands: Commands,
    planes: Query<(Entity, &Plane, &Numbered)>,
    mut uniforms: ResMut<PlaneUniforms>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    uniforms.uniforms.clear();
    for (entity, plane, said) in &planes {
        let offset = uniforms.uniforms.push(&PlaneUniform::of(plane, said));
        commands.entity(entity).insert(PlaneOffset(offset));
    }
    uniforms.uniforms.write_buffer(&device, &queue);
}

fn bind_planes(
    mut commands: Commands,
    uniforms: Res<PlaneUniforms>,
    pipeline: Res<RuledPipeline>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    lettering: Option<Res<Lettering>>,
    images: Res<RenderAssets<GpuImage>>,
) {
    let Some(binding) = uniforms.uniforms.binding() else { return };
    // Nothing is drawn until the lettering is on the card. It is made once at
    // startup and never changes, so this is a frame or two and then never.
    let Some(letters) =
        lettering.and_then(|it| images.get(&it.0).cloned())
    else {
        commands.remove_resource::<PlaneBindGroup>();
        return;
    };
    commands.insert_resource(PlaneBindGroup(device.create_bind_group(
        "ruled_plane_bind_group",
        &cache.get_bind_group_layout(&pipeline.plane_layout),
        &BindGroupEntries::sequential((
            binding,
            &letters.texture_view,
            &letters.sampler,
        )),
    )));
}

fn bind_views(
    mut commands: Commands,
    views: Query<Entity, With<ViewUniformOffset>>,
    uniforms: Res<ViewUniforms>,
    pipeline: Res<RuledPipeline>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
) {
    let Some(binding) = uniforms.uniforms.binding() else { return };
    for entity in views.iter() {
        commands.entity(entity).insert(ViewBindGroup(device.create_bind_group(
            "ruled_view_bind_group",
            &cache.get_bind_group_layout(&pipeline.view_layout),
            &BindGroupEntries::single(binding.clone()),
        )));
    }
}

fn queue(
    cache: Res<PipelineCache>,
    functions: Res<DrawFunctions<Transparent3d>>,
    pipeline: Res<RuledPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<RuledPipeline>>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<(&ExtractedView, &RenderVisibleEntities, &Msaa), With<ExtractedCamera>>,
) {
    let Some(function) = functions.read().get_id::<DrawRuled>() else {
        return;
    };

    for (view, entities, msaa) in views.iter() {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let id = pipelines.specialize(
            &cache,
            &pipeline,
            RuledKey { target: view.target_format, samples: msaa.samples() },
        );
        let Some(visible) = entities.get::<Ruled>() else { continue };
        for (entity, main) in visible.iter_visible() {
            // Transient, not retained. A retained item outlives the frame that
            // queued it, so a plane that has since been hidden goes on being
            // drawn until something thinks to take it out again.
            phase.add_transient(Transparent3d {
                pipeline: id,
                entity: (*entity, *main),
                draw_function: function,
                distance: f32::NEG_INFINITY,
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                indexed: false,
                // The plane crosses the whole view, so there is no one depth to
                // sort it at. This sorts it first, which for a phase drawn
                // back to front puts it under everything else transparent —
                // chrome laid beneath the world rather than part of it. The
                // variant reads the other way round; what it does is hand the
                // sort `f32::NEG_INFINITY`, and `Transparent3d`'s distances
                // grow towards the camera.
                sorting_info: TransparentSortingInfo3d::AlwaysOnTop,
            });
        }
    }
}

type DrawRuled = (SetItemPipeline, DrawRuledPlane);

struct DrawRuledPlane;

impl<P: PhaseItem> RenderCommand<P> for DrawRuledPlane {
    type Param = SRes<PlaneBindGroup>;
    type ViewQuery = (Read<ViewUniformOffset>, Read<ViewBindGroup>);
    type ItemQuery = Read<PlaneOffset>;

    fn render<'w>(
        _item: &P,
        (view_offset, view_group): ROQueryItem<'w, '_, Self::ViewQuery>,
        offset: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        planes: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(PlaneOffset(offset)) = offset else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(0, &view_group.0, &[view_offset.offset]);
        pass.set_bind_group(1, &planes.into_inner().0, &[*offset]);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

#[derive(Resource)]
struct RuledPipeline {
    view_layout: BindGroupLayoutDescriptor,
    plane_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
}

impl FromWorld for RuledPipeline {
    fn from_world(world: &mut World) -> Self {
        RuledPipeline {
            view_layout: BindGroupLayoutDescriptor::new(
                "ruled_view_layout",
                &BindGroupLayoutEntries::single(
                    ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    uniform_buffer::<ViewUniform>(true),
                ),
            ),
            plane_layout: BindGroupLayoutDescriptor::new(
                "ruled_plane_layout",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        uniform_buffer::<PlaneUniform>(true),
                        texture_2d(TextureSampleType::Float { filterable: true }),
                        sampler(SamplerBindingType::Filtering),
                    ),
                ),
            ),
            shader: load_embedded_asset!(
                world.resource::<AssetServer>(),
                "ruled.wgsl"
            ),
            fullscreen: world.resource::<FullscreenShader>().clone(),
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct RuledKey {
    target: TextureFormat,
    samples: u32,
}

impl SpecializedRenderPipeline for RuledPipeline {
    type Key = RuledKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("ruled_plane_pipeline".into()),
            layout: vec![self.view_layout.clone(), self.plane_layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            primitive: PrimitiveState { cull_mode: None, ..default() },
            // Tested but not written: the plane is occluded by what stands in
            // front of it and occludes nothing itself.
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::Greater),
                stencil: default(),
                bias: default(),
            }),
            multisample: MultisampleState { count: key.samples, ..default() },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format: key.target,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use big_space::plugin::BigSpaceMinimalPlugins;

    /// The widest grid's cells, in whatever the world is drawn in
    ///
    /// `2^53`, which is what this map's galaxy is laid out in, and a power of
    /// two so that everything below is exact in both a float and a double and
    /// a test can say what it expects rather than approximately what.
    const ROOT_CELL: f64 = 9007199254740992.;

    /// How much finer each grid in the tower is than the one above it
    const NEST: f64 = 1048576.;

    /// How wide a ruled cell is, as a share of the grid it hangs in
    ///
    /// Well above [`finest`], which is `2^-24` of the same, and well below the
    /// cell, which is what a ruling is for.
    const RULED: f64 = 1. / 16384.;

    /// How far apart the widest family's lines are, in cells
    const WIDEST: f32 = 8.;

    /// How many cells out the camera stands
    ///
    /// About as far as this galaxy's rim in its own grid. Far enough that an
    /// `f32` holding the distance has lost tens of ruled cells, which is the
    /// whole point of the exercise.
    const OUT: i64 = 68_272;

    /// A tower of `deep` grids, each a fraction of the one above it
    ///
    /// Concentric — every grid sits at its parent's origin — so a position is
    /// the one number whichever grid it is spoken in, and what a test expects
    /// can be written down rather than derived.
    fn tower(deep: usize) -> (App, Vec<Entity>, f64) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(BigSpaceMinimalPlugins);
        app.add_systems(
            PostUpdate,
            place.in_set(Placing).after(TransformSystems::Propagate),
        );

        let mut grids = vec![
            app.world_mut()
                .spawn((
                    BigSpace::default(),
                    Grid::new(ROOT_CELL as f32, 0.1),
                    GlobalTransform::default(),
                ))
                .id(),
        ];
        let mut edge = ROOT_CELL;
        for level in 1..deep {
            edge /= NEST;
            let parent = grids[level - 1];
            grids.push(
                app.world_mut()
                    .spawn((
                        Grid::new(edge as f32, 0.1),
                        CellCoord::default(),
                        Transform::default(),
                        ChildOf(parent),
                    ))
                    .id(),
            );
        }
        (app, grids, edge)
    }

    /// How far the plane sits from its grid's own origin, in ruled cells
    ///
    /// A quarter of a cell out and half a cell up, because a plane sits where
    /// it is worth hanging rather than on a round number of its grid's cells,
    /// and a fraction that small is the first thing a float loses out at
    /// [`OUT`]. Quarters and halves so the arithmetic is exact and a test can
    /// say what it expects.
    const ASIDE: f64 = 0.25;
    const ABOVE: f64 = 0.5;

    /// A plane standing still near the deepest grid's origin
    ///
    /// Where a plane wants to be: it is the ruling that follows the camera,
    /// not the plane, and a plane shuffled under the camera every frame is
    /// what this module exists to stop needing.
    fn plane_at_rest(app: &mut App, grid: Entity, edge: f64) -> Entity {
        let cell = edge * RULED;
        app.world_mut()
            .spawn((
                Ruled,
                Plane {
                    cell,
                    families: [
                        Family { apart: 1., strength: 0.1 },
                        Family { apart: WIDEST, strength: 0.3 },
                        Family::default(),
                        Family::default(),
                    ],
                    ..default()
                },
                CellCoord::default(),
                Transform::from_translation(Vec3::new(
                    (cell * ASIDE) as f32,
                    (cell * ABOVE) as f32,
                    -(cell * ASIDE) as f32,
                )),
                ChildOf(grid),
            ))
            .id()
    }

    /// Stand the camera `cell` cells out and `from` past that
    fn eye_at(app: &mut App, grid: Entity, cell: CellCoord, from: Vec3) {
        app.world_mut().spawn((
            FloatingOrigin,
            cell,
            Transform::from_translation(from),
            GlobalTransform::default(),
            ChildOf(grid),
        ));
    }

    fn ruled(app: &App, plane: Entity) -> Plane {
        *app.world().entity(plane).get::<Plane>().expect("a plane")
    }

    /// A plane is ruled from where the world says the camera is, at any depth
    ///
    /// The camera stands `OUT` cells from a plane left at the origin, which is
    /// the arrangement the map actually has: out at the rim of a galaxy whose
    /// ruled plane hangs at its centre. A whole number of cells out, so the
    /// ruling's origin lands exactly under the camera and what crosses to the
    /// GPU is nothing at all.
    ///
    /// This is the assertion the whole module turns on. Read the distance off
    /// an `f32` `GlobalTransform` instead and it comes back tens of cells
    /// wrong, because a float that far out steps in units of eighty ruled
    /// cells.
    #[test]
    fn a_plane_is_ruled_from_where_the_camera_stands() {
        for deep in 1..=3 {
            let (mut app, grids, edge) = tower(deep);
            let cell = edge * RULED;
            let plane = plane_at_rest(&mut app, grids[deep - 1], edge);

            // Twenty cells up, which is the sort of height a plane is looked
            // at from, and a whole number of cells along.
            let up = (cell * 20.) as f32;
            eye_at(
                &mut app,
                grids[deep - 1],
                CellCoord::new(OUT, 0, -OUT / 2),
                Vec3::new(0., up, 0.),
            );
            app.update();

            let eye = ruled(&app, plane).eye;
            assert!(
                (eye.y as f64 - (20. - ABOVE)).abs() < 1e-3,
                "{deep} grids down, the camera stood {} cells over the plane \
                 and it was told {}",
                20. - ABOVE,
                eye.y
            );
            // A whole number of cells along from a plane a quarter cell
            // aside, so the ruling is laid a quarter cell from the camera.
            assert!(
                (eye.x as f64 + ASIDE).abs() < 1e-3
                    && (eye.z as f64 - ASIDE).abs() < 1e-3,
                "{deep} grids down, the ruling came out {eye} from the camera \
                 rather than a quarter cell either side"
            );
        }
    }

    /// And the ruling stays where it falls as the camera walks
    ///
    /// The ruling's origin is laid on the lattice point nearest the camera, so
    /// it moves every frame. The lines must not move with it, which holds so
    /// long as every place the origin is laid is a whole number of cells from
    /// every other. Walked in quarter cells, out where an `f32` could not tell
    /// one quarter from the next.
    #[test]
    fn the_lines_stay_where_they_fall() {
        let (_, _, edge) = tower(2);
        let cell = edge / NEST * 0. + edge * RULED;
        for (step, want) in [(0., -ASIDE), (0.25, 0.), (2.5, 2.25), (9.75, 1.5)] {
            let (mut app, grids, edge) = tower(2);
            let plane = plane_at_rest(&mut app, grids[1], edge);
            eye_at(
                &mut app,
                grids[1],
                CellCoord::new(OUT, 0, -OUT / 2),
                Vec3::new((cell * step) as f32, (cell * 20.) as f32, 0.),
            );
            app.update();

            // Laid on a multiple of the widest family, so a camera nine and
            // three quarter cells along is one and three quarters from the
            // line the ruling was laid on.
            let eye = ruled(&app, plane).eye;
            assert!(
                (eye.x as f64 - want).abs() < 1e-3,
                "walked {step} cells along, the ruling came out {} from the \
                 camera rather than {want}",
                eye.x
            );
        }
    }

    /// A plane whose lettering is a fifth of a cell to the unit, with its
    /// crossings a hundred units apart
    ///
    /// So the pair at the crossing `from` reads `0,0`, eleven units of writing
    /// running from two fifths of a cell along to two and three fifths, on a
    /// row lying between a fifth of a cell back and one and a fifth.
    fn lettered(from: IVec2) -> Plane {
        Plane {
            cell: 1.,
            numbers: Painted {
                apart: 20.,
                tall: 1.,
                upright: Vec2::X,
                downward: Vec2::Y,
                from,
                ..default()
            },
            ..default()
        }
    }

    /// And what its rulers say: every crossing counted out from `base`, which
    /// is what a plane ruled in whole numbers from its own origin comes to
    fn counting(base: IVec2) -> Numbered {
        let mut said = Numbered { base, ..default() };
        for into in 0..NUMBERED {
            said.along[into] = Word::say(&format!("{}", base.x + into as i32));
            said.across[into] = Word::say(&format!("{}", base.y + into as i32));
        }
        said
    }

    /// The crossing given up is the one the thing is written over
    ///
    /// One at most, and that one only while it is written on. A pair stands up
    /// and to the right of its own crossing and runs on from there, so what a
    /// point falls in is the writing rather than the crossing.
    #[test]
    fn the_crossing_a_thing_is_written_over_gives_way() {
        let plane = lettered(IVec2::ZERO);
        let said = counting(IVec2::ZERO);

        // Half way along the writing of the pair at the origin, with no room
        // asked for at all.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(1.5, 0., -0.5), Vec2::ZERO),
            Some(IVec2::ZERO)
        );
    }

    /// And nothing gives way where nothing is written
    #[test]
    fn a_thing_clear_of_the_lettering_takes_no_crossing() {
        let plane = lettered(IVec2::ZERO);
        let said = counting(IVec2::ZERO);

        // Between two rows of lettering, which is most of the plane.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(1.5, 0., 2.), CLOSE),
            None
        );
        // Along a row but past the end of the pair, with nothing reaching.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(3., 0., -0.5), Vec2::ZERO),
            None
        );
        // And in the air between two pairs, out of reach of either.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(12.4, 0., -0.5), CLOSE),
            None
        );
    }

    /// A pair is given up as far after it as before it
    ///
    /// What bounds it is the writing, which runs eleven units here, rather than
    /// the block, which is the hundred the crossings are spaced by. Bounded by
    /// the block a pair is given up for the whole width of it to the right and
    /// handed back at once to the left, which reads as a number that goes out
    /// one way and not the other.
    #[test]
    fn a_pair_is_given_up_as_far_after_it_as_before_it() {
        let plane = lettered(IVec2::ZERO);
        let said = counting(IVec2::ZERO);
        let reach = Vec2::new(6., 5.);

        // The first pair ends at two and three fifths of a cell along, so six
        // units past it is three and four fifths and seven units is four.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(3.8, 0., -0.5), reach),
            Some(IVec2::ZERO)
        );
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(4., 0., -0.5), reach),
            None
        );

        // The second starts at twenty and two fifths, so six units before it is
        // nineteen and a fifth and seven is nineteen.
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(19.2, 0., -0.5), reach),
            Some(IVec2::X)
        );
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(19., 0., -0.5), reach),
            None
        );
    }

    /// And how far it reaches follows what it says
    #[test]
    fn a_longer_pair_reaches_further() {
        // `12,34` rather than `0,0`, which is two more characters of writing.
        let far = lettered(IVec2::new(12, 34));
        assert_eq!(
            far.crossing_near(
                &counting(IVec2::new(12, 34)),
                Vec3::new(3.4, 0., -0.5),
                Vec2::ZERO
            ),
            Some(IVec2::new(12, 34))
        );

        let near = lettered(IVec2::ZERO);
        assert_eq!(
            near.crossing_near(
                &counting(IVec2::ZERO),
                Vec3::new(3.4, 0., -0.5),
                Vec2::ZERO
            ),
            None
        );
    }

    /// A crossing nothing was written for takes up no room
    ///
    /// The window reaches past what the ruling does, so this is the horizon
    /// rather than anywhere a number is read. Nothing is painted there and so
    /// nothing there can be crowded.
    #[test]
    fn a_crossing_outside_the_window_is_written_nothing() {
        let plane = lettered(IVec2::ZERO);
        let said = Numbered { base: IVec2::splat(1000), ..default() };

        assert_eq!(said.written(IVec2::ZERO), None);
        assert_eq!(
            plane.crossing_near(&said, Vec3::new(1.5, 0., -0.5), Vec2::ZERO),
            None
        );
    }

    /// And it is counted from the space's own origin
    #[test]
    fn the_crossing_given_up_is_counted_from_the_space() {
        let plane = lettered(IVec2::new(5, -3));
        let said = counting(IVec2::new(5, -3));

        assert_eq!(
            plane.crossing_near(&said, Vec3::new(1.5, 0., -0.5), Vec2::ZERO),
            Some(IVec2::new(5, -3))
        );
    }

    /// A number is said in the alphabet the plane paints from
    #[test]
    fn a_word_is_the_places_of_its_letters() {
        let word = Word::say("-1.5e3");
        assert_eq!(word.letters(), 6);
        // Minus, one, point, five, e, three, in `LETTERS` order.
        assert_eq!(word.codes[..6], [10, 1, 12, 5, 13, 3]);
        // And nothing outside the alphabet is taken.
        assert_eq!(Word::say("1 2").letters(), 2);
        assert_eq!(Word::say("").letters(), 0);
    }

    /// And packed six characters to a word, with the count in the fourth
    #[test]
    fn a_word_packs_six_letters_to_a_word() {
        let packed = Word::say("1234567").packed();
        assert_eq!(packed.w, 7);
        // The first six, five bits apiece, lowest first.
        assert_eq!(packed.x & 31, 1);
        assert_eq!((packed.x >> 25) & 31, 6);
        // And the seventh at the bottom of the next.
        assert_eq!(packed.y & 31, 7);
        assert_eq!(packed.z, 0);
    }

    /// About what a thing standing at the middle of a view reaches, in the
    /// lettering's own units
    const CLOSE: Vec2 = Vec2::new(20., 5.);

    /// How finely a grid may be ruled follows its own cells
    ///
    /// The rule the whole design rests on: a grid knows where the floating
    /// origin stands to about its cell edge over `2^24`, that being an `f32`
    /// remainder bounded by half a cell. Going finer means a grid whose cells
    /// are smaller, which is what nesting is for.
    #[test]
    fn a_finer_grid_may_be_ruled_finer() {
        let galaxy = Grid::new(ROOT_CELL as f32, 0.1);
        let system = Grid::new(1., 0.1);

        assert_eq!(finest(&galaxy), 536870912.);
        assert!(finest(&system) < finest(&galaxy) / 1e8);
        // And the tower's ruling is inside what its grid can place.
        assert!(RULED > 2f64.powi(-24));
    }
}
