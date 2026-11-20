// MIT; Copyright (c) 2026 Horizon contributors.
#include <metal_stdlib>
using namespace metal;

struct HudVertex {
    packed_float2 pos;
    packed_float2 uv;
    uint color;
    float depth;
    packed_float2 glint_uv;
    uint text;
};
static_assert(sizeof(HudVertex) == 36, "HUD vertex ABI");

struct HudPixel {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

vertex HudPixel hud_vertex(uint id [[vertex_id]], device const HudVertex* vertices [[buffer(0)]],
                          constant float4& gui_to_clip [[buffer(1)]]) {
    HudVertex v = vertices[id];
    float2 p = float2(v.pos) * gui_to_clip.xy + gui_to_clip.zw;
    uint c = v.color;
    return {float4(p, v.depth, 1.0), float2(v.uv),
            float4(c & 255, (c >> 8) & 255, (c >> 16) & 255, c >> 24) / 255.0};
}

fragment float4 hud_fragment(HudPixel in [[stage_in]], texture2d<float> atlas [[texture(0)]]) {
    // UVs >= 2 encode the optional enchantment glint in the CPU batch.
    if (in.uv.x >= 2.0) discard_fragment();
    constexpr sampler nearest(filter::nearest, address::clamp_to_edge);
    float4 color = atlas.sample(nearest, in.uv) * in.color;
    if (color.a < 0.004) discard_fragment();
    return color;
}

fragment float4 hud_vignette(HudPixel in [[stage_in]], texture2d<float> atlas [[texture(0)]]) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    return float4(atlas.sample(linear, in.uv).rgb * in.color.rgb, 1.0);
}
