//! Landscape material: blends the landscape's paint layers by their weightmaps (UE3
//! LandscapeLayerBlend / TerrainLayerWeight) on top of Bevy's PBR.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub type LandscapeMaterial = ExtendedMaterial<StandardMaterial, LandscapeLayers>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct LandscapeLayers {
    /// UV scale per layer (uv = landscape quad coordinates).
    #[uniform(100)]
    pub tiling: Vec4,
    #[texture(101)]
    #[sampler(102)]
    pub layer0: Handle<Image>,
    #[texture(103)]
    #[sampler(104)]
    pub layer1: Handle<Image>,
    #[texture(105)]
    #[sampler(106)]
    pub layer2: Handle<Image>,
    #[texture(107)]
    #[sampler(108)]
    pub layer3: Handle<Image>,
}

impl MaterialExtension for LandscapeLayers {
    fn fragment_shader() -> ShaderRef {
        "embedded://goat/landscape.wgsl".into()
    }
    fn deferred_fragment_shader() -> ShaderRef {
        "embedded://goat/landscape.wgsl".into()
    }
}

pub struct LandscapePlugin;

impl Plugin for LandscapePlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "landscape.wgsl");
        app.add_plugins(MaterialPlugin::<LandscapeMaterial>::default());
    }
}
