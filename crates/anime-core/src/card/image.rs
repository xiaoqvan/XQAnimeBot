//! 图片小工具：裁切压缩、data URI。
//!
//! 出图本身由 ultralightGenImage 插件的 `image-shot` 渲染器完成，这里只处理**进模板之前**的素材：
//! 海报要压到合适尺寸再转 base64，否则一张 2MB 的原图会把 HTML 撑到几十 MB。

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::imageops::FilterType;

use crate::Result;

/// 居中裁切（cover）后缩放到指定尺寸，编码成 JPEG。
pub fn cover_jpeg(bytes: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).map_err(|err| crate::Error::Parse {
        what: "图片",
        message: err.to_string(),
    })?;
    let (src_w, src_h) = (img.width().max(1), img.height().max(1));
    let target_ratio = width as f32 / height as f32;
    let src_ratio = src_w as f32 / src_h as f32;

    let (crop_w, crop_h) = if src_ratio > target_ratio {
        (((src_h as f32) * target_ratio).round() as u32, src_h)
    } else {
        (src_w, ((src_w as f32) / target_ratio).round() as u32)
    };
    let crop_w = crop_w.clamp(1, src_w);
    let crop_h = crop_h.clamp(1, src_h);

    let cropped = img.crop_imm((src_w - crop_w) / 2, (src_h - crop_h) / 2, crop_w, crop_h);
    let resized = cropped.resize_exact(width, height, FilterType::Lanczos3);

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&resized)
        .map_err(|err| crate::Error::Parse {
            what: "JPEG 编码",
            message: err.to_string(),
        })?;
    Ok(out)
}

/// 字节 → `data:` URI。模板里直接当 `src` 用，页面零外部请求。
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn cover_produces_requested_size() {
        let mut img = image::RgbImage::new(400, 100);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        let mut src = Vec::new();
        image::codecs::png::PngEncoder::new(&mut src)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();

        let jpeg = cover_jpeg(&src, 210, 290, 88).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (210, 290));
    }

    #[test]
    fn data_uri_is_prefixed() {
        assert!(data_uri("image/png", b"abc").starts_with("data:image/png;base64,"));
    }
}
