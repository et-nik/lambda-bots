//! Textures and lightmap pages as PNG: the browser decodes them and keeps them in its cache.

use crate::miptex::Image;

fn encode(w: u32, h: u32, color: png::ColorType, palette: Option<(Vec<u8>, Option<Vec<u8>>)>, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    if let Some((plte, trns)) = palette {
        enc.set_palette(plte);
        if let Some(trns) = trns {
            enc.set_trns(trns);
        }
    }
    let mut writer = enc.write_header().expect("writing a PNG header to memory");
    writer.write_image_data(data).expect("image data of the declared size");
    writer.finish().expect("finishing a PNG in memory");
    out
}

/// A palette image; with `alpha` the palette's last color is see-through, as on `{` textures.
pub fn indexed(w: u32, h: u32, image: &Image, alpha: bool) -> Vec<u8> {
    let plte: Vec<u8> = image.palette.iter().flatten().copied().collect();
    let trns = alpha.then(|| {
        let mut t = vec![255u8; 256];
        t[255] = 0;
        t
    });
    encode(w, h, png::ColorType::Indexed, Some((plte, trns)), &image.indices)
}

pub fn rgb(w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    encode(w, h, png::ColorType::Rgb, None, data)
}
