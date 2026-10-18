use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::Cursor;

pub struct Prepared {
    pub original: Vec<u8>,
    pub mime: &'static str,
    pub extension: &'static str,
    pub width: u32,
    pub height: u32,
    pub display: Vec<u8>,
    pub display_width: u32,
    pub display_height: u32,
}
pub fn prepare(bytes: Vec<u8>) -> Result<Prepared> {
    if bytes.len() > 8 * 1024 * 1024 {
        bail!("image exceeds 8 MiB");
    }
    let (image, mime, extension, width, height) = match image::guess_format(&bytes) {
        Ok(format) => {
            let (mime, ext) = match format {
                ImageFormat::Png => ("image/png", "png"),
                ImageFormat::Jpeg => ("image/jpeg", "jpg"),
                ImageFormat::WebP => ("image/webp", "webp"),
                ImageFormat::Ico => ("image/vnd.microsoft.icon", "ico"),
                ImageFormat::Gif => ("image/gif", "gif"),
                _ => bail!("unsupported raster image type"),
            };
            let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().context("invalid or oversized image")?;
            let (w, h) = (image.width(), image.height());
            (image, mime, ext, w, h)
        }
        Err(_) => {
            let xml = std::str::from_utf8(&bytes).context("unrecognized image bytes")?;
            let document = resvg::usvg::roxmltree::Document::parse(xml).context("invalid SVG")?;
            let root = document.root_element();
            if root.tag_name().name() != "svg"
                || root.tag_name().namespace() != Some("http://www.w3.org/2000/svg")
            {
                bail!("not an SVG image");
            }
            // Disable all image resolvers, including file and data references.
            // No system fonts or local files are read by the SVG renderer.
            let options = resvg::usvg::Options {
                image_href_resolver: resvg::usvg::ImageHrefResolver {
                    resolve_data: Box::new(|_, _, _| None),
                    resolve_string: Box::new(|_, _| None),
                },
                ..Default::default()
            };
            let tree = resvg::usvg::Tree::from_str(xml, &options)?;
            let size = tree.size();
            let w = size.width().ceil() as u32;
            let h = size.height().ceil() as u32;
            if w == 0 || h == 0 || w > 8192 || h > 8192 {
                bail!("invalid SVG dimensions");
            }
            // SVG is vector: render directly at display resolution.
            let scale = 256.0 / size.width().max(size.height());
            let dw = (size.width() * scale).round().max(1.0) as u32;
            let dh = (size.height() * scale).round().max(1.0) as u32;
            let mut pixmap =
                resvg::tiny_skia::Pixmap::new(dw, dh).context("cannot allocate SVG bitmap")?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
            // tiny-skia stores premultiplied RGBA. WebP needs straight alpha.
            let pixels: Vec<u8> = pixmap
                .pixels()
                .iter()
                .flat_map(|p| {
                    let p = p.demultiply();
                    [p.red(), p.green(), p.blue(), p.alpha()]
                })
                .collect();
            let image = DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(dw, dh, pixels).context("invalid SVG bitmap")?,
            );
            (image, "image/svg+xml", "svg", w, h)
        }
    };
    let image = if image.width().max(image.height()) > 256 {
        image.resize(256, 256, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let (display_width, display_height) = rgba.dimensions();
    let mut display = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut display).encode(
        &rgba,
        display_width,
        display_height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(Prepared {
        original: bytes,
        mime,
        extension,
        width,
        height,
        display,
        display_width,
        display_height,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn png(w: u32, h: u32) -> Vec<u8> {
        let im = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([1, 2, 3, 90]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        im.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    #[test]
    fn detects_bytes_preserves_alpha_and_aspect_never_upscales() {
        for (w, h, expected) in [(800, 400, (256, 128)), (16, 16, (16, 16))] {
            let p = prepare(png(w, h)).unwrap();
            assert_eq!(p.mime, "image/png");
            assert_eq!((p.width, p.height), (w, h));
            assert_eq!((p.display_width, p.display_height), expected);
            let decoded = image::load_from_memory(&p.display).unwrap().to_rgba8();
            assert_eq!(decoded.get_pixel(0, 0)[3], 90);
        }
        assert!(prepare(b"<html>not an image</html>".to_vec()).is_err());
    }
    #[test]
    fn vector_rendering_uses_straight_alpha() {
        let p=prepare(br#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="256"><rect width="512" height="256" fill="red" opacity="0.5"/></svg>"#.to_vec()).unwrap();
        assert_eq!((p.display_width, p.display_height), (256, 128));
        assert_eq!(p.mime, "image/svg+xml");
        let image = image::load_from_memory(&p.display).unwrap().to_rgba8();
        let pixel = image.get_pixel(20, 20);
        assert_eq!(pixel[0], 255);
        assert!((127..=128).contains(&pixel[3]));
    }
}
