// MIT; Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
#include <metal_stdlib>
using namespace metal;
// packed_float3 is essential: float3 would align to 16 and break the 24-byte ABI.
struct TerrainVertex24 { packed_float3 pos; ushort2 uv; ushort layer; ushort light; uint color; };
static_assert(sizeof(TerrainVertex24) == 24, "terrain stride");
static_assert(alignof(TerrainVertex24) == 4, "terrain alignment");
static_assert(__builtin_offsetof(TerrainVertex24, uv) == 12, "terrain UV offset");
static_assert(__builtin_offsetof(TerrainVertex24, color) == 20, "terrain color offset");
struct View24 { float4x4 clip; float4 origin_uv; };
struct Raster24 { float4 clip [[position]]; float2 uv; float3 lit; uint layer [[flat]]; };

vertex Raster24 terrain24_vertex(uint id [[vertex_id]],
    device const TerrainVertex24* vertices [[buffer(0)]], constant View24& view [[buffer(1)]],
    texture2d<float> lightmap [[texture(0)]]) {
    TerrainVertex24 v = vertices[id];
    float2 light = float2(v.light >> 8, v.light & 255) / 256.0 + 0.5 / 16.0;
    constexpr sampler lm(coord::normalized, address::clamp_to_edge, filter::linear);
    float3 baked = float3(v.color & 255, (v.color >> 8) & 255, (v.color >> 16) & 255) / 255.0;
    Raster24 out;
    out.clip = view.clip * float4(float3(v.pos) + view.origin_uv.xyz, 1.0);
    out.uv = float2(v.uv) * view.origin_uv.w;
    out.layer = v.layer; // includes OPAQUE_LAYER; color alpha stays metadata in the input.
    out.lit = baked * lightmap.sample(lm, clamp(light, float2(0.5/16.0), float2(15.5/16.0)), level(0.0)).rgb;
    return out;
}

fragment float4 terrain24_fragment(Raster24 in [[stage_in]], texture2d_array<float> sprites [[texture(0)]]) {
    // Preserve the section UV convention, including exact 1 at sprite edges.
    float2 uv = in.uv - max(ceil(in.uv) - 1.0, float2(0.0));
    constexpr sampler point(coord::normalized, address::clamp_to_edge, filter::nearest);
    float4 texel = sprites.sample(point, uv, in.layer & 0x7fff);
    if ((in.layer & 0x8000) == 0 && texel.a < 0.5) discard_fragment();
    // SDR solid terrain coverage is opaque after cutout; color's alpha encodes
    // emission/fluid flags, not coverage. HDR/translucent passes are future work.
    return float4(texel.rgb * in.lit, 1.0);
}
