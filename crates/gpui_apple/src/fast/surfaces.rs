//! The video surfaces a frame samples, released on the render thread once the GPU is done.
//!
//! CoreVideo may hand a `CVMetalTexture`'s backing buffer to a new picture once the texture is
//! released, so each frame's textures are held until its command buffer completes. They are
//! released here, on the thread that renders, never in Metal's completion handler. Releasing a
//! texture tells the texture cache it came from that its backing is free
//! (`CVMetalTextureCache::bufferBackingNotInUse`). The cache is not safe to touch from Metal's
//! thread while the render thread flushes it, and it is gone once the renderer is dropped with a
//! frame still on the GPU: either crashed in that call.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2_core_foundation::CFRetained;
use objc2_core_video::CVMetalTexture;

/// Each frame's surface textures, until the GPU finished with that frame.
#[derive(Default)]
pub(crate) struct SurfacesInFlight {
    frames: VecDeque<(Arc<FrameDone>, Vec<CFRetained<CVMetalTexture>>)>,
}

/// Set by a frame's completion handler: its textures may go.
#[derive(Default)]
pub(crate) struct FrameDone(AtomicBool);

impl FrameDone {
    /// The GPU is done with the frame. Safe on any thread: it touches no CoreVideo object.
    pub(crate) fn set(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl SurfacesInFlight {
    /// Hold `textures` for the frame being committed, until the flag handed back is set; no
    /// flag for a frame that samples none.
    pub(crate) fn hold(
        &mut self,
        textures: Vec<CFRetained<CVMetalTexture>>,
    ) -> Option<Arc<FrameDone>> {
        if textures.is_empty() {
            return None;
        }
        let done = Arc::new(FrameDone::default());
        self.frames.push_back((done.clone(), textures));
        Some(done)
    }

    /// Release the textures of every frame the GPU has finished with.
    pub(crate) fn release_finished(&mut self) {
        self.frames
            .retain(|(done, _)| !done.0.load(Ordering::Acquire));
    }

    /// How many frames still hold textures.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.frames.len()
    }
}
