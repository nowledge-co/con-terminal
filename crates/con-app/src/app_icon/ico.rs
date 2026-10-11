/// Generate the executable's multi-resolution icon from the same PNG used
/// by the picker and other platform packages.
pub fn encode(png: &[u8]) -> image::ImageResult<Vec<u8>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    let frames = [16, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let resized =
                image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3);
            IcoFrame::as_png(
                resized.as_raw(),
                size,
                size,
                image::ExtendedColorType::Rgba8,
            )
        })
        .collect::<image::ImageResult<Vec<_>>>()?;
    let mut bytes = Vec::new();
    IcoEncoder::new(&mut bytes).encode_images(&frames)?;
    Ok(bytes)
}
