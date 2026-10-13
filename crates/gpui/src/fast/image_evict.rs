//! Freeing an image's sprite atlas tiles with its asset.
//!
//! [`ImageSource::remove_asset`] drops the asset cache's entry for an image
//! source, but the decoded [`RenderImage`] lives inside that entry, and its
//! tiles in each window's sprite atlas stay resident: nothing outside the
//! cache can reach the image to hand it to [`App::drop_image`]. An
//! application showing many images from bytes (`ImageSource::Image`) grows
//! its atlases with every image it ever showed, and a large image takes a
//! texture of its own. [`ImageSource::evict`] removes the entry and drops
//! the tiles; [`App::peek_asset`] reads a finished load without starting
//! one.
//!
//! Ported from zui (<https://github.com/zeronsh/zui>), which carries it
//! from wingleeio/zed `b687328` ("gpui: ImageSource::evict +
//! App::peek_asset — free atlas tiles for asset-cached images"; Apache-2.0,
//! by Wing; zui is a fork of Zed's GPUI). Changed from zui: `peek_asset`
//! reads GPUI's cached load as it is kept now (`CachedLoad`).

use crate::{
    App, Asset, AssetLogger, CachedLoad, ImageDecoder, ImageSource, ImgResourceLoader, RenderImage,
    Window,
};
use std::{any::TypeId, sync::Arc};

impl App {
    /// The result of loading the asset `source` if the load has finished,
    /// without starting one: `None` if it is not cached or still loading.
    pub fn peek_asset<A: Asset>(&self, source: &A::Source) -> Option<A::Output> {
        let asset_id = (TypeId::of::<A>(), crate::hash(source));
        self.loading_assets
            .get(&asset_id)?
            .downcast_ref::<CachedLoad<A::Output>>()?
            .get()
    }
}

impl ImageSource {
    /// Removes this image source from the asset cache, as
    /// [`Self::remove_asset`] does, and frees its decoded image's sprite
    /// atlas tiles in every window. Pass the window being updated, if any:
    /// it is out of [`App`]'s windows while it is updated and would keep its
    /// tiles otherwise.
    pub fn evict(&self, window: Option<&mut Window>, cx: &mut App) {
        let render_image: Option<Arc<RenderImage>> = match self {
            ImageSource::Resource(resource) => cx
                .peek_asset::<ImgResourceLoader>(resource)
                .and_then(Result::ok),
            ImageSource::Image(data) => cx
                .peek_asset::<AssetLogger<ImageDecoder>>(data)
                .and_then(Result::ok),
            ImageSource::Render(data) => Some(data.clone()),
            ImageSource::Custom(_) => None,
        };
        self.remove_asset(cx);
        if let Some(image) = render_image {
            cx.drop_image(image, window);
        }
    }
}
