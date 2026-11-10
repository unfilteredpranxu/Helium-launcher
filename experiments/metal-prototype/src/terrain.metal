#include <metal_stdlib>
using namespace metal;
struct Vertex { short4 position; ushort2 uv; uchar4 tint; };
static_assert(sizeof(Vertex) == 16, "Rust/MSL vertex stride mismatch");
struct Raster { float4 position [[position]]; float2 uv; float4 tint; };
vertex Raster terrain_vertex(uint id [[vertex_id]], device const Vertex* vertices [[buffer(0)]],
                             constant float4x4& clip_from_world [[buffer(1)]]) {
    Vertex v = vertices[id];
    return {clip_from_world * float4(float3(v.position.xyz), 1.0),
            float2(v.uv) / 65535.0, float4(v.tint) / 255.0};
}
fragment float4 terrain_fragment(Raster in [[stage_in]], texture2d<float> albedo [[texture(0)]]) {
    constexpr sampler nearest(coord::normalized, address::repeat, filter::nearest);
    return albedo.sample(nearest, in.uv) * in.tint;
}
