// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The device the engine draws with, asked for as the web asks for it
//! (`render/core/gpu-context.ts`): the optional features where the adapter
//! has them, the limits the engine leans on as high as the adapter goes,
//! and the capabilities the subsystems read to pick their variants.

/// The features the engine takes where the adapter has them: the web's
/// `OPTIONAL_FEATURES`, and ETC2, the compressed format every Android GPU
/// with Vulkan reads.
pub const OPTIONAL_FEATURES: wgpu::Features = wgpu::Features::SHADER_F16
    .union(wgpu::Features::CLIP_DISTANCES)
    .union(wgpu::Features::DEPTH32FLOAT_STENCIL8)
    .union(wgpu::Features::TIMESTAMP_QUERY)
    .union(wgpu::Features::TEXTURE_COMPRESSION_BC)
    .union(wgpu::Features::TEXTURE_COMPRESSION_ASTC)
    .union(wgpu::Features::TEXTURE_COMPRESSION_ETC2)
    .union(wgpu::Features::FLOAT32_FILTERABLE)
    .union(wgpu::Features::INDIRECT_FIRST_INSTANCE)
    .union(wgpu::Features::SUBGROUP);

/// What the device can do beyond core WebGPU, as the subsystems ask it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuCapabilities {
    pub shader_f16: bool,
    pub clip_distances: bool,
    pub depth32_stencil8: bool,
    pub timestamp_query: bool,
    pub texture_compression_bc: bool,
    pub texture_compression_astc: bool,
    pub texture_compression_etc2: bool,
    pub float32_filterable: bool,
    pub indirect_first_instance: bool,
    pub subgroups: bool,
}

impl GpuCapabilities {
    pub fn of(features: wgpu::Features) -> GpuCapabilities {
        GpuCapabilities {
            shader_f16: features.contains(wgpu::Features::SHADER_F16),
            clip_distances: features.contains(wgpu::Features::CLIP_DISTANCES),
            depth32_stencil8: features.contains(wgpu::Features::DEPTH32FLOAT_STENCIL8),
            timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            texture_compression_bc: features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            texture_compression_astc: features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC),
            texture_compression_etc2: features.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2),
            float32_filterable: features.contains(wgpu::Features::FLOAT32_FILTERABLE),
            indirect_first_instance: features.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            subgroups: features.contains(wgpu::Features::SUBGROUP),
        }
    }

    /// The depth and stencil the frame draws with: the float depth where
    /// the device has it, else `Depth24PlusStencil8`, which core WebGPU has.
    pub fn depth_stencil_format(&self) -> wgpu::TextureFormat {
        if self.depth32_stencil8 { wgpu::TextureFormat::Depth32FloatStencil8 } else { wgpu::TextureFormat::Depth24PlusStencil8 }
    }
}

/// The device the engine draws with, and what it was given.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
    pub capabilities: GpuCapabilities,
    /// What the adapter offers, for the report: the device may take less.
    adapter_features: wgpu::Features,
    adapter_limits: wgpu::Limits,
}

/// The limits the engine leans on, as high as the adapter goes, as the web
/// asks for them; the rest stay at WebGPU's defaults.
pub fn required_limits(adapter: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits {
        max_storage_buffers_per_shader_stage: adapter.max_storage_buffers_per_shader_stage,
        max_buffer_size: adapter.max_buffer_size,
        max_storage_buffer_binding_size: adapter.max_storage_buffer_binding_size,
        max_texture_array_layers: adapter.max_texture_array_layers,
        ..wgpu::Limits::default()
    }
}

impl GpuContext {
    /// A device of the adapter, with the features and limits the engine takes.
    pub async fn new(adapter: &wgpu::Adapter) -> Result<GpuContext, wgpu::RequestDeviceError> {
        let adapter_features = adapter.features();
        let adapter_limits = adapter.limits();
        let features = adapter_features & OPTIONAL_FEATURES;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor { label: Some("ImmersiveMap"), required_features: features, required_limits: required_limits(&adapter_limits), ..Default::default() }).await?;
        device.on_uncaptured_error(std::sync::Arc::new(|error| log::error!("ImmersiveMap wgpu: {error}")));
        Ok(GpuContext { capabilities: GpuCapabilities::of(device.features()), device, queue, adapter_info: adapter.get_info(), adapter_features, adapter_limits })
    }

    /// The adapter, what the device took of it and the limits that matter
    /// to the port, in lines for a log.
    pub fn report(&self) -> String {
        let info = &self.adapter_info;
        let limits = &self.adapter_limits;
        let missing = OPTIONAL_FEATURES - self.device.features();
        format!(
            "adapter: {} ({:?}, {:?}), driver {} {}, vendor {:#x} device {:#x}\n\
             subgroup size: {}..{}\n\
             features taken: {:?}\n\
             features missing: {:?}\n\
             adapter features: {:?}\n\
             limits: 2D texture {}, array layers {}, buffer {}, storage binding {}, uniform binding {}, \
             storage buffers a stage {}, sampled textures a stage {}, samplers a stage {}, bind groups {}, \
             vertex buffers {}, vertex attributes {}, colour attachments {}, workgroup invocations {}, workgroup storage {}",
            info.name,
            info.backend,
            info.device_type,
            info.driver,
            info.driver_info,
            info.vendor,
            info.device,
            info.subgroup_min_size,
            info.subgroup_max_size,
            self.device.features(),
            missing,
            self.adapter_features,
            limits.max_texture_dimension_2d,
            limits.max_texture_array_layers,
            limits.max_buffer_size,
            limits.max_storage_buffer_binding_size,
            limits.max_uniform_buffer_binding_size,
            limits.max_storage_buffers_per_shader_stage,
            limits.max_sampled_textures_per_shader_stage,
            limits.max_samplers_per_shader_stage,
            limits.max_bind_groups,
            limits.max_vertex_buffers,
            limits.max_vertex_attributes,
            limits.max_color_attachments,
            limits.max_compute_invocations_per_workgroup,
            limits.max_compute_workgroup_storage_size,
        )
    }
}
