//! Recadrage à la taille de l'écran (en reproduisant les styles de Windows) et encodage JPEG.

use crate::util::{Error, Res};
use fast_image_resize as fr;
use image::RgbImage;

/// Zone à couvrir (en pixels physiques) et hauteur de la barre des tâches.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Screen {
    pub w: i32,
    pub h: i32,
    pub bottom_inset: i32,
}

/// Côté maximal des photos gardées (cache hors ligne, photos affichées).
pub const KEPT_MAX_SIDE: u32 = 3840;

/// Redimensionne la zone `crop` (x, y, largeur, hauteur ; toute l'image si None) de src en dw × dh.
pub fn resize(src: &RgbImage, crop: Option<(f64, f64, f64, f64)>, dw: u32, dh: u32, filter: fr::FilterType) -> RgbImage {
    let (dw, dh) = (dw.max(1), dh.max(1));
    let mut dst = fr::images::Image::new(dw, dh, fr::PixelType::U8x3);
    let mut opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(filter));
    if let Some((x, y, w, h)) = crop {
        opts = opts.crop(x, y, w, h);
    }
    let ok = fr::images::ImageRef::new(src.width(), src.height(), src.as_raw(), fr::PixelType::U8x3)
        .map_err(|e| e.to_string())
        .and_then(|view| fr::Resizer::new().resize(&view, &mut dst, &opts).map_err(|e| e.to_string()));
    if let Err(e) = ok {
        crate::logf!("redimensionnement : {e}");
    }
    RgbImage::from_raw(dw, dh, dst.into_vec()).unwrap_or_else(|| RgbImage::new(dw, dh))
}

/// Copie src dans dst avec son coin haut-gauche en (ox, oy) ; ce qui dépasse est rogné.
pub fn blit(dst: &mut RgbImage, src: &RgbImage, ox: i32, oy: i32) {
    let (dw, dh, sw, sh) = (dst.width() as i32, dst.height() as i32, src.width() as i32, src.height() as i32);
    let x0 = ox.max(0);
    let x1 = (ox + sw).min(dw);
    if x1 <= x0 {
        return;
    }
    let n = ((x1 - x0) * 3) as usize;
    let (dstride, sstride) = (dw as usize * 3, sw as usize * 3);
    let sraw: &[u8] = src;
    let draw: &mut [u8] = dst;
    for y in oy.max(0)..(oy + sh).min(dh) {
        let d = y as usize * dstride + x0 as usize * 3;
        let s = (y - oy) as usize * sstride + (x0 - ox) as usize * 3;
        draw[d..d + n].copy_from_slice(&sraw[s..s + n]);
    }
}

/// Redimensionne l'image à la taille de l'écran en reproduisant le style Windows,
/// pour que la légende incrustée ne soit jamais rognée. Le fond est noir.
pub fn compose(src: &RgbImage, sc: Screen, style: &str) -> RgbImage {
    let (dw, dh) = (sc.w.max(1) as u32, sc.h.max(1) as u32);
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let (w, h) = (dw as f64, dh as f64);
    let filter = fr::FilterType::CatmullRom;
    match style {
        "fit" => {
            let s = (w / sw).min(h / sh);
            let (fw, fh) = ((sw * s + 0.5) as u32, (sh * s + 0.5) as u32);
            let scaled = resize(src, None, fw.max(1), fh.max(1), filter);
            let mut dst = RgbImage::new(dw, dh);
            blit(&mut dst, &scaled, (sc.w - fw as i32) / 2, (sc.h - fh as i32) / 2);
            dst
        }
        "stretch" => resize(src, None, dw, dh, filter),
        // Taille d'origine, centrée (rognée si plus grande que l'écran).
        "center" => {
            let mut dst = RgbImage::new(dw, dh);
            blit(&mut dst, src, (sc.w - src.width() as i32) / 2, (sc.h - src.height() as i32) / 2);
            dst
        }
        // Taille d'origine, répétée depuis le coin supérieur gauche.
        "tile" => {
            let mut dst = RgbImage::new(dw, dh);
            let (tw, th) = (src.width().max(1) as usize, src.height().max(1) as usize);
            for y in (0..dh as usize).step_by(th) {
                for x in (0..dw as usize).step_by(tw) {
                    blit(&mut dst, src, x as i32, y as i32);
                }
            }
            dst
        }
        // fill ; span : même recadrage, l'écran passé est alors le bureau virtuel.
        _ => {
            let s = (w / sw).max(h / sh);
            let (cw, ch) = ((w / s).min(sw), (h / s).min(sh));
            resize(src, Some(((sw - cw) / 2.0, (sh - ch) / 2.0, cw, ch)), dw, dh, filter)
        }
    }
}

pub fn encode_jpeg(img: &RgbImage) -> Res<Vec<u8>> {
    let mut buf = Vec::with_capacity((img.width() as usize * img.height() as usize) / 4);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95)
        .encode_image(img)
        .map_err(|e| Error::Msg(format!("encodage JPEG : {e}")))?;
    Ok(buf)
}

/// Encode l'image en JPEG, redimensionnée à 3840 px max.
pub fn shrink_jpeg(img: &RgbImage) -> Res<Vec<u8>> {
    let side = img.width().max(img.height());
    if side > KEPT_MAX_SIDE {
        let s = KEPT_MAX_SIDE as f64 / side as f64;
        let small = resize(
            img,
            None,
            (img.width() as f64 * s) as u32,
            (img.height() as f64 * s) as u32,
            fr::FilterType::Bilinear,
        );
        return encode_jpeg(&small);
    }
    encode_jpeg(img)
}

pub fn decode_jpeg(data: &[u8]) -> Res<RgbImage> {
    image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
        .map(|d| d.into_rgb8())
        .map_err(|e| Error::Msg(format!("JPEG illisible : {e}")))
}

/// Dimensions d'un JPEG (lecture de l'en-tête seulement).
pub fn jpeg_dims(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Jpeg)
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: image::Rgb<u8> = image::Rgb([255, 255, 255]);
    const BLACK: image::Rgb<u8> = image::Rgb([0, 0, 0]);

    /// Le recadrage maison reproduit chaque style.
    #[test]
    fn styles() {
        let src = RgbImage::from_pixel(100, 50, WHITE);
        let sc = Screen {
            w: 400,
            h: 300,
            bottom_inset: 0,
        };
        for style in ["fill", "fit", "stretch", "center", "tile", "span"] {
            let dst = compose(&src, sc, style);
            assert_eq!((dst.width(), dst.height()), (400, 300), "{style}");
            assert_eq!(*dst.get_pixel(200, 150), WHITE, "{style}");
            let corner = *dst.get_pixel(2, 2);
            match style {
                "fit" | "center" => assert_eq!(corner, BLACK, "{style} : bandes noires attendues"),
                _ => assert_eq!(corner, WHITE, "{style} : l'image doit couvrir tout l'écran"),
            }
        }
        // Centrer : l'image garde sa taille d'origine (100 × 50 au milieu de 400 × 300).
        let c = compose(&src, sc, "center");
        assert_eq!(*c.get_pixel(151, 126), WHITE);
        assert_eq!(*c.get_pixel(148, 150), BLACK);
        assert_eq!(*c.get_pixel(248, 173), WHITE);
        assert_eq!(*c.get_pixel(200, 176), BLACK);
        // Centrer une image plus grande que l'écran : rognée, sans débordement.
        let big = RgbImage::from_pixel(900, 700, WHITE);
        assert_eq!(*compose(&big, sc, "center").get_pixel(0, 0), WHITE);
    }

    #[test]
    fn jpeg_round_trip() {
        let mut img = RgbImage::from_pixel(5000, 100, image::Rgb([10, 200, 30]));
        img.put_pixel(0, 0, WHITE);
        let small = shrink_jpeg(&img).unwrap();
        assert_eq!(jpeg_dims(&small), Some((3840, 76)));
        let back = decode_jpeg(&small).unwrap();
        let px = back.get_pixel(2000, 40).0;
        assert!((px[1] as i32 - 200).abs() < 8, "{px:?}");
        let same = shrink_jpeg(&RgbImage::new(300, 200)).unwrap();
        assert_eq!(jpeg_dims(&same), Some((300, 200)));
        assert!(decode_jpeg(b"non").is_err() && jpeg_dims(b"non").is_none());
    }
}
