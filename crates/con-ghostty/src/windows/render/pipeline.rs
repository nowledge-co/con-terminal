//! D3D11 cell pipeline: HLSL compile, IA layout, buffers, and layered draws.
//!
//! One uploaded instance buffer is replayed for cell backgrounds, the cursor,
//! and glyphs. This leaves room for Kitty image draws between those passes
//! without duplicating per-cell CPU work.

use std::ffi::CString;

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_INDEX_BUFFER, D3D11_BIND_VERTEX_BUFFER,
    D3D11_BLEND_DESC, D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD,
    D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_CPU_ACCESS_WRITE, D3D11_CULL_NONE,
    D3D11_FILL_SOLID, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_INSTANCE_DATA,
    D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_RASTERIZER_DESC,
    D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SUBRESOURCE_DATA, D3D11_USAGE_DYNAMIC,
    D3D11_USAGE_IMMUTABLE, ID3D11BlendState, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext,
    ID3D11InputLayout, ID3D11PixelShader, ID3D11RasterizerState, ID3D11SamplerState,
    ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_UINT};

use super::atlas::GlyphRect;

const HLSL_SOURCE: &str = include_str!("shaders.hlsl");

/// Per-cell instance pushed to the GPU. Exactly matches the HLSL
/// `VSInstance` layout (36 bytes). Keep in sync with
/// [`shaders.hlsl`].
///
/// `atlas_rect` is split into `atlas_pos` + `atlas_size` (two
/// `R32G32_UINT` slots) rather than a single `R32G32B32A32_UINT`
/// — the latter at offset 8 (non-16-aligned) has been observed to
/// produce partial / zeroed reads on some AMD Radeon drivers.
/// Two 8-byte slots at 8-byte-aligned offsets sidestep the issue
/// entirely and cost nothing at runtime.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Instance {
    pub cell_pos: [u32; 2],   // CELLPOS
    pub atlas_pos: [u32; 2],  // ATLAS_POS (x, y)
    pub atlas_size: [u32; 2], // ATLAS_SIZE (w, h)
    pub fg: u32,              // FGCOLOR
    pub bg: u32,              // BGCOLOR
    pub attrs: u32,           // ATTRS
}

/// Per-frame constant buffer matching `cbuffer Globals` in HLSL.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Globals {
    pub inv_viewport: [f32; 2],
    pub cell_size: [f32; 2],
    pub grid_cols: u32,
    pub grid_rows: u32,
    /// (1/atlas_w, 1/atlas_h) — lets the VS normalize pixel-space
    /// glyph UVs into the [0,1] range the sampler expects.
    pub inv_atlas_size: [f32; 2],
}

pub struct Pipeline {
    text_vs: ID3D11VertexShader,
    cell_vs: ID3D11VertexShader,
    background_ps: ID3D11PixelShader,
    cursor_ps: ID3D11PixelShader,
    text_ps: ID3D11PixelShader,
    pub input_layout: ID3D11InputLayout,
    pub sampler: ID3D11SamplerState,
    alpha_blend: ID3D11BlendState,
    /// Explicit no-cull rasterizer — guarantees both triangles of
    /// each cell-quad are rendered regardless of the winding we
    /// happen to produce. Cheaper than reasoning about
    /// `FrontCounterClockwise` vs. default cull semantics.
    pub rasterizer: ID3D11RasterizerState,

    /// 6 indices for a two-triangle quad, immutable.
    pub index_buffer: ID3D11Buffer,
    /// Per-frame instance data (dynamic, WRITE_DISCARD).
    pub instance_buffer: ID3D11Buffer,
    pub instance_capacity: u32,

    /// Per-frame globals.
    pub globals_buffer: ID3D11Buffer,
}

impl Pipeline {
    pub fn new(device: &ID3D11Device, initial_instance_capacity: u32) -> Result<Self> {
        let text_vs_blob = compile_shader(HLSL_SOURCE, "vs_text", "vs_5_0")?;
        let cell_vs_blob = compile_shader(HLSL_SOURCE, "vs_cell", "vs_5_0")?;
        let background_ps_blob = compile_shader(HLSL_SOURCE, "ps_background", "ps_5_0")?;
        let cursor_ps_blob = compile_shader(HLSL_SOURCE, "ps_cursor", "ps_5_0")?;
        let text_ps_blob = compile_shader(HLSL_SOURCE, "ps_text", "ps_5_0")?;

        let text_vs = create_vertex_shader(device, &text_vs_blob)?;
        let cell_vs = create_vertex_shader(device, &cell_vs_blob)?;
        let background_ps = create_pixel_shader(device, &background_ps_blob)?;
        let cursor_ps = create_pixel_shader(device, &cursor_ps_blob)?;
        let text_ps = create_pixel_shader(device, &text_ps_blob)?;

        // Matches the `VSInstance` struct in shaders.hlsl. All per-instance.
        let cellpos = CString::new("CELLPOS").unwrap();
        let atlas_pos = CString::new("ATLAS_POS").unwrap();
        let atlas_size = CString::new("ATLAS_SIZE").unwrap();
        let fg = CString::new("FGCOLOR").unwrap();
        let bg = CString::new("BGCOLOR").unwrap();
        let attrs = CString::new("ATTRS").unwrap();

        let layout = [
            instance_elem(&cellpos, 0, DXGI_FORMAT_R32G32_UINT, 0),
            instance_elem(&atlas_pos, 0, DXGI_FORMAT_R32G32_UINT, 8),
            instance_elem(&atlas_size, 0, DXGI_FORMAT_R32G32_UINT, 16),
            instance_elem(&fg, 0, DXGI_FORMAT_R32_UINT, 24),
            instance_elem(&bg, 0, DXGI_FORMAT_R32_UINT, 28),
            instance_elem(&attrs, 0, DXGI_FORMAT_R32_UINT, 32),
        ];

        let mut input_layout: Option<ID3D11InputLayout> = None;
        // SAFETY: layout and vs_bytes live for the call.
        unsafe {
            device.CreateInputLayout(&layout, blob_slice(&text_vs_blob), Some(&mut input_layout))
        }
        .context("CreateInputLayout failed")?;
        let input_layout = input_layout.context("CreateInputLayout produced no layout")?;

        // Sampler: point clamp — pixel-accurate atlas reads.
        let sampler = create_point_sampler(device)?;
        let alpha_blend = create_premultiplied_alpha_blend(device)?;

        // No-cull rasterizer: we compose each cell from two triangles
        // and don't want to worry about which winding happens to face
        // the camera in NDC after the Y-flip in our VS.
        let rasterizer = create_no_cull_rasterizer(device)?;

        // Index buffer: 6 indices (two triangles, CCW).
        let indices: [u32; 6] = [0, 1, 2, 2, 1, 3];
        let index_buffer =
            create_immutable_buffer(device, &indices, D3D11_BIND_INDEX_BUFFER.0 as u32)?;

        // Instance buffer: dynamic.
        let instance_buffer = create_dynamic_instance_buffer(device, initial_instance_capacity)?;

        // Globals constant buffer.
        let globals_buffer = create_dynamic_cbuffer(device, std::mem::size_of::<Globals>() as u32)?;

        Ok(Self {
            text_vs,
            cell_vs,
            background_ps,
            cursor_ps,
            text_ps,
            input_layout,
            sampler,
            alpha_blend,
            rasterizer,
            index_buffer,
            instance_buffer,
            instance_capacity: initial_instance_capacity,
            globals_buffer,
        })
    }

    /// Grow the instance buffer to `new_capacity` cells when the grid
    /// expands past what we've allocated. Drops the old buffer; GPU
    /// drivers allocate from a renamed pool so this is cheap.
    pub fn ensure_instance_capacity(
        &mut self,
        device: &ID3D11Device,
        new_capacity: u32,
    ) -> Result<()> {
        if new_capacity <= self.instance_capacity {
            return Ok(());
        }
        self.instance_buffer = create_dynamic_instance_buffer(device, new_capacity)?;
        self.instance_capacity = new_capacity;
        Ok(())
    }

    /// Upload `instances` via `Map(WRITE_DISCARD)`. `Unmap` on drop of
    /// the returned guard — callers should invoke immediately.
    pub fn upload_instances(
        &self,
        context: &ID3D11DeviceContext,
        instances: &[Instance],
    ) -> Result<()> {
        debug_assert!(
            instances.len() as u32 <= self.instance_capacity,
            "instance buffer overflow"
        );
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: instance_buffer is DYNAMIC + CPU_WRITE; WRITE_DISCARD
        // is the contract. We unmap before returning.
        unsafe {
            context
                .Map(
                    &self.instance_buffer,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )
                .context("Map(instance_buffer) failed")?;
            let dst = mapped.pData as *mut Instance;
            std::ptr::copy_nonoverlapping(instances.as_ptr(), dst, instances.len());
            context.Unmap(&self.instance_buffer, 0);
        }
        Ok(())
    }

    pub fn upload_globals(&self, context: &ID3D11DeviceContext, globals: &Globals) -> Result<()> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: cbuffer is DYNAMIC + CPU_WRITE.
        unsafe {
            context
                .Map(
                    &self.globals_buffer,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )
                .context("Map(globals) failed")?;
            *(mapped.pData as *mut Globals) = *globals;
            context.Unmap(&self.globals_buffer, 0);
        }
        Ok(())
    }

    fn bind_common(&self, context: &ID3D11DeviceContext) {
        unsafe {
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.IASetInputLayout(&self.input_layout);
            context.RSSetState(&self.rasterizer);

            let stride = std::mem::size_of::<Instance>() as u32;
            context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(self.instance_buffer.clone())),
                Some(&stride),
                Some(&0),
            );
            context.IASetIndexBuffer(
                &self.index_buffer,
                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT,
                0,
            );
            context.VSSetConstantBuffers(0, Some(&[Some(self.globals_buffer.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(self.globals_buffer.clone())]));
        }
    }

    pub fn draw_backgrounds(
        &self,
        context: &ID3D11DeviceContext,
        first_instance: u32,
        instance_count: u32,
    ) {
        if instance_count == 0 {
            return;
        }
        self.bind_common(context);
        unsafe {
            context.OMSetBlendState(None, None, u32::MAX);
            context.VSSetShader(&self.cell_vs, None);
            context.PSSetShader(&self.background_ps, None);
            context.DrawIndexedInstanced(6, instance_count, 0, 0, first_instance);
        }
    }

    pub fn draw_cursor(&self, context: &ID3D11DeviceContext, instance: Option<u32>) {
        let Some(instance) = instance else {
            return;
        };
        self.bind_common(context);
        unsafe {
            context.OMSetBlendState(None, None, u32::MAX);
            context.VSSetShader(&self.cell_vs, None);
            context.PSSetShader(&self.cursor_ps, None);
            context.DrawIndexedInstanced(6, 1, 0, 0, instance);
        }
    }

    pub fn draw_text(
        &self,
        context: &ID3D11DeviceContext,
        atlas_srv: &windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView,
        instance_count: u32,
    ) {
        self.bind_common(context);
        unsafe {
            context.OMSetBlendState(&self.alpha_blend, None, u32::MAX);
            context.VSSetShader(&self.text_vs, None);
            context.PSSetShader(&self.text_ps, None);
            context.PSSetShaderResources(0, Some(&[Some(atlas_srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.DrawIndexedInstanced(6, instance_count, 0, 0, 0);
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

pub(super) fn compile_shader(source: &str, entry: &str, target: &str) -> Result<ID3DBlob> {
    let src_bytes = source.as_bytes();
    let entry_c = CString::new(entry).unwrap();
    let target_c = CString::new(target).unwrap();

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;

    // SAFETY: source bytes live for the call; entry/target C strings
    // outlive the call; blobs are owned out params.
    let hr = unsafe {
        D3DCompile(
            src_bytes.as_ptr() as *const _,
            src_bytes.len(),
            None,
            None,
            None,
            windows::core::PCSTR(entry_c.as_ptr() as *const u8),
            windows::core::PCSTR(target_c.as_ptr() as *const u8),
            D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_ENABLE_STRICTNESS,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };

    if hr.is_err() {
        let err_str = errors
            .as_ref()
            .map(|b| {
                // SAFETY: GetBufferPointer returns a NUL-terminated
                // ASCII error string.
                unsafe {
                    let ptr = b.GetBufferPointer() as *const u8;
                    let len = b.GetBufferSize();
                    std::str::from_utf8(std::slice::from_raw_parts(ptr, len))
                        .unwrap_or("<non-utf8>")
                        .to_string()
                }
            })
            .unwrap_or_default();
        anyhow::bail!("D3DCompile({entry}) failed: {hr:?}\n{err_str}");
    }
    blob.context("D3DCompile produced no blob")
}

pub(super) fn blob_slice(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: blob outlives the slice use at the call site.
    unsafe {
        let ptr = blob.GetBufferPointer() as *const u8;
        let len = blob.GetBufferSize();
        std::slice::from_raw_parts(ptr, len)
    }
}

pub(super) fn create_vertex_shader(
    device: &ID3D11Device,
    blob: &ID3DBlob,
) -> Result<ID3D11VertexShader> {
    let mut shader = None;
    unsafe { device.CreateVertexShader(blob_slice(blob), None, Some(&mut shader)) }
        .context("CreateVertexShader failed")?;
    shader.context("CreateVertexShader produced no shader")
}

pub(super) fn create_pixel_shader(
    device: &ID3D11Device,
    blob: &ID3DBlob,
) -> Result<ID3D11PixelShader> {
    let mut shader = None;
    unsafe { device.CreatePixelShader(blob_slice(blob), None, Some(&mut shader)) }
        .context("CreatePixelShader failed")?;
    shader.context("CreatePixelShader produced no shader")
}

fn instance_elem(
    name: &CString,
    semantic_index: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    offset: u32,
) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: windows::core::PCSTR(name.as_ptr() as *const u8),
        SemanticIndex: semantic_index,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

pub(super) fn create_no_cull_rasterizer(device: &ID3D11Device) -> Result<ID3D11RasterizerState> {
    let desc = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: false.into(),
        MultisampleEnable: false.into(),
        AntialiasedLineEnable: false.into(),
    };
    let mut out: Option<ID3D11RasterizerState> = None;
    // SAFETY: desc is stack-local; single out param.
    unsafe { device.CreateRasterizerState(&desc, Some(&mut out)) }
        .context("CreateRasterizerState failed")?;
    out.context("CreateRasterizerState produced no state")
}

pub(super) fn create_premultiplied_alpha_blend(device: &ID3D11Device) -> Result<ID3D11BlendState> {
    let target = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0] = target;
    let mut state = None;
    unsafe { device.CreateBlendState(&desc, Some(&mut state)) }
        .context("CreateBlendState failed")?;
    state.context("CreateBlendState produced no state")
}

fn create_point_sampler(device: &ID3D11Device) -> Result<ID3D11SamplerState> {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_SAMPLER_DESC,
        D3D11_TEXTURE_ADDRESS_CLAMP,
    };
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: 0.0,
    };
    let mut out: Option<ID3D11SamplerState> = None;
    // SAFETY: desc stack-local.
    unsafe { device.CreateSamplerState(&desc, Some(&mut out)) }
        .context("CreateSamplerState failed")?;
    out.context("CreateSamplerState produced no sampler")
}

fn create_immutable_buffer<T: Copy>(
    device: &ID3D11Device,
    data: &[T],
    bind_flags: u32,
) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: (data.len() * std::mem::size_of::<T>()) as u32,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: bind_flags,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let subres = D3D11_SUBRESOURCE_DATA {
        pSysMem: data.as_ptr() as *const _,
        SysMemPitch: 0,
        SysMemSlicePitch: 0,
    };
    let mut out: Option<ID3D11Buffer> = None;
    // SAFETY: data and descs live for the call.
    unsafe { device.CreateBuffer(&desc, Some(&subres), Some(&mut out)) }
        .context("CreateBuffer (immutable) failed")?;
    out.context("CreateBuffer (immutable) produced no buffer")
}

fn create_dynamic_instance_buffer(device: &ID3D11Device, capacity: u32) -> Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: capacity * std::mem::size_of::<Instance>() as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut out: Option<ID3D11Buffer> = None;
    // SAFETY: desc stack-local.
    unsafe { device.CreateBuffer(&desc, None, Some(&mut out)) }
        .context("CreateBuffer (dynamic instance) failed")?;
    out.context("CreateBuffer (dynamic instance) produced no buffer")
}

fn create_dynamic_cbuffer(device: &ID3D11Device, bytes: u32) -> Result<ID3D11Buffer> {
    // Constant buffers must be 16-byte aligned.
    let aligned = (bytes + 15) & !15;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: aligned,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut out: Option<ID3D11Buffer> = None;
    // SAFETY: desc stack-local.
    unsafe { device.CreateBuffer(&desc, None, Some(&mut out)) }
        .context("CreateBuffer (dynamic cbuffer) failed")?;
    out.context("CreateBuffer (dynamic cbuffer) produced no buffer")
}

// Expose the GlyphRect -> Instance helpers next to the Instance struct.
pub fn instance_for_cell(
    col: u16,
    row: u16,
    glyph: GlyphRect,
    fg: u32,
    bg: u32,
    attrs: u32,
) -> Instance {
    Instance {
        cell_pos: [col as u32, row as u32],
        atlas_pos: [glyph.x as u32, glyph.y as u32],
        atlas_size: [glyph.w as u32, glyph.h as u32],
        fg,
        bg,
        attrs,
    }
}
