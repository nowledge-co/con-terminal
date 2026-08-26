// Kitty image placement pipeline. Source textures contain straight-alpha
// RGBA8; the pixel shader premultiplies before source-over blending into the
// renderer's premultiplied BGRA target.

cbuffer ImageGlobals : register(b0) {
    float2 invViewport;
    float2 _padding;
};

struct ImageInstance {
    float4 destination : DESTINATION; // left, top, right, bottom in pixels
    float4 source      : SOURCE;      // u0, v0, u1, v1
};

struct VSOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
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
    return o;
}

Texture2D<float4> image : register(t0);
SamplerState imageSampler : register(s0);

float4 ps_main(VSOut i) : SV_Target {
    float4 color = image.Sample(imageSampler, i.uv);
    return float4(color.rgb * color.a, color.a);
}
