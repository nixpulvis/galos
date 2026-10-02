// The glow's evenly filled cells as one volume, marched along each pixel's
// ray: one quad over the volume, and each pixel walks its ray from box to
// box, a cell of the volume or a ghost, integrating the field through each.
// The light is summed here, in full precision, and laid down once: summed
// by the blend a box at a time instead, each pixel's few dozen half-float
// roundings stepped wherever a box's edge crossed the frame.
//
// The field is `crate::map::paint::volume`'s basis: at a point, the mean of
// the densities of the cells whose tents cover it, each weighted by its tent.
// Each box's list names every cell whose tent reaches into it and which of
// its octants each reaches, so a point asks only the cells that cover it.
//
// The box a point is in is found in the volume's table, which holds every
// box and every cell holding one by its place in the cube: the levels held
// at a point run unbroken from the root to the box it is in, or to the
// empty cell it is in, so walking down or up from the last box's level
// finds which, mostly in a look or two. An empty cell is stepped over whole.
//
// The pixel's frustum is `(t·p)²` square light years across at depth `t`,
// `p` a logical pixel's width a light year out, and the ray runs a unit step
// into the screen, so the distance along it is the depth: the light a pixel
// takes is the integral of the field times `t² p²` along it.
//
// Within a box the ray is cut wherever the field can turn — on the box's
// grid of half cells of the finest cell on its list — and each piece is
// integrated by three-point Gauss–Legendre. Between cells of one size the
// field there is the product of three lines, which times `t²` is of degree
// five and integrated exactly; where sizes change it is a ratio of such,
// smooth, and near.

#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}

struct Frame {
    right: vec3<f32>,
    // Light years a logical pixel covers at a depth of one light year.
    per_pixel: f32,
    up: vec3<f32>,
    // The table's size less one.
    mask: u32,
    forward: vec3<f32>,
    // Light years the volume fades out over inside the reach.
    soft: f32,
    // The reach's centre from the eye, and its radius; nought, not clearing.
    reach: vec3<f32>,
    radius: f32,
    // The eye from the cube's low corner, light years.
    root: vec3<f32>,
    // Which target this draw lays, as the bit a span marks it by.
    target_bit: u32,
    // The corners of the box every box stands in, from the eye.
    low: vec3<f32>,
    _pad0: f32,
    high: vec3<f32>,
    _pad1: f32,
    // Half the frame, logical pixels.
    half: vec2<f32>,
    // Where the point straight ahead lands from the frame's middle, logical
    // pixels with y up: nought but in a piece of a larger picture.
    shift: vec2<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> frame: Frame;
// Each box's centre from the eye and its edge, light years.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<storage, read> boxes: array<vec4<f32>>;
// Each cell's density in this target, light a cubic light year.
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<storage, read> density: array<vec4<f32>>;
// Where each box's list starts; how long it is under bit 24, how many
// levels finer than the box its finest cell is from there, and a bit from
// 30 for each target the box has light to lay in.
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<storage, read> spans: array<vec2<u32>>;
// A cell in the low three bytes, the octants of the box it reaches in the
// high one.
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<storage, read> influence: array<u32>;
// The table: a place's `x`, `y`, and `z` with its level from bit 24, and the
// box there or `HOLDS`.
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<storage, read> table: array<vec4<u32>>;

// The largest finite half float, which is what the field's targets hold.
const HALF_MAX: f32 = 65504.0;

// Three-point Gauss–Legendre: where its nodes stand on `-1..1`, and their
// weights.
const NODE: f32 = 0.7745966692414834;
const OUTER: f32 = 0.5555555555555556;
const INNER: f32 = 0.8888888888888888;

// The cube's edge, and the deepest level a place is counted at.
const ROOT_EDGE: f32 = 131072.0;
const DEEPEST: u32 = 21u;

// What the table says of a place it does not hold, of an empty slot, and of
// a cell holding boxes further down: `crate::map::paint::volume::Table`.
const NONE: u32 = 0xffffffffu;
const EMPTY: u32 = 0xffffffffu;
const HOLDS: u32 = 0xfffffffeu;

// Where a span keeps its length, under the target bits.
const LISTED: u32 = 0xffffffu;
const FINER: u32 = 24u;
const LIGHTS: u32 = 30u;

// The most pieces one box's stretch of a ray is cut into: a box cut for
// neighbours four levels finer is 32 planes an axis.
const PIECES: u32 = 100u;

// The most boxes and empty cells a ray walks through.
const STEPS: u32 = 1024u;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // `x` and `y` which corner of the quad.
    @location(0) position: vec3<f32>,
}

struct Varying {
    @builtin(position) clip: vec4<f32>,
    // The eye's ray through the pixel, one light year deep: linear across
    // the screen, which is exactly how a perspective ray moves.
    @location(0) ray: vec3<f32>,
}

// The quad is laid over the rectangle the volume's bounds land in on
// screen, a pixel wider all round; bounds reaching behind the eye cover the
// whole frame.
@vertex
fn vertex(in: Vertex) -> Varying {
    var low = vec2(3.0e38);
    var high = vec2(-3.0e38);
    var behind = 0u;
    for (var k = 0u; k < 8u; k++) {
        let corner = vec3(
            select(frame.low.x, frame.high.x, (k & 1u) != 0u),
            select(frame.low.y, frame.high.y, (k & 2u) != 0u),
            select(frame.low.z, frame.high.z, (k & 4u) != 0u),
        );
        let depth = dot(corner, frame.forward);
        if depth <= 0.0 {
            behind++;
            continue;
        }
        let at = vec2(dot(corner, frame.right), dot(corner, frame.up))
            / (depth * frame.per_pixel) + frame.shift;
        low = min(low, at);
        high = max(high, at);
    }
    let edge = frame.half + vec2(1.0);
    if behind > 0u {
        low = -edge;
        high = edge;
    } else {
        low = max(low - vec2(1.0), -edge);
        high = min(high + vec2(1.0), edge);
    }
    let xy = mix(low, high, in.position.xy);
    var out: Varying;
    // In front of the origin camera, clear of its near plane, as the splats.
    out.clip = mesh_position_local_to_clip(
        get_world_from_local(in.instance_index),
        vec4(xy, -2.0, 1.0),
    );
    let aside = xy - frame.shift;
    out.ray = frame.forward
        + frame.right * (aside.x * frame.per_pixel)
        + frame.up * (aside.y * frame.per_pixel);
    return out;
}

// Where a place's probe starts in the table: `volume::scatter`.
fn scatter(x: u32, y: u32, tagged: u32) -> u32 {
    var h = (x * 73856093u) ^ (y * 19349663u) ^ (tagged * 83492791u);
    h = h ^ (h >> 15u);
    h = h * 0x2c1b3c6du;
    return h ^ (h >> 12u);
}

// What the table holds at `level` where `q`, from the cube's low corner,
// stands: a box, `HOLDS`, or `NONE`.
fn probe(level: u32, q: vec3<f32>) -> u32 {
    let side = 1u << level;
    let edge = ROOT_EDGE / f32(side);
    let c = vec3<u32>(clamp(floor(q / edge), vec3(0.0), vec3(f32(side - 1u))));
    let tagged = c.z | (level << 24u);
    var slot = scatter(c.x, c.y, tagged) & frame.mask;
    for (var i = 0u; i <= frame.mask; i++) {
        let held = table[slot];
        if held.z == EMPTY {
            return NONE;
        }
        if held.x == c.x && held.y == c.y && held.z == tagged {
            return held.w;
        }
        slot = (slot + 1u) & frame.mask;
    }
    return NONE;
}

// The deepest level the table holds at `q`, and what it holds there; `NONE`
// where it holds nothing, which is outside every box.
//
// Asked from `near`, the level the last box along the ray was found at:
// the next is mostly at that level or one either side of it, so this walks
// up or down from there, a look a level.
fn locate(q: vec3<f32>, near: u32) -> vec2<u32> {
    var level = near;
    var found = probe(level, q);
    if found == NONE {
        // Too deep: up to where the table holds the place.
        loop {
            if level == 0u {
                return vec2(NONE, 0u);
            }
            level -= 1u;
            found = probe(level, q);
            if found != NONE {
                return vec2(found, level);
            }
        }
    }
    // Down while what is held only holds boxes further down.
    while found == HOLDS && level < DEEPEST {
        let deeper = probe(level + 1u, q);
        if deeper == NONE {
            break;
        }
        level += 1u;
        found = deeper;
    }
    return vec2(found, level);
}

// Where the ray leaves the box from `low` to `high`, from the eye, and where
// it is inside it.
fn slab(low: vec3<f32>, high: vec3<f32>, ray: vec3<f32>) -> vec2<f32> {
    // A ray parallel to a pair of faces is held off nought, so it crosses
    // them at a great distance either side, or both on one side when it
    // passes outside them.
    let tiny = vec3(1e-30);
    let along = select(ray, tiny, abs(ray) < tiny);
    let a = low / along;
    let b = high / along;
    let enter = min(a, b);
    let leave = max(a, b);
    return vec2(
        max(max(enter.x, enter.y), enter.z),
        min(min(leave.x, leave.y), leave.z),
    );
}

// How much of the volume the reach leaves at `at`, from the eye: all of it
// well inside, nothing at the sphere, and everywhere where it is not
// clearing.
fn kept(at: vec3<f32>) -> f32 {
    if frame.radius <= 0.0 {
        return 1.0;
    }
    return clamp((frame.radius - length(at - frame.reach)) / frame.soft, 0.0, 1.0);
}

// The field times `t²` integrated along `ray` from `a` to `b`, where both
// are inside one piece of `region`'s box: the mean over the cells covering
// each of three nodes, density times tent and the tents summed.
//
// A piece at a time rather than a box's pieces together: three sums and
// their nodes stay in registers, where four pieces' dozen spilled and ran
// at two thirds the speed.
fn piece(region: u32, ray: vec3<f32>, a: f32, b: f32) -> vec3<f32> {
    let centre = boxes[region].xyz;
    let half = 0.5 * (b - a);
    let mid = 0.5 * (a + b);
    let x = ray * mid;
    let octant = u32(x.x > centre.x) | (u32(x.y > centre.y) << 1u)
        | (u32(x.z > centre.z) << 2u);
    var at: array<vec3<f32>, 3>;
    var weight: array<f32, 3>;
    for (var n = 0u; n < 3u; n++) {
        let t = mid + half * NODE * (f32(n) - 1.0);
        at[n] = ray * t;
        weight[n] = half * select(OUTER, INNER, n == 1u) * t * t * kept(at[n]);
    }
    var sum: array<vec4<f32>, 3>;
    let span = spans[region];
    let last = span.x + (span.y & LISTED);
    for (var i = span.x; i < last; i++) {
        let entry = influence[i];
        if ((entry >> (24u + octant)) & 1u) == 0u {
            continue;
        }
        let k = entry & 0xffffffu;
        let other = boxes[k];
        let rho = density[k].rgb;
        let inv = 1.0 / other.w;
        for (var n = 0u; n < 3u; n++) {
            let d = max(vec3(1.0) - abs(at[n] - other.xyz) * inv, vec3(0.0));
            let w = d.x * d.y * d.z;
            sum[n] += vec4(rho * w, w);
        }
    }
    var light = vec3(0.0);
    for (var n = 0u; n < 3u; n++) {
        if sum[n].w > 0.0 {
            light += sum[n].rgb / sum[n].w * weight[n];
        }
    }
    return light;
}

// The field times `t²` integrated along `ray` through `region`'s box, from
// `t0` to `t1`, both inside it.
//
// Cut wherever the field can turn, so each piece is smooth: at the planes
// through the centres of the cells on the box's list and where their tents
// end. Every one of those is on the box's own grid of half cells of the
// finest of them, so the ray is walked across that grid — the box's halves
// where its neighbours are its own size or coarser. Left uncut, a turn
// inside a piece is rounded over by the integration, by how much depending
// on where the ray crosses it: thin lines across the frame, a percent deep.
fn through(region: u32, ray: vec3<f32>, t0: f32, t1: f32) -> vec3<f32> {
    let cell = boxes[region];
    let finer = (spans[region].y >> FINER) & 0x3fu;
    let side = 2u << finer;
    let pitch = cell.w / f32(side);
    let low = cell.xyz - vec3(0.5 * cell.w);
    let tiny = vec3(1e-30);
    let level_with = abs(ray) < tiny;
    let along = select(ray, tiny, level_with);
    let up = ray > vec3(0.0);
    let never = vec3(3.0e38);

    var light = vec3(0.0);
    var t = t0;
    for (var cut = 0u; cut < PIECES && t < t1; cut++) {
        // Which square of the grid the ray is in a hair past `t`, and where
        // it next crosses a plane of it; the rest of the way where rounding
        // finds none ahead, or the cuts run out.
        let here = (ray * (t + (t1 - t) * 1e-4) - low) / pitch;
        let square = clamp(floor(here), vec3(0.0), vec3(f32(side - 1u)));
        let plane = low + (square + select(vec3(0.0), vec3(1.0), up)) * pitch;
        let crossed = select(plane / along, never, level_with);
        let ahead = select(crossed, never, crossed <= vec3(t));
        var end = min(min(min(ahead.x, ahead.y), ahead.z), t1);
        if cut + 1u == PIECES {
            end = t1;
        }
        light += piece(region, ray, t, end);
        t = end;
    }
    return light;
}

@fragment
fn fragment(in: Varying) -> @location(0) vec4<f32> {
    let ray = in.ray;

    // Where the ray is inside the volume's bounds, in front of the eye and
    // inside the reach.
    let bounds = slab(frame.low, frame.high, ray);
    var t = max(bounds.x, 0.0);
    var end = bounds.y;
    if frame.radius > 0.0 {
        let rr = dot(ray, ray);
        let along = dot(ray, frame.reach);
        let off = dot(frame.reach, frame.reach) - frame.radius * frame.radius;
        let disc = along * along - rr * off;
        if disc <= 0.0 {
            return vec4(0.0);
        }
        let s = sqrt(disc);
        t = max(t, (along - s) / rr);
        end = min(end, (along + s) / rr);
    }

    // From box to box: find the one a hair past where the ray is, lay its
    // stretch, and go on from where the ray leaves it; over an empty cell
    // whole. Never back, so no stretch is laid twice whatever the rounding.
    var light = vec3(0.0);
    var near = 0u;
    for (var walked = 0u; walked < STEPS && t < end; walked++) {
        let nudge = max(t * 1e-6, 1e-3);
        let q = frame.root + ray * (t + nudge);
        let found = locate(q, near);
        near = found.y;
        if found.x == NONE {
            break;
        }
        var leave: f32;
        if found.x == HOLDS {
            let edge = ROOT_EDGE / f32(1u << (found.y + 1u));
            let corner = floor(q / edge) * edge - frame.root;
            leave = slab(corner, corner + vec3(edge), ray).y;
        } else {
            let cell = boxes[found.x];
            let h = 0.5 * cell.w;
            let inside = slab(cell.xyz - h, cell.xyz + h, ray);
            leave = inside.y;
            let start = max(t, inside.x);
            let to = min(leave, end);
            if ((spans[found.x].y >> LIGHTS) & frame.target_bit) != 0u && to > start {
                light += through(found.x, ray, start, to);
            }
        }
        t = max(leave, t + nudge);
    }
    light *= frame.per_pixel * frame.per_pixel;

    // Held to what the target holds, a half float. A clamp rather than a
    // test for a number: Metal compiles `x == x` to true, and a clamp takes
    // anything to an end of the range.
    //
    // Alpha nought: an added material is drawn through the premultiplied
    // blend, which adds only where the alpha is nought.
    return vec4(clamp(light, vec3(0.0), vec3(HALF_MAX)), 0.0);
}
