//! Untrusted SVG, parsed without what it points at: a diagram an agent wrote
//! (onicode renders Mermaid this way) must not read a file or a nested
//! document through an `<image>`.
//!
//! Ported from monocode (hardbeat920/monocode, its patched GPUI's
//! `SvgRenderer::parse_svg_without_images`).

use crate::{ParsedSvg, SvgRenderer};

impl SvgRenderer {
    /// Parses generated, untrusted SVG as [`SvgRenderer::parse_svg`] does,
    /// without resolving any image reference: neither a path or URL nor a
    /// data URL, a nested SVG's included, is followed, and an `<image>` is
    /// left out. Text is still set in this renderer's installed and bundled
    /// fonts.
    pub fn parse_svg_without_images(&self, bytes: &[u8]) -> Result<ParsedSvg, usvg::Error> {
        let base = &self.usvg_options;
        let options = usvg::Options {
            resources_dir: None,
            dpi: base.dpi,
            font_family: base.font_family.clone(),
            font_size: base.font_size,
            languages: base.languages.clone(),
            shape_rendering: base.shape_rendering,
            text_rendering: base.text_rendering,
            image_rendering: base.image_rendering,
            default_size: base.default_size,
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_data: Box::new(|_, _, _| None),
                resolve_string: Box::new(|_, _| None),
            },
            font_resolver: usvg::FontResolver {
                select_font: Box::new(|font, db| (base.font_resolver.select_font)(font, db)),
                select_fallback: Box::new(|ch, fonts, db| {
                    (base.font_resolver.select_fallback)(ch, fonts, db)
                }),
            },
            fontdb: base.fontdb.clone(),
            style_sheet: base.style_sheet.clone(),
        };
        usvg::Tree::from_data(bytes, &options).map(ParsedSvg)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::SvgRenderer;

    /// A 4 × 4 red square as SVG, base64 for a data URL.
    const NESTED: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0IiBoZWlnaHQ9IjQiPjxyZWN0IHdpZHRoPSI0IiBoZWlnaHQ9IjQiIGZpbGw9InJlZCIvPjwvc3ZnPg==";
    /// A 1 × 1 red PNG, base64 for a data URL.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

    fn images(group: &usvg::Group) -> usize {
        group
            .children()
            .iter()
            .map(|node| match node {
                usvg::Node::Image(_) => 1,
                usvg::Node::Group(group) => images(group),
                _ => 0,
            })
            .sum()
    }

    fn document(href: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><rect width="8" height="4" fill="blue"/><image width="8" height="8" href="{href}"/></svg>"#
        )
    }

    /// Parsed as trusted, each data URL becomes an image; parsed without
    /// images none does, and a path is not read, while the rest of the
    /// document stays.
    #[test]
    fn untrusted_svg_resolves_no_image() {
        let renderer = SvgRenderer::new(Arc::new(()));
        for href in [
            format!("data:image/svg+xml;base64,{NESTED}"),
            format!("data:image/png;base64,{PNG}"),
        ] {
            let svg = document(&href);
            let trusted = renderer.parse_svg(svg.as_bytes()).expect("parsed");
            assert_eq!(images(trusted.0.root()), 1, "{href}");
            let untrusted = renderer
                .parse_svg_without_images(svg.as_bytes())
                .expect("parsed");
            assert_eq!(images(untrusted.0.root()), 0, "{href}");
            assert!(untrusted.0.root().has_children(), "the rectangle stays");
        }
        let path = document("/etc/hosts");
        let untrusted = renderer
            .parse_svg_without_images(path.as_bytes())
            .expect("parsed");
        assert_eq!(images(untrusted.0.root()), 0);
    }
}
