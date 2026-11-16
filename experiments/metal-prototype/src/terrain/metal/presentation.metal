// MIT; Copyright (c) 2026 Horizon contributors.
#include <metal_stdlib>
using namespace metal;
struct DisplayVertex { float4 position [[position]]; float2 uv; };
vertex DisplayVertex display_vertex(uint id [[vertex_id]]) {
    float2 p = id == 0 ? float2(-1,-1) : id == 1 ? float2(3,-1) : float2(-1,3);
    return {float4(p,0,1),float2(p.x*.5+.5,.5-p.y*.5)};
}
fragment float4 display_fragment(DisplayVertex in [[stage_in]], texture2d<float> source [[texture(0)]]) {
    constexpr sampler point(coord::normalized,address::clamp_to_edge,filter::nearest);
    return source.sample(point,in.uv,level(0));
}
