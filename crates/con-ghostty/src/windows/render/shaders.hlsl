// Con Windows terminal cell renderer — HLSL.
//
// Kitty graphics can appear below cell backgrounds, between backgrounds
// and text, or above text. Cell rendering is therefore split into three
// passes over one instance buffer:
//
//   ps_background  opaque/selected cell backgrounds
//   ps_cursor      cursor block (text-layer decoration)
//   ps_text        glyph coverage, underline, and strikethrough
//
// The background VS always emits one-cell quads. The text VS may widen a
// quad to preserve oversized Nerd Font glyphs.

cbuffer Globals : register(b0) {
    float2 invViewport;
    float2 cellSize;
    uint   gridCols;
    uint   gridRows;
    float2 invAtlasSize;
};

struct VSInstance {
    uint2  cellPos       : CELLPOS;
    uint2  atlasPos      : ATLAS_POS;
    uint2  atlasSize     : ATLAS_SIZE;
    uint   fg            : FGCOLOR;
    uint   bg            : BGCOLOR;
    // Low bits mirror Ghostty cell attrs. Con reserves:
    //   bit 8 = default background
    //   bit 9 = cursor cell
    uint   attrs         : ATTRS;
};

struct VSOut {
    float4 pos           : SV_Position;
    float2 atlasUV       : TEXCOORD0;
    float2 cellUV        : TEXCOORD1;
    nointerpolation float4 fg : FGCOLOR;
    nointerpolation float4 bg : BGCOLOR;
    nointerpolation uint   attrs : ATTRS;
};

static const uint ATTR_UNDERLINE  = 4u;
static const uint ATTR_STRIKE     = 8u;
static const uint ATTR_INVERSE    = 16u;
static const uint ATTR_DEFAULT_BG = 256u;
static const uint ATTR_CURSOR     = 512u;

float4 unpackRGBA(uint v) {
    return float4(
        float((v >> 24) & 0xFF),
        float((v >> 16) & 0xFF),
        float((v >>  8) & 0xFF),
        float( v        & 0xFF)
    ) / 255.0;
}

uint2 quadCorner(uint vid) {
    const uint2 mapping[4] = {
        uint2(0, 0),
        uint2(1, 0),
        uint2(0, 1),
        uint2(1, 1),
    };
    return mapping[vid % 4];
}

VSOut vertexOut(uint vid, VSInstance inst, float2 quadSize) {
    uint2 corner = quadCorner(vid);
    float2 px = float2(inst.cellPos) * cellSize + float2(corner) * quadSize;

    VSOut o;
    o.pos = float4(px * invViewport + float2(-1.0, 1.0), 0.0, 1.0);
    o.atlasUV = (float2(inst.atlasPos) + float2(inst.atlasSize) * float2(corner))
        * invAtlasSize;
    o.cellUV = float2(corner);
    o.fg = unpackRGBA(inst.fg);
    o.bg = unpackRGBA(inst.bg);
    o.attrs = inst.attrs;
    return o;
}

Texture2D<float4> atlas : register(t0);
SamplerState      samp  : register(s0);

VSOut vs_text(uint vid : SV_VertexID, VSInstance inst) {
    float2 quadSize = float2(
        max(cellSize.x, float(inst.atlasSize.x)),
        cellSize.y
    );
    return vertexOut(vid, inst, quadSize);
}

VSOut vs_cell(uint vid : SV_VertexID, VSInstance inst) {
    return vertexOut(vid, inst, cellSize);
}

void effectiveColors(VSOut i, out float4 fg, out float4 bg) {
    fg = i.fg;
    bg = i.bg;
    if (i.attrs & ATTR_INVERSE) {
        float4 tmp = fg;
        fg = bg;
        bg = tmp;
        // Selection and inverse-video cells are solid highlights even when
        // the terminal's default background is translucent.
        fg.a = 1.0;
        bg.a = 1.0;
    }
}

float4 premultiply(float4 color) {
    return float4(color.rgb * color.a, color.a);
}

float4 ps_background(VSOut i) : SV_Target {
    // The renderer clear already supplies the default background. Keeping
    // these pixels untouched is also what lets below-background images show
    // through empty/default cells, matching upstream Ghostty's cell-bg pass.
    if ((i.attrs & ATTR_CURSOR) ||
        ((i.attrs & ATTR_DEFAULT_BG) && !(i.attrs & ATTR_INVERSE))) {
        discard;
    }

    float4 fg;
    float4 bg;
    effectiveColors(i, fg, bg);
    return premultiply(bg);
}

float4 ps_cursor(VSOut i) : SV_Target {
    if (!(i.attrs & ATTR_CURSOR)) {
        discard;
    }

    float4 fg;
    float4 bg;
    effectiveColors(i, fg, bg);
    // The cursor toggles the effective cell colors and stays opaque even
    // when the effective foreground came from the translucent default bg.
    fg.a = 1.0;
    return premultiply(fg);
}

float4 ps_text(VSOut i) : SV_Target {
    float3 coverageRgb = atlas.Sample(samp, i.atlasUV).rgb;
    float coverage = max(coverageRgb.r, max(coverageRgb.g, coverageRgb.b));

    float pxUV = 1.0 / max(cellSize.y, 1.0);
    float bandCoverage = 0.0;
    if ((i.attrs & ATTR_UNDERLINE) && abs(i.cellUV.y - 0.92) < pxUV) {
        bandCoverage = 1.0;
    }
    if ((i.attrs & ATTR_STRIKE) && abs(i.cellUV.y - 0.52) < pxUV) {
        bandCoverage = 1.0;
    }

    float4 fg;
    float4 bg;
    effectiveColors(i, fg, bg);
    float4 color = (i.attrs & ATTR_CURSOR) ? bg : fg;
    if (i.attrs & ATTR_CURSOR) {
        color.a = 1.0;
    }

    float alpha = color.a * max(coverage, bandCoverage);
    return float4(color.rgb * alpha, alpha);
}
