//! Everything gpui-fast adds to the Metal renderer and text system lives under
//! this module.
//!
//! The rest of this crate is kept as close to upstream GPUI (Zed's
//! `crates/gpui_apple`) as possible, so that pulling in upstream changes stays
//! a matter of merging rather than of rewriting. Upstream files only *call
//! into* this module: a field holding this module's state, a line forwarding a
//! call to it. The logic itself — data structures, algorithms, bookkeeping,
//! tests — is written here, one file per topic.

pub(crate) mod backdrop;
pub(crate) mod edge_fade;
pub mod font_weight;
#[cfg(test)]
mod glyph_blur;
pub(crate) mod layers;
pub(crate) mod occlusion;
pub(crate) mod paths;
pub(crate) mod surfaces;
#[cfg(feature = "font-kit")]
pub(crate) mod text_system;
pub mod video_layer;

/// A private BGRA render target the shaders may also read, as the tests and measurements draw
/// frames into.
#[cfg(test)]
pub(crate) fn render_target(
    device: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>,
    width: usize,
    height: usize,
) -> objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>> {
    use objc2_metal::{
        MTLDevice as _, MTLPixelFormat, MTLStorageMode, MTLTextureDescriptor, MTLTextureUsage,
    };
    let descriptor = MTLTextureDescriptor::new();
    // SAFETY: a 2D texture's width and height, which Metal checks against the device's limits.
    unsafe {
        descriptor.setWidth(width);
        descriptor.setHeight(height);
    }
    descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    descriptor.setStorageMode(MTLStorageMode::Private);
    device
        .newTextureWithDescriptor(&descriptor)
        .expect("a render target")
}
