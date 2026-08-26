//! D3D11 Kitty image uploads and layered placement draws.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER,
    D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_CPU_ACCESS_WRITE,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_INSTANCE_DATA,
    D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DYNAMIC, D3D11_USAGE_IMMUTABLE,
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader,
    ID3D11RasterizerState, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};

use super::pipeline::{
    blob_slice, compile_shader, create_no_cull_rasterizer, create_pixel_shader,
    create_premultiplied_alpha_blend, create_vertex_shader,
};
use crate::vt::{KittyImage, KittyPlacement};

const HLSL_SOURCE: &str = include_str!("image_shaders.hlsl");
const INITIAL_INSTANCE_CAPACITY: u32 = 64;
const BELOW_BACKGROUND_LIMIT: i32 = i32::MIN / 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLayer {
    BelowBackground,
    BelowText,
    AboveText,
}

impl ImageLayer {
    fn contains(self, z: i32) -> bool {
        match self {
            Self::BelowBackground => z < BELOW_BACKGROUND_LIMIT,
            Self::BelowText => (BELOW_BACKGROUND_LIMIT..0).contains(&z),
            Self::AboveText => z >= 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImageKey {
    id: u32,
    generation: u64,
}

impl From<&KittyImage> for ImageKey {
    fn from(image: &KittyImage) -> Self {
        Self {
            id: image.id,
            generation: image.generation,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ImageInstance {
    destination: [f32; 4],
    source: [f32; 4],
    source_clamp: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ImageGlobals {
    inv_viewport: [f32; 2],
    padding: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
struct PreparedDraw {
    key: ImageKey,
    z: i32,
}

pub struct ImagePipeline {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    sampler: ID3D11SamplerState,
    blend: windows::Win32::Graphics::Direct3D11::ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    instance_buffer: ID3D11Buffer,
    instance_capacity: u32,
    globals_buffer: ID3D11Buffer,
    textures: HashMap<ImageKey, ID3D11ShaderResourceView>,
    failed_uploads: HashSet<ImageKey>,
    instances: Vec<ImageInstance>,
    draws: Vec<PreparedDraw>,
}

impl ImagePipeline {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let vs_blob = compile_shader(HLSL_SOURCE, "vs_main", "vs_5_0")?;
        let ps_blob = compile_shader(HLSL_SOURCE, "ps_main", "ps_5_0")?;
        let vs = create_vertex_shader(device, &vs_blob)?;
        let ps = create_pixel_shader(device, &ps_blob)?;

        let destination = c"DESTINATION";
        let source = c"SOURCE";
        let source_clamp = c"SOURCECLAMP";
        let layout = [
            input_element(destination, DXGI_FORMAT_R32G32B32A32_FLOAT, 0),
            input_element(source, DXGI_FORMAT_R32G32B32A32_FLOAT, 16),
            input_element(source_clamp, DXGI_FORMAT_R32G32B32A32_FLOAT, 32),
        ];
        let mut input_layout = None;
        unsafe { device.CreateInputLayout(&layout, blob_slice(&vs_blob), Some(&mut input_layout)) }
            .context("CreateInputLayout(image) failed")?;

        Ok(Self {
            vs,
            ps,
            input_layout: input_layout.context("CreateInputLayout(image) produced no layout")?,
            sampler: create_linear_sampler(device)?,
            blend: create_premultiplied_alpha_blend(device)?,
            rasterizer: create_no_cull_rasterizer(device)?,
            instance_buffer: create_dynamic_buffer(
                device,
                INITIAL_INSTANCE_CAPACITY,
                std::mem::size_of::<ImageInstance>() as u32,
                D3D11_BIND_VERTEX_BUFFER.0 as u32,
            )?,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            globals_buffer: create_dynamic_buffer(
                device,
                1,
                std::mem::size_of::<ImageGlobals>() as u32,
                D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            )?,
            textures: HashMap::new(),
            failed_uploads: HashSet::new(),
            instances: Vec::with_capacity(INITIAL_INSTANCE_CAPACITY as usize),
            draws: Vec::with_capacity(INITIAL_INSTANCE_CAPACITY as usize),
        })
    }

    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        placements: &[KittyPlacement],
        cell_size: [u32; 2],
        viewport_size: [u32; 2],
    ) -> Result<()> {
        let active: HashSet<ImageKey> = placements
            .iter()
            .map(|placement| ImageKey::from(placement.image.as_ref()))
            .collect();
        self.textures.retain(|key, _| active.contains(key));
        self.failed_uploads.retain(|key| active.contains(key));
        self.instances.clear();
        self.draws.clear();

        for placement in placements {
            let key = ImageKey::from(placement.image.as_ref());
            if !self.textures.contains_key(&key) && !self.failed_uploads.contains(&key) {
                match upload_texture(device, &placement.image) {
                    Ok(texture) => {
                        self.textures.insert(key, texture);
                    }
                    Err(error) => {
                        log::warn!(
                            "failed to upload Kitty image {} generation {}: {error:#}",
                            key.id,
                            key.generation
                        );
                        self.failed_uploads.insert(key);
                    }
                }
            }
            if !self.textures.contains_key(&key) {
                continue;
            }

            let Some(instance) = image_instance(placement, cell_size[0], cell_size[1]) else {
                log::warn!(
                    "ignoring invalid Kitty image placement image={} placement={}",
                    key.id,
                    placement.placement_id
                );
                continue;
            };
            self.instances.push(instance);
            self.draws.push(PreparedDraw {
                key,
                z: placement.z,
            });
        }

        self.ensure_capacity(device, self.instances.len() as u32)?;
        if !self.instances.is_empty() {
            upload_slice(context, &self.instance_buffer, &self.instances)?;
        }
        let globals = ImageGlobals {
            inv_viewport: [
                2.0 / viewport_size[0].max(1) as f32,
                -2.0 / viewport_size[1].max(1) as f32,
            ],
            padding: [0.0; 2],
        };
        upload_value(context, &self.globals_buffer, &globals)
    }

    pub fn draw_layer(&self, context: &ID3D11DeviceContext, layer: ImageLayer) {
        if self.draws.is_empty() {
            return;
        }

        unsafe {
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.IASetInputLayout(&self.input_layout);
            context.RSSetState(&self.rasterizer);
            let stride = std::mem::size_of::<ImageInstance>() as u32;
            context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(self.instance_buffer.clone())),
                Some(&stride),
                Some(&0),
            );
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.globals_buffer.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }

        let mut index = 0usize;
        while index < self.draws.len() {
            let draw = self.draws[index];
            if !layer.contains(draw.z) {
                index += 1;
                continue;
            }
            let mut end = index + 1;
            while end < self.draws.len()
                && layer.contains(self.draws[end].z)
                && self.draws[end].key == draw.key
            {
                end += 1;
            }

            let Some(texture) = self.textures.get(&draw.key) else {
                index = end;
                continue;
            };
            unsafe {
                context.PSSetShaderResources(0, Some(&[Some(texture.clone())]));
                context.DrawInstanced(6, (end - index) as u32, 0, index as u32);
            }
            index = end;
        }

        // Do not keep the last image alive through a context binding after its
        // placement disappears and the cache prunes it on the next frame.
        unsafe { context.PSSetShaderResources(0, Some(&[None])) };
    }

    fn ensure_capacity(&mut self, device: &ID3D11Device, needed: u32) -> Result<()> {
        if needed <= self.instance_capacity {
            return Ok(());
        }
        let capacity = (needed + needed / 2).max(INITIAL_INSTANCE_CAPACITY);
        self.instance_buffer = create_dynamic_buffer(
            device,
            capacity,
            std::mem::size_of::<ImageInstance>() as u32,
            D3D11_BIND_VERTEX_BUFFER.0 as u32,
        )?;
        self.instance_capacity = capacity;
        Ok(())
    }
}

fn image_instance(
    placement: &KittyPlacement,
    cell_width_px: u32,
    cell_height_px: u32,
) -> Option<ImageInstance> {
    let image = &placement.image;
    let source_right = placement.source_x.checked_add(placement.source_width)?;
    let source_bottom = placement.source_y.checked_add(placement.source_height)?;
    if placement.pixel_width == 0
        || placement.pixel_height == 0
        || placement.source_width == 0
        || placement.source_height == 0
        || source_right > image.width
        || source_bottom > image.height
        || image.width == 0
        || image.height == 0
    {
        return None;
    }

    let left =
        placement.viewport_col as f64 * cell_width_px as f64 + placement.cell_x_offset as f64;
    let top =
        placement.viewport_row as f64 * cell_height_px as f64 + placement.cell_y_offset as f64;
    let right = left + placement.pixel_width as f64;
    let bottom = top + placement.pixel_height as f64;
    if ![left, top, right, bottom]
        .iter()
        .all(|value| value.is_finite())
    {
        return None;
    }

    Some(ImageInstance {
        destination: [left as f32, top as f32, right as f32, bottom as f32],
        source: [
            placement.source_x as f32 / image.width as f32,
            placement.source_y as f32 / image.height as f32,
            source_right as f32 / image.width as f32,
            source_bottom as f32 / image.height as f32,
        ],
        source_clamp: [
            (placement.source_x as f32 + 0.5) / image.width as f32,
            (placement.source_y as f32 + 0.5) / image.height as f32,
            (source_right as f32 - 0.5) / image.width as f32,
            (source_bottom as f32 - 0.5) / image.height as f32,
        ],
    })
}

fn premultiplied_rgba(rgba: &[u8]) -> Cow<'_, [u8]> {
    let Some(first_translucent) = rgba.chunks_exact(4).position(|pixel| pixel[3] != u8::MAX) else {
        return Cow::Borrowed(rgba);
    };

    let mut premultiplied = rgba.to_vec();
    for pixel in premultiplied[first_translucent * 4..].chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Cow::Owned(premultiplied)
}

fn upload_texture(device: &ID3D11Device, image: &KittyImage) -> Result<ID3D11ShaderResourceView> {
    let expected_len = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("Kitty image dimensions overflow")?;
    if image.width == 0 || image.height == 0 || image.rgba.len() != expected_len {
        anyhow::bail!(
            "invalid RGBA dimensions {}x{} for {} bytes",
            image.width,
            image.height,
            image.rgba.len()
        );
    }
    let pitch = image
        .width
        .checked_mul(4)
        .context("Kitty image row pitch overflow")?;
    // Premultiply before the texture reaches LINEAR filtering. Multiplying in
    // the pixel shader is too late: interpolation has already mixed RGB from
    // transparent neighbours and produces dark fringes around scaled images.
    let pixels = premultiplied_rgba(&image.rgba);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: image.width,
        Height: image.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: pitch,
        SysMemSlicePitch: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture)) }
        .context("CreateTexture2D(Kitty image) failed")?;
    let texture = texture.context("CreateTexture2D(Kitty image) produced no texture")?;
    let mut view = None;
    unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view)) }
        .context("CreateShaderResourceView(Kitty image) failed")?;
    view.context("CreateShaderResourceView(Kitty image) produced no view")
}

fn input_element(
    name: &std::ffi::CStr,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    offset: u32,
) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: windows::core::PCSTR(name.as_ptr().cast()),
        SemanticIndex: 0,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

fn create_linear_sampler(device: &ID3D11Device) -> Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
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
    let mut sampler = None;
    unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }
        .context("CreateSamplerState(image) failed")?;
    sampler.context("CreateSamplerState(image) produced no sampler")
}

fn create_dynamic_buffer(
    device: &ID3D11Device,
    count: u32,
    stride: u32,
    bind_flags: u32,
) -> Result<ID3D11Buffer> {
    let byte_width = count
        .checked_mul(stride)
        .context("dynamic image buffer size overflow")?;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: (byte_width + 15) & !15,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind_flags,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }
        .context("CreateBuffer(image dynamic) failed")?;
    buffer.context("CreateBuffer(image dynamic) produced no buffer")
}

fn upload_slice<T: Copy>(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    values: &[T],
) -> Result<()> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
            .context("Map(image instances) failed")?;
        std::ptr::copy_nonoverlapping(values.as_ptr(), mapped.pData.cast::<T>(), values.len());
        context.Unmap(buffer, 0);
    }
    Ok(())
}

fn upload_value<T: Copy>(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    value: &T,
) -> Result<()> {
    upload_slice(context, buffer, std::slice::from_ref(value))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::vt::{KittyImage, KittyPlacement};

    use super::{image_instance, premultiplied_rgba};

    #[test]
    fn premultiplied_rgba_premultiplies_translucent_rgb() {
        let rgba = [
            10, 20, 30, 255, // opaque prefix remains exact
            200, 100, 50, 128, // rounded half alpha
            255, 128, 64, 0, // hidden RGB is eliminated
        ];

        assert_eq!(
            premultiplied_rgba(&rgba).as_ref(),
            &[10, 20, 30, 255, 100, 50, 25, 128, 0, 0, 0, 0]
        );
    }

    #[test]
    fn cropped_source_uvs_stay_inside_edge_texel_centres() {
        let placement = KittyPlacement {
            image: Arc::new(KittyImage {
                id: 1,
                generation: 2,
                width: 4,
                height: 4,
                rgba: vec![0; 4 * 4 * 4].into(),
            }),
            placement_id: 3,
            z: 0,
            viewport_col: 2,
            viewport_row: 1,
            cell_x_offset: 3,
            cell_y_offset: 4,
            pixel_width: 20,
            pixel_height: 10,
            source_x: 1,
            source_y: 1,
            source_width: 2,
            source_height: 1,
        };

        let instance = image_instance(&placement, 10, 20).expect("valid image placement");
        assert_eq!(instance.destination, [23.0, 24.0, 43.0, 34.0]);
        assert_eq!(instance.source, [0.25, 0.25, 0.75, 0.5]);
        assert_eq!(instance.source_clamp, [0.375, 0.375, 0.625, 0.375]);
    }
}
