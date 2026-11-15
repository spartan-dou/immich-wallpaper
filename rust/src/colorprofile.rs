//! Lecture d'une image téléchargée : dimensions, profil ICC → sRGB, orientation EXIF.
//!
//! Immich génère par défaut ses aperçus en Display P3 (profil ICC intégré). Windows ne gère pas
//! les profils pour le fond d'écran : sans conversion, les couleurs seraient délavées.

use crate::util::{err, Error, Res};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use std::io::Cursor;

/// Au-delà, l'image est refusée avant décodage (120 Mpx en RGB = 360 Mo).
pub const MAX_PIXELS: u64 = 120_000_000;

// ---------- profil ICC matrice/courbe ----------

#[derive(Debug, Clone)]
pub enum Curve {
    Identity,
    Gamma(f64),
    Table(Vec<f64>),
    /// Courbe paramétrique : type (0..=4) et paramètres g, a, b, c, d, e, f.
    Para(u16, [f64; 7]),
}

impl Curve {
    pub fn eval(&self, x: f64) -> f64 {
        match self {
            Curve::Identity => x,
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(tab) => {
                let n = tab.len();
                let f = x * (n - 1) as f64;
                let i = if f.is_nan() || f < 0.0 { 0 } else { f as usize };
                if i >= n - 1 {
                    return tab[n - 1];
                }
                tab[i] + (tab[i + 1] - tab[i]) * (f - i as f64)
            }
            Curve::Para(ft, v) => {
                let [g, a, b, c, d, e, f] = *v;
                let pw = |x: f64| x.max(0.0).powf(g);
                match ft {
                    0 => pw(x),
                    1 => {
                        if x >= -b / a {
                            pw(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if x >= -b / a {
                            pw(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            pw(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            pw(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct IccProfile {
    /// Colonnes : XYZ (D50) des primaires R, G, B.
    pub m: [[f64; 3]; 3],
    pub trc: [Curve; 3],
}

/// XYZ D50 des primaires sRGB (adaptation Bradford), tels qu'on les trouve dans le profil sRGB standard.
pub const SRGB_D50: [[f64; 3]; 3] = [
    [0.4360747, 0.3850649, 0.1430804],
    [0.2225045, 0.7168786, 0.0606169],
    [0.0139322, 0.0971045, 0.7141733],
];

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn s15(b: &[u8]) -> f64 {
    be32(b) as i32 as f64 / 65536.0
}

/// Lit un profil RGB matrice/courbe. None pour les profils LUT (A2B0…) : l'image est laissée telle quelle.
pub fn parse_icc(b: &[u8]) -> Option<IccProfile> {
    if b.len() < 132 || &b[16..20] != b"RGB " {
        return None;
    }
    let n = be32(&b[128..]) as usize;
    let tag = |sig: &[u8; 4]| -> Option<&[u8]> {
        let mut found = None;
        for k in 0..n {
            let e = 132usize.checked_add(12usize.checked_mul(k)?)?;
            if e + 12 > b.len() {
                return None;
            }
            let (off, size) = (be32(&b[e + 4..]) as usize, be32(&b[e + 8..]) as usize);
            if off + size > b.len() || size < 8 {
                continue;
            }
            if &b[e..e + 4] == sig {
                found = Some(&b[off..off + size]); // le dernier l'emporte, comme dans la version Go
            }
        }
        found
    };
    let mut m = [[0.0; 3]; 3];
    for (c, sig) in [b"rXYZ", b"gXYZ", b"bXYZ"].into_iter().enumerate() {
        let t = tag(sig)?;
        if t.len() < 20 || &t[..4] != b"XYZ " {
            return None;
        }
        for r in 0..3 {
            m[r][c] = s15(&t[8 + 4 * r..]);
        }
    }
    let trc = [
        parse_curve(tag(b"rTRC")?)?,
        parse_curve(tag(b"gTRC")?)?,
        parse_curve(tag(b"bTRC")?)?,
    ];
    Some(IccProfile { m, trc })
}

pub fn parse_curve(t: &[u8]) -> Option<Curve> {
    if t.len() < 12 {
        return None;
    }
    match &t[..4] {
        b"curv" => {
            let n = be32(&t[8..]) as usize;
            if n == 0 {
                Some(Curve::Identity)
            } else if n == 1 && t.len() >= 14 {
                Some(Curve::Gamma(be16(&t[12..]) as f64 / 256.0))
            } else if n >= 2 && t.len() >= 12 + 2 * n {
                Some(Curve::Table((0..n).map(|i| be16(&t[12 + 2 * i..]) as f64 / 65535.0).collect()))
            } else {
                None
            }
        }
        b"para" => {
            let ft = be16(&t[8..]);
            let np = [1usize, 3, 4, 5, 7];
            if ft > 4 || t.len() < 12 + 4 * np[ft as usize] {
                return None;
            }
            let mut v = [0.0; 7];
            for (i, x) in v.iter_mut().enumerate().take(np[ft as usize]) {
                *x = s15(&t[12 + 4 * i..]);
            }
            Some(Curve::Para(ft, v))
        }
        _ => None,
    }
}

fn inv3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

fn mul3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                r[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    r
}

fn srgb_encode(l: f64) -> f64 {
    if l <= 0.0031308 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_decode(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

impl IccProfile {
    /// Le profil est (à peu près) sRGB → aucune conversion utile.
    pub fn is_srgb(&self) -> bool {
        for r in 0..3 {
            for c in 0..3 {
                if (self.m[r][c] - SRGB_D50[r][c]).abs() > 0.004 {
                    return false;
                }
            }
        }
        for c in 0..3 {
            for x in [0.02, 0.2, 0.5, 0.8] {
                if (self.trc[c].eval(x) - srgb_decode(x)).abs() > 0.01 {
                    return false;
                }
            }
        }
        true
    }
}

/// Convertit en place l'image décrite par le profil p (Display P3, Adobe RGB…) vers sRGB.
pub fn to_srgb(img: &mut RgbImage, p: &IccProfile) {
    let t = mul3(inv3(SRGB_D50), p.m);
    let mut lin = [[0.0f64; 256]; 3];
    for c in 0..3 {
        for v in 0..256 {
            lin[c][v] = p.trc[c].eval(v as f64 / 255.0);
        }
    }
    const N: usize = 4096;
    let mut enc = [0u8; N + 1];
    for (i, e) in enc.iter_mut().enumerate() {
        *e = (srgb_encode(i as f64 / N as f64) * 255.0).round() as u8;
    }
    let q = |x: f64| -> u8 {
        if !(x > 0.0) {
            0 // négatif ou NaN (profil ICC aberrant)
        } else if x >= 1.0 {
            255
        } else {
            enc[(x * N as f64 + 0.5) as usize]
        }
    };
    let convert = |pix: &mut [u8]| {
        for px in pix.chunks_exact_mut(3) {
            let (r, g, b) = (lin[0][px[0] as usize], lin[1][px[1] as usize], lin[2][px[2] as usize]);
            px[0] = q(t[0][0] * r + t[0][1] * g + t[0][2] * b);
            px[1] = q(t[1][0] * r + t[1][1] * g + t[1][2] * b);
            px[2] = q(t[2][0] * r + t[2][1] * g + t[2][2] * b);
        }
    };
    // Conversion par bandes de lignes, une par cœur.
    let (w, h) = (img.width() as usize, img.height() as usize);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(h / 64)
        .max(1);
    let rows_per = h.div_ceil(workers).max(1);
    let buf: &mut [u8] = img;
    if workers == 1 || w == 0 {
        convert(buf);
        return;
    }
    std::thread::scope(|s| {
        for band in buf.chunks_mut(rows_per * w * 3) {
            s.spawn(|| convert(band));
        }
    });
}

// ---------- lecture d'une image téléchargée ----------

/// Ce qu'on sait d'une image à partir de son seul en-tête.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meta {
    /// Dimensions affichées (orientation EXIF appliquée).
    pub w: u32,
    pub h: u32,
}

impl Meta {
    pub fn portrait(&self) -> bool {
        self.w <= self.h
    }
}

fn open(data: &[u8]) -> Res<impl ImageDecoder + '_> {
    let unsupported = |e: String| Error::Msg(format!("format non pris en charge (HEIC/RAW ? utilise image_size=fullsize) : {e}"));
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| unsupported(e.to_string()))?;
    let dec = reader.into_decoder().map_err(|e| unsupported(e.to_string()))?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS {
        return err(format!("dimensions refusées ({w} × {h}, maximum {} Mpx)", MAX_PIXELS / 1_000_000));
    }
    Ok(dec)
}

fn swaps_axes(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH
    )
}

/// Lit l'en-tête (dimensions, orientation EXIF) sans décoder les pixels.
pub fn probe(data: &[u8]) -> Res<Meta> {
    let mut dec = open(data)?;
    let (w, h) = dec.dimensions();
    let o = dec.orientation().unwrap_or(Orientation::NoTransforms);
    Ok(if swaps_axes(o) { Meta { w: h, h: w } } else { Meta { w, h } })
}

/// Décode l'image, la convertit en sRGB et la redresse selon l'EXIF.
pub fn decode(data: &[u8]) -> Res<RgbImage> {
    let mut dec = open(data)?;
    let icc = dec.icc_profile().ok().flatten();
    let o = dec.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|e| Error::Msg(e.to_string()))?.into_rgb8();
    if let Some(p) = icc.as_deref().and_then(parse_icc) {
        if !p.is_srgb() {
            to_srgb(&mut img, &p);
        }
    }
    if o != Orientation::NoTransforms {
        let mut d = DynamicImage::ImageRgb8(img);
        d.apply_orientation(o);
        img = d.into_rgb8();
    }
    Ok(img)
}

/// Décode une image ; un panic du décodeur (fichier malformé) devient une erreur.
pub fn safe_decode(data: &[u8]) -> Res<RgbImage> {
    match std::panic::catch_unwind(|| decode(data)) {
        Ok(r) => r,
        Err(p) => err(format!("décodage interrompu : {}", crate::util::panic_message(&*p))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::mk_img;

    #[test]
    fn pixel_limit() {
        // En-tête JPEG annonçant 65535 × 65535 : refusé avant toute allocation.
        let huge: Vec<u8> = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 0xFF, 0xC0, 0x00, 0x11, 8, 0xFF,
            0xFF, 0xFF, 0xFF, 3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1,
        ];
        assert!(probe(&huge).is_err());
        assert!(safe_decode(&huge).is_err());
        let m = probe(&mk_img(30, 60, "jpeg")).unwrap();
        assert_eq!((m.w, m.h), (30, 60));
        assert!(m.portrait());
        assert!(probe(b"pas une image").is_err());
        let img = decode(&mk_img(80, 40, "png")).unwrap();
        assert_eq!((img.width(), img.height()), (80, 40));
    }

    #[test]
    fn nan_profile() {
        // Profil aberrant (gamma négatif → infini/NaN) : la conversion ne doit pas paniquer.
        let nan = Curve::Para(0, [-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let p = IccProfile {
            m: SRGB_D50,
            trc: [nan.clone(), nan.clone(), nan],
        };
        let mut img = RgbImage::new(8, 200);
        to_srgb(&mut img, &p);
        assert_eq!(img.height(), 200);
    }

    /// Profil gamma 2,2 avec les primaires sRGB : valeurs proches de sRGB mais pas identiques.
    #[test]
    fn gamma_profile_converts() {
        let g = Curve::Gamma(2.2);
        let p = IccProfile {
            m: SRGB_D50,
            trc: [g.clone(), g.clone(), g],
        };
        let mut img = RgbImage::from_pixel(70, 70, image::Rgb([128, 64, 200]));
        to_srgb(&mut img, &p);
        let px = img.get_pixel(35, 35).0;
        for (got, src) in px.iter().zip([128u8, 64, 200]) {
            assert!((*got as i32 - src as i32).abs() <= 6, "{px:?}");
        }
        // Identité : sRGB décrit par une table reste inchangé (détecté comme sRGB).
        let tab = Curve::Table((0..1024).map(|i| srgb_decode(i as f64 / 1023.0)).collect());
        let srgb = IccProfile {
            m: SRGB_D50,
            trc: [tab.clone(), tab.clone(), tab],
        };
        assert!(srgb.is_srgb());
    }

    #[test]
    fn malformed_icc_never_panics() {
        let mut seed = 12345u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 16) as u8
        };
        for round in 0..3000 {
            let len = 132 + (round % 200);
            let mut b: Vec<u8> = (0..len).map(|_| next()).collect();
            b[16..20].copy_from_slice(b"RGB ");
            b[128..132].copy_from_slice(&((round % 12) as u32).to_be_bytes());
            if let Some(p) = parse_icc(&b) {
                p.is_srgb();
                let mut img = RgbImage::new(4, 4);
                to_srgb(&mut img, &p);
            }
            if let Some(c) = parse_curve(&b[..(round % 40).max(1)]) {
                c.eval(0.5);
            }
        }
        for t in [&b"para\0\0\0\0\0\0\0\0\xFF\xFF\0\0"[..], &b"curv\0\0\0\0\0\0\0\x02\0\0\xFF\xFF"[..]] {
            if let Some(c) = parse_curve(t) {
                let p = IccProfile {
                    m: SRGB_D50,
                    trc: [c.clone(), c.clone(), c],
                };
                let mut img = RgbImage::new(4, 4);
                to_srgb(&mut img, &p);
            }
        }
    }
}
