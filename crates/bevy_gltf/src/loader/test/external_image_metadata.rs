//! Exercise real native glTF and PNG loaders, not handler/cache ordering.
use super::*;
use bevy_asset::{
    meta::{AssetAction, AssetMeta, AssetMetaDyn},
    RenderAssetUsages,
};
use bevy_image::{
    ImageAddressMode, ImageFilterMode, ImageLoader, ImageSampler, ImageSamplerDescriptor,
};

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 208, 8, 168, 248, 15, 0, 3,
    132, 1, 240, 105, 106, 22, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn model(wrap: u32) -> String {
    serde_json::json!({
        "asset":{"version":"2.0"},
        "images":[{"uri":"shared.png"}],
        "textures":[{"source":0,"sampler":0}],
        "samplers":[{"wrapS":wrap,"magFilter":9729}],
        "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]
    })
    .to_string()
}

fn image_app(dir: Dir) -> App {
    let reader = MemoryAssetReader { root: dir };
    let mut app = App::new();
    app.register_asset_source(
        AssetSourceId::Default,
        AssetSourceBuilder::new(move || Box::new(reader.clone())),
    )
    .insert_resource(bevy_image::CompressedImageFormatSupport(
        bevy_image::CompressedImageFormats::NONE,
    ))
    .add_plugins((
        TaskPoolPlugin::default(),
        AssetPlugin::default(),
        WorldSerializationPlugin,
        MeshPlugin,
        crate::GltfPlugin::default(),
    ))
    .init_asset::<GltfMaterial>()
    .init_asset::<Image>()
    .register_asset_loader(ImageLoader::new(bevy_image::CompressedImageFormats::NONE));
    app.finish();
    app.cleanup();
    app
}

fn load_image(metadata: bool, reverse: bool, sidecar: bool) {
    let dir = Dir::default();
    dir.insert_asset_text(Path::new("first.gltf"), &model(10497));
    dir.insert_asset_text(Path::new("second.gltf"), &model(33648));
    dir.insert_asset(Path::new("shared.png"), PNG.to_vec());
    let meta = AssetMeta::<ImageLoader, ()>::new(AssetAction::Load {
        loader: ImageLoader::type_path().into(),
        settings: ImageLoaderSettings {
            is_srgb: false,
            sampler: ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::ClampToEdge,
                mag_filter: ImageFilterMode::Nearest,
                ..Default::default()
            }),
            asset_usage: RenderAssetUsages::MAIN_WORLD,
            ..Default::default()
        },
    });
    if sidecar {
        dir.insert_meta(Path::new("shared.png"), meta.serialize());
    }
    let mut app = image_app(dir);
    let server = app.world().resource::<AssetServer>().clone();
    let names = if reverse {
        ["second.gltf", "first.gltf"]
    } else {
        ["first.gltf", "second.gltf"]
    };
    let mut models = Vec::new();
    for name in names {
        let handle: Handle<Gltf> = server
            .load_builder()
            .with_settings(move |settings: &mut GltfLoaderSettings| {
                settings.use_external_image_metadata = metadata;
            })
            .load(name);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.update();
            if server.is_loaded_with_dependencies(&handle) {
                break;
            }
            if let LoadState::Failed(error) = server.load_state(&handle) {
                panic!("{error}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "image dependencies timed out"
            );
            std::thread::yield_now();
        }
        models.push(handle);
    }
    let gltfs = app.world().resource::<Assets<Gltf>>();
    let materials = app.world().resource::<Assets<GltfMaterial>>();
    let texture = |model: &Handle<Gltf>| {
        materials
            .get(&gltfs.get(model).unwrap().materials[0])
            .unwrap()
            .base_color_texture
            .as_ref()
            .unwrap()
    };
    assert_eq!(texture(&models[0]), texture(&models[1]));
    assert_eq!(
        texture(&models[0]).path().unwrap().to_string(),
        "shared.png"
    );
    let images = app.world().resource::<Assets<Image>>();
    let image = images.get(texture(&models[0])).unwrap();
    assert_eq!(
        image.texture_descriptor.format.is_srgb(),
        !(metadata && sidecar)
    );
    assert_eq!(
        image.asset_usage,
        if metadata && sidecar {
            RenderAssetUsages::MAIN_WORLD
        } else {
            RenderAssetUsages::default()
        }
    );
    if metadata && !sidecar {
        assert!(matches!(image.sampler, ImageSampler::Default));
        return;
    }
    let ImageSampler::Descriptor(sampler) = &image.sampler else {
        panic!("expected explicit sampler")
    };
    assert_eq!(
        sampler.address_mode_u,
        if metadata {
            ImageAddressMode::ClampToEdge
        } else if reverse {
            ImageAddressMode::MirrorRepeat
        } else {
            ImageAddressMode::Repeat
        }
    );
    assert_eq!(
        sampler.mag_filter,
        if metadata {
            ImageFilterMode::Nearest
        } else {
            ImageFilterMode::Linear
        }
    );
}

#[test]
fn metadata_owns_shared_external_image_in_both_model_orders() {
    load_image(true, false, true);
    load_image(true, true, true);
}

#[test]
fn default_preserves_gltf_image_overrides() {
    load_image(false, false, true);
    load_image(false, true, true);
}

#[test]
fn absent_metadata_uses_native_image_defaults() {
    load_image(true, false, false);
    load_image(true, true, false);
}

#[test]
fn embedded_images_keep_gltf_settings_when_external_metadata_is_enabled() {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, PNG);
    for buffer_view in [false, true] {
        let mut json: serde_json::Value = serde_json::from_str(&model(33648)).unwrap();
        json["images"] = if buffer_view {
            json["buffers"] = serde_json::json!([{"byteLength":PNG.len(),"uri":format!("data:application/octet-stream;base64,{encoded}")}]);
            json["bufferViews"] = serde_json::json!([{"buffer":0,"byteLength":PNG.len()}]);
            serde_json::json!([{"bufferView":0,"mimeType":"image/png"}])
        } else {
            serde_json::json!([{"uri":format!("data:image/png;base64,{encoded}")}])
        };
        let gltf = gltf::Gltf::from_slice(&serde_json::to_vec(&json).unwrap()).unwrap();
        for metadata in [false, true] {
            let settings = GltfLoaderSettings {
                use_external_image_metadata: metadata,
                load_materials: RenderAssetUsages::RENDER_WORLD,
                ..Default::default()
            };
            let loaded = bevy_tasks::block_on(super::super::load_image(
                gltf.document.textures().next().unwrap(),
                &[PNG.to_vec()],
                &Default::default(),
                &bevy_asset::AssetPath::from("embedded.gltf"),
                bevy_image::CompressedImageFormats::NONE,
                &ImageSamplerDescriptor::linear(),
                &settings,
            ))
            .unwrap();
            let super::super::ImageOrPath::Image { image, .. } = loaded else {
                panic!("embedded image became an external request")
            };
            assert!(image.texture_descriptor.format.is_srgb());
            assert_eq!(image.asset_usage, RenderAssetUsages::RENDER_WORLD);
            let ImageSampler::Descriptor(sampler) = image.sampler else {
                panic!("embedded sampler missing")
            };
            assert_eq!(sampler.address_mode_u, ImageAddressMode::MirrorRepeat);
            assert_eq!(sampler.mag_filter, ImageFilterMode::Linear);
        }
    }
}

#[test]
fn metadata_authority_preserves_missing_image_dependency_failure() {
    let dir = Dir::default();
    dir.insert_asset_text(Path::new("missing.gltf"), &model(10497));
    let mut app = image_app(dir);
    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<Gltf> = server
        .load_builder()
        .with_settings(|settings: &mut GltfLoaderSettings| {
            settings.use_external_image_metadata = true;
        })
        .load("missing.gltf");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.update();
        if let bevy_asset::RecursiveDependencyLoadState::Failed(_) =
            server.recursive_dependency_load_state(&handle)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing image dependency failure timed out"
        );
        std::thread::yield_now();
    }
    assert!(!server.is_loaded_with_dependencies(&handle));
    assert!(app.world().resource::<Assets<Image>>().is_empty());
}
