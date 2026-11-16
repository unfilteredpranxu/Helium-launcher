// MIT; Copyright (c) 2026 Horizon contributors.
// Appended to the unchanged G02 source; CPU dispatch preserves triangle order.
fragment float4 terrain_banked_fragment(Raster24 in [[stage_in]],
    texture2d_array<float> sprites [[texture(0)]], constant uint& first [[buffer(0)]]) {
    float2 uv = in.uv - max(ceil(in.uv)-1.0,float2(0.0));
    constexpr sampler point(coord::normalized,address::clamp_to_edge,filter::nearest);
    float4 texel=sprites.sample(point,uv,(in.layer & 0x7fff)-first);
    if ((in.layer & 0x8000)==0 && texel.a<0.5) discard_fragment();
    return float4(texel.rgb*in.lit,1.0);
}
