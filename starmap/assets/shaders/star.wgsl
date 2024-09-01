struct FragmentInput {
    @builtin(front_facing) is_front: bool,
    @builtin(position) frag_coord: vec4<f32>,
    #import bevy_pbr::meshs_vertex_output
};

@fragment
fn fragment(in: FragmentInput) -> @location(0) vec4<f32> {
    return vec4(1.0, 1.0, 1.0, 1.0);
}
