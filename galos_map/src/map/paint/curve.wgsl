// The field's curve: the two targets the marks and the splats were summed
// into, brought onto the display a pixel at a time.
//
// The curve is read in stops of light over an average system's mark and
// comes out as a display level, sRGB-encoded, which is what the knots are
// dragged in. It is `crate::map::paint::curve::FieldCurve::level`, and the
// two are kept the
// same function by hand: the knots and their tangents are worked out on the
// CPU and handed over whole, so all this does is the Hermite step.

#import bevy_pbr::forward_io::VertexOutput

const KNOTS: u32 = 7u;

struct Curve {
    // Per knot: where it stands in stops, its display level, and the
    // curve's slope through it in levels a stop.
    knots: array<vec4<f32>, KNOTS>,
    // Linear light to marks, with the dial and its stops already in it.
    gain: f32,
    // What share of its own curve the dimmed target is drawn at.
    dim: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> curve: Curve;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var lit: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var dimmed: texture_2d<f32>;

// The largest finite half float. A pixel summed past it is infinite, and an
// infinite channel over an infinite top is not a number.
const HALF_MAX: f32 = 65504.0;

// An sRGB-encoded level as linear light.
fn decode(level: f32) -> f32 {
    let v = clamp(level, 0.0, 1.0);
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

// The display's linear light for a pixel `stops` over one mark.
fn shown(stops: f32) -> f32 {
    let first = curve.knots[0];
    // Under the first knot, linear in the light: half the light is half the
    // display's, down to black.
    if stops <= first.x {
        return decode(first.y) * exp2(stops - first.x);
    }
    let last = curve.knots[KNOTS - 1u];
    if stops >= last.x {
        return decode(last.y);
    }
    for (var k = 0u; k < KNOTS - 1u; k++) {
        let a = curve.knots[k];
        let b = curve.knots[k + 1u];
        if stops <= b.x {
            let h = b.x - a.x;
            let t = (stops - a.x) / h;
            let t2 = t * t;
            let t3 = t2 * t;
            let level = (2.0 * t3 - 3.0 * t2 + 1.0) * a.y
                + (t3 - 2.0 * t2 + t) * h * a.z
                + (-2.0 * t3 + 3.0 * t2) * b.y
                + (t3 - t2) * h * b.z;
            return decode(level);
        }
    }
    return decode(last.y);
}

// A pixel through the curve, struck on its brightest channel and applied to
// all three, so what moves is the brightness and not the colour.
fn through(light: vec3<f32>) -> vec3<f32> {
    let c = clamp(light, vec3(0.0), vec3(HALF_MAX));
    let top = max(c.r, max(c.g, c.b));
    if top <= 0.0 {
        return vec3(0.0);
    }
    return c * (shown(log2(top * curve.gain)) / top);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // A pixel of the frame is a texel of the targets, which are sized to it;
    // held inside them for the frame a resize is still landing on.
    let size = vec2<i32>(textureDimensions(lit)) - vec2(1);
    let at = clamp(vec2<i32>(in.position.xy), vec2(0), size);
    let a = textureLoad(lit, at, 0).rgb;
    let b = textureLoad(dimmed, at, 0).rgb;
    // Alpha nought: an added material is drawn through the premultiplied
    // blend, `src + dst * (1 - src.a)`, which adds only where the alpha is
    // nought. At one it laid the field over the galaxy as a sheet and took
    // the ruled plane under it off the frame.
    return vec4(through(a) + curve.dim * through(b), 0.0);
}
