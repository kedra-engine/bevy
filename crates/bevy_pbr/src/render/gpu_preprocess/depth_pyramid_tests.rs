//! The textureLoad-only occlusion pyramid must work without float32 filtering.
use super::gpu_occlusion_culling_bind_group_layout_entries;
use bevy_render::render_resource::{BindingType, TextureSampleType, TextureViewDimension};

#[test]
fn depth_pyramid_binding_accepts_unfilterable_float_textures() {
    let entries = gpu_occlusion_culling_bind_group_layout_entries();
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.binding == 11)
            .map(|entry| entry.ty),
        Some(BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: false },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        })
    );
}
