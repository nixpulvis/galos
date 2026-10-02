// One piece of an enhanced picture laid over the window: `crate::map::enhance`.
//
// A window pixel takes in `across` of the piece's texels each way, several
// where the whole picture is shown on the window and a fraction where it is
// looked into closely. Sampled once, the pixel would land on one texel of
// the several it covers and the picture would shimmer and drop the fainter
// marks; so it is the average of them, taken a two-by-two block a tap
// through the sampler's own filtering.

#import bevy_pbr::forward_io::VertexOutput

struct Footprint {
    across: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> footprint: Footprint;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var piece: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var piece_sampler: sampler;

// The most taps a side: eight two-texel taps is sixteen texels a pixel,
// past anything a scale the controls offer asks for.
const TAPS: u32 = 8u;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(piece));
    let across = max(footprint.across, 1.0);
    let taps = u32(clamp(ceil(across / 2.0), 1.0, f32(TAPS)));
    let step = across / f32(taps);
    var sum = vec4(0.0);
    for (var i = 0u; i < taps; i++) {
        for (var j = 0u; j < taps; j++) {
            let off = (vec2(f32(i), f32(j)) + 0.5) * step - 0.5 * across;
            sum += textureSampleLevel(piece, piece_sampler, in.uv + off / size, 0.0);
        }
    }
    let color = sum / f32(taps * taps);
    return vec4(color.rgb, 1.0);
}
