//! Légende : carte « verre dépoli » (fond flouté, coins arrondis, voile sombre, liseré clair,
//! ombre douce) contenant le lieu (avec icône) et la date.

use crate::immich::AssetInfo;
use crate::render::resize;
use ab_glyph::{Font, FontArc, ScaleFont};
use fast_image_resize::FilterType;
use image::RgbImage;

/// Titre (semi-gras) et sous-titre (normal).
pub struct Fonts {
    pub title: FontArc,
    pub body: FontArc,
}

fn parse_first(paths: &[std::path::PathBuf]) -> Option<FontArc> {
    paths.iter().find_map(|p| FontArc::try_from_vec(std::fs::read(p).ok()?).ok())
}

/// Charge les polices de Windows (Segoe UI). None si aucune n'est lisible : pas de légende.
pub fn load_fonts() -> Option<Fonts> {
    let win = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let dir = std::path::Path::new(&win).join("Fonts");
    let mut title = vec![dir.join("seguisb.ttf"), dir.join("segoeui.ttf"), dir.join("arial.ttf")];
    let mut body = vec![dir.join("segoeui.ttf"), dir.join("arial.ttf")];
    // Hors Windows (développement, tests).
    for p in [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ] {
        title.push(p.into());
        body.push(p.into());
    }
    Some(Fonts {
        title: parse_first(&title)?,
        body: parse_first(&body)?,
    })
}

const MOIS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// Extrait (année, mois) d'une date ISO 8601 du type 2025-03-14T10:00:00.000Z.
fn year_month(s: &str) -> Option<(u32, usize)> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let year: u32 = s.get(0..4)?.parse().ok()?;
    let month: usize = s.get(5..7)?.parse().ok()?;
    let _day: u32 = s.get(8..10)?.parse().ok()?;
    (1..=12).contains(&month).then_some((year, month))
}

/// Renvoie le lieu (« Ville, Région, Pays ») et la date (« mars 2025 »).
pub fn caption_parts(info: &AssetInfo, with_place: bool, with_date: bool) -> (String, String) {
    let mut place = String::new();
    if with_place {
        let mut parts: Vec<&str> = vec![];
        for p in [&info.exif.city, &info.exif.state, &info.exif.country] {
            let p = p.trim();
            if !p.is_empty() && !parts.iter().any(|x| x.to_lowercase() == p.to_lowercase()) {
                parts.push(p);
            }
        }
        place = parts.join(", ");
    }
    let mut date = String::new();
    if with_date {
        let ds = if info.local_date_time.is_empty() {
            &info.exif.date_time_original
        } else {
            &info.local_date_time
        };
        if let Some((y, m)) = year_month(ds) {
            date = format!("{} {y:04}", MOIS_FR[m - 1]);
        }
    }
    (place, date)
}

// ---------- géométrie : rectangle arrondi anti-aliasé ----------

#[derive(Clone, Copy)]
struct RRect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    r: f64,
}

/// Rectangle entier [x0, x1) × [y0, y1).
#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Rect {
    fn w(&self) -> i32 {
        (self.x1 - self.x0).max(0)
    }
    fn h(&self) -> i32 {
        (self.y1 - self.y0).max(0)
    }
    fn inset(&self, d: i32) -> Rect {
        Rect {
            x0: self.x0 + d,
            y0: self.y0 + d,
            x1: self.x1 - d,
            y1: self.y1 - d,
        }
    }
    fn intersect(&self, w: i32, h: i32) -> Rect {
        Rect {
            x0: self.x0.max(0),
            y0: self.y0.max(0),
            x1: self.x1.min(w),
            y1: self.y1.min(h),
        }
    }
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

impl RRect {
    /// Part du pixel (x, y) couverte par le rectangle arrondi (0..1).
    fn coverage(&self, x: i32, y: i32) -> f64 {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let qx = (self.x0 + self.r - px).max(px - (self.x1 - self.r)).max(0.0);
        let qy = (self.y0 + self.r - py).max(py - (self.y1 - self.r)).max(0.0);
        let d = if qx > 0.0 && qy > 0.0 {
            qx.hypot(qy) - self.r
        } else {
            -(px - self.x0).min(self.x1 - px).min((py - self.y0).min(self.y1 - py))
        };
        (0.5 - d).clamp(0.0, 1.0)
    }

    fn inset(&self, d: f64) -> RRect {
        RRect {
            x0: self.x0 + d,
            y0: self.y0 + d,
            x1: self.x1 - d,
            y1: self.y1 - d,
            r: (self.r - d).max(0.0),
        }
    }

    fn bounds(&self, pad: i32) -> Rect {
        Rect {
            x0: self.x0 as i32 - pad,
            y0: self.y0 as i32 - pad,
            x1: self.x1.ceil() as i32 + pad,
            y1: self.y1.ceil() as i32 + pad,
        }
    }
}

// ---------- flou ----------

/// Flou boîte séparable, en place, sur un tampon de `ch` canaux par pixel.
fn box_blur(pix: &mut [u8], w: usize, h: usize, ch: usize, r: usize) {
    if r < 1 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0i32; ch * w.max(h)];
    let mut pass = |n: usize, start: usize, step: usize| {
        let mut s = [0i32; 4];
        for i in 0..n {
            let o = start + i * step;
            for c in 0..ch {
                tmp[ch * i + c] = pix[o + c] as i32;
            }
        }
        let mut cnt = 0;
        for i in -(r as isize)..=(r as isize) {
            let j = i.clamp(0, n as isize - 1) as usize;
            for c in 0..ch {
                s[c] += tmp[ch * j + c];
            }
            cnt += 1;
        }
        for i in 0..n {
            let o = start + i * step;
            for c in 0..ch {
                pix[o + c] = (s[c] / cnt) as u8;
            }
            let out = (i as isize - r as isize).clamp(0, n as isize - 1) as usize;
            let inn = (i as isize + r as isize + 1).clamp(0, n as isize - 1) as usize;
            for c in 0..ch {
                s[c] += tmp[ch * inn + c] - tmp[ch * out + c];
            }
        }
    };
    for y in 0..h {
        pass(w, y * w * ch, ch);
    }
    for x in 0..w {
        pass(h, x * ch, w * ch);
    }
}

/// Version très floutée de la zone r de src (réduction → flou → agrandissement), à la taille de r.
fn frosted(src: &RgbImage, r: Rect, factor: i32, radius: usize) -> RgbImage {
    let (sw, sh) = ((r.w() / factor).max(1) as u32, (r.h() / factor).max(1) as u32);
    let mut small = resize(
        src,
        Some((r.x0 as f64, r.y0 as f64, r.w() as f64, r.h() as f64)),
        sw,
        sh,
        FilterType::Bilinear,
    );
    for _ in 0..3 {
        box_blur(&mut small, sw as usize, sh as usize, 3, radius);
    }
    resize(&small, None, r.w() as u32, r.h() as u32, FilterType::Bilinear)
}

/// Mélange col (avec opacité a) dans dst au pixel (x, y).
fn blend(dst: &mut RgbImage, x: i32, y: i32, col: [u8; 3], a: f64) {
    if a <= 0.0 || x < 0 || y < 0 || x >= dst.width() as i32 || y >= dst.height() as i32 {
        return;
    }
    let a = a.min(1.0);
    let px = dst.get_pixel_mut(x as u32, y as u32);
    for c in 0..3 {
        px.0[c] = (px.0[c] as f64 * (1.0 - a) + col[c] as f64 * a + 0.5) as u8;
    }
}

// ---------- icône lieu ----------

/// Dessine un repère de carte (goutte avec un trou) de hauteur h, coin haut-gauche (x, y).
fn pin_icon(dst: &mut RgbImage, x: f64, y: f64, h: f64, alpha: f64) {
    use ab_glyph_rasterizer::{point, Rasterizer};
    let w = h * 0.78;
    let rad = w / 2.0;
    let (cx, cy) = (x + w / 2.0, y + rad);
    let k = 0.5523 * rad;
    let (ox, oy) = (x.floor() - 1.0, y.floor() - 1.0);
    let mut z = Rasterizer::new(w.ceil() as usize + 3, h.ceil() as usize + 3);
    let pt = |px: f64, py: f64| point((px - ox) as f32, (py - oy) as f32);

    // Chaque contour : point de départ puis courbes de Bézier cubiques (deux contrôles, arrivée).
    let mut path = |start: (f64, f64), cubes: &[[(f64, f64); 3]]| {
        let mut cur = start;
        for c in cubes {
            z.draw_cubic(pt(cur.0, cur.1), pt(c[0].0, c[0].1), pt(c[1].0, c[1].1), pt(c[2].0, c[2].1));
            cur = c[2];
        }
        if cur != start {
            z.draw_line(pt(cur.0, cur.1), pt(start.0, start.1));
        }
    };
    // Contour : arc supérieur + pointe en bas.
    let tip = y + h;
    path(
        (cx, tip),
        &[
            [(cx - rad * 0.35, tip - h * 0.22), (cx - rad, cy + rad * 0.55), (cx - rad, cy)],
            [(cx - rad, cy - k), (cx - k, cy - rad), (cx, cy - rad)],
            [(cx + k, cy - rad), (cx + rad, cy - k), (cx + rad, cy)],
            [(cx + rad, cy + rad * 0.55), (cx + rad * 0.35, tip - h * 0.22), (cx, tip)],
        ],
    );
    // Trou central, tracé en sens inverse → évidé.
    let hr = rad * 0.38;
    let hk = 0.5523 * hr;
    path(
        (cx + hr, cy),
        &[
            [(cx + hr, cy - hk), (cx + hk, cy - hr), (cx, cy - hr)],
            [(cx - hk, cy - hr), (cx - hr, cy - hk), (cx - hr, cy)],
            [(cx - hr, cy + hk), (cx - hk, cy + hr), (cx, cy + hr)],
            [(cx + hk, cy + hr), (cx + hr, cy + hk), (cx + hr, cy)],
        ],
    );
    z.for_each_pixel_2d(|px, py, a| {
        blend(dst, ox as i32 + px as i32, oy as i32 + py as i32, [255, 255, 255], a as f64 * alpha);
    });
}

// ---------- texte ----------

/// Police mise à l'échelle pour une taille de corps donnée (en pixels par cadratin).
fn scaled(font: &FontArc, size: f64) -> ab_glyph::PxScaleFont<&FontArc> {
    let upem = font.units_per_em().unwrap_or(1000.0);
    font.as_scaled(size as f32 * font.height_unscaled() / upem)
}

fn text_width(f: &ab_glyph::PxScaleFont<&FontArc>, s: &str) -> f64 {
    let mut w = 0.0f32;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w as f64
}

/// Dessine s avec sa ligne de base en (x, baseline).
fn draw_text(dst: &mut RgbImage, f: &ab_glyph::PxScaleFont<&FontArc>, s: &str, x: f64, baseline: f64, alpha: f64) {
    let mut pen = x as f32;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            pen += f.kern(p, id);
        }
        let glyph = id.with_scale_and_position(f.scale(), ab_glyph::point(pen, baseline as f32));
        if let Some(o) = f.outline_glyph(glyph) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                blend(
                    dst,
                    b.min.x as i32 + gx as i32,
                    b.min.y as i32 + gy as i32,
                    [255, 255, 255],
                    cov as f64 * alpha,
                );
            });
        }
        pen += f.h_advance(id);
        prev = Some(id);
    }
}

// ---------- carte ----------

/// Dessine la légende sur dst. `pos` : bottom-right | bottom-left | top-right | top-left.
pub fn draw_caption(dst: &mut RgbImage, fonts: Option<&Fonts>, place: &str, date: &str, pos: &str, bottom_inset: i32) {
    let fonts = match fonts {
        Some(f) if !(place.is_empty() && date.is_empty()) => f,
        _ => return,
    };
    let (w, h) = (dst.width() as i32, dst.height() as i32);
    let u = h as f64 / 1080.0; // unité : 1 px à 1080p

    let (title_size, body_size) = (21.0 * u, 16.0 * u);
    let (title, sub) = if place.is_empty() { (date, "") } else { (place, date) }; // date seule → en titre
    let tf = scaled(&fonts.title, title_size);
    let bf = scaled(&fonts.body, body_size);

    let (pad_x, pad_y) = (20.0 * u, 13.0 * u);
    let icon_h = if place.is_empty() { 0.0 } else { title_size * 1.05 };
    let icon_gap = if icon_h > 0.0 { 11.0 * u } else { 0.0 };
    let t_ascent = (tf.ascent() as f64).ceil();
    let t_line = (tf.ascent() as f64 - tf.descent() as f64).ceil();
    let b_ascent = (bf.ascent() as f64).ceil();
    let b_line = (bf.ascent() as f64 - bf.descent() as f64).ceil();
    let line_gap = 3.0 * u;
    let tw = text_width(&tf, title).ceil();
    let mut bw = 0.0;
    let mut text_h = t_line;
    if !sub.is_empty() {
        bw = text_width(&bf, sub).ceil();
        text_h += line_gap + b_line;
    }
    let cw = pad_x * 2.0 + icon_h * 0.78 + icon_gap + tw.max(bw);
    let ch = pad_y * 2.0 + text_h;

    let margin = 28.0 * u;
    let x0 = if pos.ends_with("right") { w as f64 - margin - cw } else { margin };
    let y0 = if pos.starts_with("bottom") {
        (h - bottom_inset) as f64 - margin - ch
    } else {
        margin
    };
    let card = RRect {
        x0,
        y0,
        x1: x0 + cw,
        y1: y0 + ch,
        r: (16.0 * u).min(ch / 2.0),
    };

    // 1. Ombre douce sous la carte.
    let sh = (18.0 * u) as i32;
    let drop = (6.0 * u) as i32;
    let b = card.bounds(sh * 2);
    let sb = Rect {
        x0: b.x0,
        y0: b.y0 + drop,
        x1: b.x1,
        y1: b.y1 + drop,
    };
    let sc = RRect {
        y0: card.y0 + 6.0 * u,
        y1: card.y1 + 6.0 * u,
        ..card
    };
    let (sw, shh) = (sb.w() as usize, sb.h() as usize);
    let mut shadow = vec![0u8; sw * shh];
    for y in 0..shh {
        for x in 0..sw {
            shadow[y * sw + x] = (255.0 * sc.coverage(sb.x0 + x as i32, sb.y0 + y as i32)) as u8;
        }
    }
    for _ in 0..3 {
        box_blur(&mut shadow, sw, shh, 1, (sh / 3).max(1) as usize);
    }
    for y in 0..shh {
        for x in 0..sw {
            let (px, py) = (sb.x0 + x as i32, sb.y0 + y as i32);
            let a = shadow[y * sw + x] as f64 / 255.0;
            blend(dst, px, py, [0, 0, 0], a * 0.30 * (1.0 - card.coverage(px, py)));
        }
    }

    // 2. Verre dépoli : fond flouté + voile sombre, découpé en rectangle arrondi.
    let cb = card.bounds(1);
    let br = cb.inset(-((24.0 * u) as i32)).intersect(w, h);
    if br.w() > 0 && br.h() > 0 {
        let blurred = frosted(dst, br, ((6.0 * u) as i32).max(4), 3);
        for y in cb.y0..cb.y1 {
            for x in cb.x0..cb.x1 {
                let a = card.coverage(x, y);
                if a <= 0.0 || !br.contains(x, y) {
                    continue;
                }
                let p = blurred.get_pixel((x - br.x0) as u32, (y - br.y0) as u32).0;
                let glass = [
                    (p[0] as f64 * 0.62 + 12.0) as u8,
                    (p[1] as f64 * 0.62 + 12.0) as u8,
                    (p[2] as f64 * 0.62 + 14.0) as u8,
                ];
                blend(dst, x, y, glass, a);
            }
        }
    }

    // 3. Liseré clair d'un pixel (effet verre).
    let inner = card.inset(u.max(1.0));
    for y in cb.y0..cb.y1 {
        for x in cb.x0..cb.x1 {
            let ring = card.coverage(x, y) - inner.coverage(x, y);
            blend(dst, x, y, [255, 255, 255], ring * 0.22);
        }
    }

    // 4. Contenu.
    let mut tx = x0 + pad_x;
    let ty = y0 + pad_y;
    if icon_h > 0.0 {
        pin_icon(dst, tx, ty + (t_line - icon_h) / 2.0, icon_h, 235.0 / 255.0);
        tx += icon_h * 0.78 + icon_gap;
    }
    draw_text(dst, &tf, title, tx.floor(), ty.floor() + t_ascent, 1.0);
    if !sub.is_empty() {
        let by = ty + t_line + line_gap + b_ascent;
        draw_text(dst, &bf, sub, tx.floor(), by.floor(), 190.0 / 255.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_text() {
        let mut i = AssetInfo::default();
        i.exif.city = "Paris".into();
        i.exif.state = "Paris".into();
        i.exif.country = "France".into();
        i.local_date_time = "2024-08-02T18:00:00.000Z".into();
        assert_eq!(
            caption_parts(&i, true, true),
            ("Paris, France".to_string(), "août 2024".to_string())
        );
        assert_eq!(caption_parts(&AssetInfo::default(), true, false), (String::new(), String::new()));
        i.local_date_time.clear();
        i.exif.date_time_original = "2025-03-14T10:00:00+02:00".into();
        assert_eq!(caption_parts(&i, false, true).1, "mars 2025");
        i.exif.date_time_original = "hier".into();
        assert_eq!(caption_parts(&i, false, true).1, "");
    }

    #[test]
    fn blur_keeps_uniform_area() {
        let mut p = vec![100u8; 20 * 10 * 3];
        box_blur(&mut p, 20, 10, 3, 4);
        assert!(p.iter().all(|v| *v == 100));
        let mut a = vec![0u8; 9 * 9];
        a[4 * 9 + 4] = 255;
        box_blur(&mut a, 9, 9, 1, 2);
        assert!(a[4 * 9 + 4] < 255 && a[4 * 9 + 5] > 0 && a[0] == 0);
    }

    /// La carte modifie l'image dans le coin demandé, et seulement là.
    #[test]
    fn card_is_drawn_in_the_requested_corner() {
        let Some(fonts) = load_fonts() else {
            eprintln!("aucune police disponible : test ignoré");
            return;
        };
        let base = RgbImage::from_pixel(1920, 1080, image::Rgb([90, 120, 160]));
        for (pos, cx, cy) in [("bottom-right", 1800, 950), ("top-left", 120, 70)] {
            let mut img = base.clone();
            draw_caption(&mut img, Some(&fonts), "Rovaniemi, Laponie, Finlande", "mars 2025", pos, 48);
            let changed =
                |x0: u32, y0: u32, x1: u32, y1: u32| (y0..y1).any(|y| (x0..x1).any(|x| img.get_pixel(x, y) != base.get_pixel(x, y)));
            assert!(changed(cx - 60, cy - 30, cx + 60, cy + 30), "{pos} : carte absente");
            assert!(!changed(900, 500, 1000, 600), "{pos} : le centre de l'image ne doit pas changer");
            // Du texte blanc est présent.
            assert!(
                img.pixels().any(|p| p.0[0] > 235 && p.0[1] > 235 && p.0[2] > 235),
                "{pos} : texte absent"
            );
        }
        // Sans lieu ni date : rien n'est dessiné.
        let mut img = base.clone();
        draw_caption(&mut img, Some(&fonts), "", "", "bottom-right", 0);
        assert!(img == base);
    }
}
