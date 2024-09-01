#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::mesh_bindings
#import bevy_pbr::forward_io::VertexOutput
#import bevy_core_pipeline::tonemapping::tone_mapping

#import bevy_pbr::pbr_types
#import bevy_pbr::utils::PI

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var N = normalize(in.world_normal);
    var V = normalize(view.world_position.xyz - in.world_position.xyz);
    let NdotV = max(dot(N, V), 0.0001);

    let glow = pow(NdotV, 2.0);
    return vec4(1.0, 0.5, 0.0, 0.6) * glow;
}
