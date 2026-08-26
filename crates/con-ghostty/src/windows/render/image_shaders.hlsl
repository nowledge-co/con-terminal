// Kitty image placement pipeline. Source textures are uploaded as
// premultiplied RGBA8 so LINEAR filtering cannot mix hidden RGB from
// transparent neighbours into visible edge pixels.

cbuffer ImageGlobals : register(b0) {
    float2 invViewport;
    float2 _padding;
};

struct ImageInstance {
    float4 destination : DESTINATION; // left, top, right, bottom in pixels
    float4 source      : SOURCE;      // u0, v0, u1, v1
    float4 sourceClamp : SOURCECLAMP; // first/last source texel centres
};

struct VSOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
    nointerpolation float4 sourceClamp : TEXCOORD1;
};

VSOut vs_main(uint vid : SV_VertexID, ImageInstance inst) {
    const uint2 corners[6] = {
        uint2(0, 0), uint2(1, 0), uint2(0, 1),
        uint2(0, 1), uint2(1, 0), uint2(1, 1),
    };
    uint2 corner = corners[vid];
    float2 px = float2(
        corner.x == 0 ? inst.destination.x : inst.destination.z,
        corner.y == 0 ? inst.destination.y : inst.destination.w
    );

    VSOut o;
    o.pos = float4(px * invViewport + float2(-1.0, 1.0), 0.0, 1.0);
    o.uv = float2(
        corner.x == 0 ? inst.source.x : inst.source.z,
        corner.y == 0 ? inst.source.y : inst.source.w
    );
    o.sourceClamp = inst.sourceClamp;
    return o;
}

Texture2D<float4> image : register(t0);
SamplerState imageSampler : register(s0);

float4 ps_main(VSOut i) : SV_Target {
    float2 uv = clamp(i.uv, i.sourceClamp.xy, i.sourceClamp.zw);
    return image.Sample(imageSampler, uv);
}
