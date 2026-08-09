// A plane ruled into cells, drawn by meeting the view ray with it per pixel.
//
// Everything here is counted in cells rather than in whatever the world is
// drawn in. `plane.eye` arrives already divided by the cell, and a ray is a
// direction, which has no unit at all, so the distance to the plane comes out
// in cells and so does the point it lands on. A world measured in metres and
// ruled in light seconds never puts a metre through this shader.
//
// That is the whole of it. A ruling worked out from absolute positions loses
// its fractional part far from the origin, and squaring one overflows a float
// past 1.8e19, which is only a couple of thousand light years of metres.
//
// The one number that has to be spoken in the world's own unit is the depth,
// and it is spoken as a division rather than a length, a length being the
// thing that overflows.

#import bevy_render::view::View
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

/// How many spacings a plane may be ruled at once
const FAMILIES: u32 = 4u;

struct Plane {
    /// Where the camera stands from the ruling's origin, in cells, on the
    /// plane's own axes
    ///
    /// Small, the origin being laid on the lattice point nearest the camera.
    /// This is the number the whole shader is built to keep small.
    eye: vec3<f32>,
    /// How far the ruling reaches before it has faded out, in cells
    reach: f32,
    /// From the world the plane is drawn in onto the plane's own axes, so the
    /// lines run along them however the plane is turned
    rot: mat3x3<f32>,
    /// How wide a cell is, in whatever the world is drawn in
    cell: f32,
    /// How sharply the ruling goes as the plane is turned edge on
    edge_on: f32,
    /// Each family's spacing, as a multiple of a cell, and how strongly it is
    /// drawn. A spacing of nothing is a family that is not there.
    families: array<vec4<f32>, FAMILIES>,
    color: vec4<f32>,
    /// How the crossings are numbered: how far apart the numbered ones are in
    /// cells, how tall a digit is in cells, and how strongly they are drawn.
    numbers: vec4<f32>,
    /// Which way the lettering runs, on the plane's own axes
    ///
    /// One of the four quarter turns, whichever runs nearest to across the
    /// view.
    upright: vec2<f32>,
    /// And which way its rows go: the perpendicular of `upright` that runs
    /// down the view, which turns over as the camera crosses the plane.
    downward: vec2<f32>,
    /// Which crossing the ruling's origin is, counted from the space's own
    /// origin in numbered crossings
    ///
    /// A whole number, and an `i32` rather than a float because it runs to
    /// hundreds of millions at the fine end of the zoom out at the rim, where
    /// a float has long since stopped counting by ones.
    ///
    /// Named the long way round because `from` is a word WGSL keeps.
    counted: vec2<i32>,
    /// And which crossing the first word of each ruler below is
    base: vec2<i32>,
    /// Which crossings are not to be numbered, because something else is
    /// already written over them. Anything past the end is `BARE_NONE`.
    bare: array<vec4<i32>, BARE>,
    /// What each numbered crossing says, along the lettering and down it
    ///
    /// Written out on the processor and read here as characters. Which decade a
    /// crossing is worth, which thousand it is called, where its point goes and
    /// how many places follow it are all questions about what a number means,
    /// and they are answered once, there, rather than a second time here.
    along: array<vec4<u32>, NUMBERED>,
    across: array<vec4<u32>, NUMBERED>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<uniform> plane: Plane;
/// One row of equal cells, one glyph to a cell, in `LETTERS` order
@group(1) @binding(1) var lettering: texture_2d<f32>;
@group(1) @binding(2) var reader: sampler;

/// How many crossings may be left bare at once
const BARE: u32 = 16u;

/// What stands in an unused place in that list
const BARE_NONE: i32 = 2147483647;

/// How many glyphs the lettering strip holds
///
/// Ten digits, a minus, a comma, a point and an `e`, in that order. The same
/// order the caller rasterised them in.
const LETTERS: f32 = 14.0;

/// How many crossings along each ruler carry a written number
const NUMBERED: u32 = 256u;

/// How far to the right of its crossing a pair is written, in font units
const BESIDE: f32 = 2.0;

/// And how far above the line it stands, likewise
///
/// Above rather than across it. Written on the line the line runs through the
/// letters, and a number with a rule through it is a number to be worked out
/// rather than read.
const ABOVE: f32 = 1.0;

const COMMA: u32 = 11u;
const BLANK: u32 = 17u;

/// How many characters a written number takes
fn letters(word: vec4<u32>) -> i32 {
    return i32(word.w);
}

/// The `slot`th character of one, from the left
///
/// Six to a packed word at five bits apiece, which is what fourteen letters and
/// a blank take. Six rather than a straddle across the join, so this is a
/// shift and a mask.
fn letter(word: vec4<u32>, slot: i32) -> u32 {
    var packed = word.x;
    if slot >= 12 {
        packed = word.z;
    } else if slot >= 6 {
        packed = word.y;
    }
    return (packed >> (u32(slot % 6) * 5u)) & 31u;
}

/// How much of this fragment the lettering at the nearest numbered crossing
/// covers
///
/// The numbers are part of the ruling rather than laid over it, so they lie in
/// the plane, turn with it, shrink with it and go the way it goes. Nothing has
/// to be kept in step with anything, because there is only the one thing.
fn painted(at: vec2<f32>) -> f32 {
    let apart = plane.numbers.x;
    let tall = plane.numbers.y;
    if apart <= 0.0 || tall <= 0.0 {
        return 0.0;
    }

    let right = plane.upright;
    let down = plane.downward;
    let unit = tall / 5.0;
    let u = dot(at, right);
    let v = dot(at, down);

    // Which crossing's lettering this belongs to. Along the lettering, the
    // crossing behind rather than the nearest: a pair is written to the right
    // of its own crossing, so its far end stands nearer the next one, and
    // asking for the nearest cuts every label off in the middle.
    let cu = floor((u - BESIDE * unit) / apart);
    let cv = round(v / apart);
    let which = right * cu + down * cv;
    let local = vec2(u - cu * apart, v - cv * apart) / unit;

    // Gone once a row of the font is thinner than a pixel, which is what turns
    // lettering into grey. The same screen space derivative that antialiases
    // the lines, and taken of the run rather than of the offset, which steps
    // at every crossing.
    let pixel = fwidth(u) / unit;
    let legible = clamp((0.85 - pixel) / 0.5, 0.0, 1.0);
    if legible <= 0.0 {
        return 0.0;
    }

    // Which crossing this is, counted from the space's own origin.
    let nx = plane.counted.x + i32(round(which.x));
    let nz = plane.counted.y + i32(round(which.y));

    // Unless something else is already written there. A number under a name
    // is a number nobody can read, and the name is the one that was asked for.
    for (var i = 0u; i < BARE; i++) {
        if plane.bare[i].x == BARE_NONE {
            break;
        }
        if plane.bare[i].x == nx && plane.bare[i].y == nz {
            return 0.0;
        }
    }

    // And what it says. Outside the window nothing was written for it, which
    // is a crossing out past where a number is still worth reading.
    let ix = nx - plane.base.x;
    let iz = nz - plane.base.y;
    if ix < 0 || ix >= i32(NUMBERED) || iz < 0 || iz >= i32(NUMBERED) {
        return 0.0;
    }
    let said_x = plane.along[ix];
    let said_z = plane.across[iz];
    let wx = letters(said_x);
    let wz = letters(said_z);
    if wx == 0 || wz == 0 {
        return 0.0;
    }
    // A comma between them and nothing else. A space after it is a gap the
    // eye reads as the end of one thing rather than the join of two.
    let total = wx + 1 + wz;

    // Four across to a character, three of them inked and one of air. The
    // pair stands beside the crossing it marks rather than over it, up and to
    // the right, so the crossing is left clear, the line is not drawn through
    // the letters, and the two numbers read out from it.
    let across = f32(total) * 4.0 - 1.0;
    let column = local.x - BESIDE;
    let line = local.y + ABOVE;
    if column < 0.0 || column >= across || line < -5.0 || line > 0.0 {
        return 0.0;
    }
    let slot = i32(floor(column * 0.25));
    let inset = column - f32(slot) * 4.0;
    if inset >= 3.0 {
        return 0.0;
    }

    var code = BLANK;
    if slot < wx {
        code = letter(said_x, slot);
    } else if slot == wx {
        code = COMMA;
    } else {
        code = letter(said_z, slot - wx - 1);
    }
    if code == BLANK {
        return 0.0;
    }

    // Read the glyph out of its cell. Sampled rather than tested bit by bit,
    // so the shape is a real one and the edge is the card's own filtering
    // rather than a staircase.
    let across_cell = (f32(code) + inset / 3.0) / LETTERS;
    let down_cell = (line + 5.0) / 5.0;
    let coverage = textureSampleLevel(
        lettering,
        reader,
        vec2(across_cell, down_cell),
        0.0
    ).r;

    // Taken as it is read. The card's own filtering is what antialiases the
    // lettering, and at the size this is drawn most of a stroke is edge, so
    // sharpening about the halfway mark throws that away and leaves the
    // numbers a grey suggestion.
    return coverage * legible;
}

fn ndc_to_world(ndc: vec3<f32>) -> vec3<f32> {
    let world = view.world_from_clip * vec4(ndc, 1.0);
    return world.xyz / world.w;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> FragmentOutput {
    // Which way this pixel looks. Both points are taken off the near plane's
    // own scale rather than the world's, so neither is ever large, and what
    // comes out of them is a direction, which carries no unit into the rest.
    let clip = in.uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    let near = ndc_to_world(vec3(clip, 1.0));
    let seen = normalize(ndc_to_world(vec3(clip, 0.001)) - near);
    // Onto the plane's axes, where the ruling is square and the plane is flat
    // in y, so meeting it is a division rather than a dot product.
    let ray = plane.rot * seen;

    let t = -plane.eye.y / ray.y;
    // Behind the camera, or so near edge on that the meeting point ran out of
    // float. Neither is a place the plane is drawn.
    if !(t > 0.0 && t < 3.0e38) {
        discard;
    }
    let at = (plane.eye + ray * t).xz;

    // The lines, widest family first. Each is drawn into what the wider ones
    // have left, so a line two families both fall on is drawn once, at the
    // stronger of the two, rather than twice into the one pixel.
    var lines = 0.0;
    for (var i = 0u; i < FAMILIES; i++) {
        let apart = plane.families[i].x;
        let strength = plane.families[i].y;
        if apart <= 0.0 || strength <= 0.0 {
            continue;
        }
        let cells = at / apart;
        // How far this pixel stands from a line, over how far a pixel reaches.
        // The screen space derivative is what antialiases the ruling, and what
        // loses it as the cells close up towards the horizon rather than
        // letting them turn to moire.
        let off = abs(fract(cells - 0.5) - 0.5) / fwidth(cells);
        lines += (1.0 - min(min(off.x, off.y), 1.0)) * strength * (1.0 - lines);
    }
    // Out at the horizon the ruling is edge on whatever the camera is doing,
    // and a plane the camera has come level with is a line across the sky.
    // Both are faded rather than drawn.
    let square = abs(ray.y);
    let fade =
        mix(clamp(1.0 - t / plane.reach, 0.0, 1.0), 1.0, square)
        * min(square / plane.edge_on, 1.0);

    // The numbers over the lines, being the part of the ruling that is read.
    // Faded with them and by the same amount: the fade is what is left of the
    // plane here, and everything drawn on it is drawn into what is left. What
    // sets a number apart from a line is the ink it starts in, `numbers.z`
    // against a family's own strength, and nothing else.
    let ink = lines + painted(at) * plane.numbers.z * (1.0 - lines);
    if ink <= 0.0 {
        discard;
    }

    // How deep the meeting point is, for the depth buffer, which is the one
    // place the world's own unit is wanted. `clip_from_view[3][2]` is the near
    // plane of the reversed infinite projection the map is drawn with.
    var out: FragmentOutput;
    let deep = t * plane.cell * dot(seen, -view.world_from_view[2].xyz);
    out.depth = view.clip_from_view[3][2] / deep;
    out.color = vec4(plane.color.rgb, ink * plane.color.a * fade);
    return out;
}
