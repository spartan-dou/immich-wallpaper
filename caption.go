package main

import (
	"image"
	"image/color"
	"image/draw"
	"math"
	"os"
	"strings"
	"time"

	xdraw "golang.org/x/image/draw"
	"golang.org/x/image/font"
	"golang.org/x/image/font/gofont/gomedium"
	"golang.org/x/image/font/gofont/goregular"
	"golang.org/x/image/font/opentype"
	"golang.org/x/image/math/fixed"
	"golang.org/x/image/vector"
)

// Fonts : titre (semi-gras) et sous-titre (normal).
type Fonts struct{ Title, Body *opentype.Font }

func parseFirst(paths []string, fallback []byte) *opentype.Font {
	for _, p := range paths {
		if b, err := os.ReadFile(p); err == nil {
			if f, err := opentype.Parse(b); err == nil {
				return f
			}
		}
	}
	f, _ := opentype.Parse(fallback)
	return f
}

func loadFonts() *Fonts {
	return &Fonts{
		Title: parseFirst([]string{`C:\Windows\Fonts\seguisb.ttf`, `C:\Windows\Fonts\segoeui.ttf`}, gomedium.TTF),
		Body:  parseFirst([]string{`C:\Windows\Fonts\segoeui.ttf`}, goregular.TTF),
	}
}

var moisFR = []string{"janvier", "février", "mars", "avril", "mai", "juin", "juillet",
	"août", "septembre", "octobre", "novembre", "décembre"}

// captionParts renvoie le lieu (« Ville, Région, Pays ») et la date (« mars 2025 »).
func captionParts(info AssetInfo, withPlace, withDate bool) (place, date string) {
	if withPlace {
		var parts []string
		seen := map[string]bool{}
		for _, p := range []string{info.Exif.City, info.Exif.State, info.Exif.Country} {
			p = strings.TrimSpace(p)
			if p != "" && !seen[strings.ToLower(p)] {
				seen[strings.ToLower(p)] = true
				parts = append(parts, p)
			}
		}
		place = strings.Join(parts, ", ")
	}
	if withDate {
		ds := info.LocalDateTime
		if ds == "" {
			ds = info.Exif.DateTimeOriginal
		}
		if t, err := time.Parse(time.RFC3339, ds); err == nil {
			date = moisFR[t.Month()-1] + " " + t.Format("2006")
		}
	}
	return
}

// ---------- géométrie : rectangle arrondi anti-aliasé ----------

type rrect struct{ x0, y0, x1, y1, r float64 }

// coverage renvoie la part du pixel (x, y) couverte par le rectangle arrondi (0..1).
func (rr rrect) coverage(x, y int) float64 {
	px, py := float64(x)+0.5, float64(y)+0.5
	qx := math.Max(math.Max(rr.x0+rr.r-px, px-(rr.x1-rr.r)), 0)
	qy := math.Max(math.Max(rr.y0+rr.r-py, py-(rr.y1-rr.r)), 0)
	var d float64
	if qx > 0 && qy > 0 {
		d = math.Hypot(qx, qy) - rr.r
	} else {
		d = -math.Min(math.Min(px-rr.x0, rr.x1-px), math.Min(py-rr.y0, rr.y1-py))
	}
	return math.Min(math.Max(0.5-d, 0), 1)
}

func (rr rrect) inset(d float64) rrect {
	return rrect{rr.x0 + d, rr.y0 + d, rr.x1 - d, rr.y1 - d, math.Max(rr.r-d, 0)}
}

func (rr rrect) bounds(pad int) image.Rectangle {
	return image.Rect(int(rr.x0)-pad, int(rr.y0)-pad, int(math.Ceil(rr.x1))+pad, int(math.Ceil(rr.y1))+pad)
}

// ---------- flou ----------

// boxBlurRGBA : flou boîte séparable, en place.
func boxBlurRGBA(img *image.RGBA, r int) {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	if r < 1 || w == 0 || h == 0 {
		return
	}
	tmp := make([]int, 4*max(w, h))
	pass := func(n int, at func(i int) int) {
		var s [4]int
		cnt := 0
		for i := 0; i < n; i++ {
			o := at(i)
			for c := 0; c < 4; c++ {
				tmp[4*i+c] = int(img.Pix[o+c])
			}
		}
		for i := -r; i <= r; i++ {
			j := min(max(i, 0), n-1)
			for c := 0; c < 4; c++ {
				s[c] += tmp[4*j+c]
			}
			cnt++
		}
		for i := 0; i < n; i++ {
			o := at(i)
			for c := 0; c < 4; c++ {
				img.Pix[o+c] = uint8(s[c] / cnt)
			}
			out, in := min(max(i-r, 0), n-1), min(max(i+r+1, 0), n-1)
			for c := 0; c < 4; c++ {
				s[c] += tmp[4*in+c] - tmp[4*out+c]
			}
		}
	}
	for y := 0; y < h; y++ {
		pass(w, func(i int) int { return y*img.Stride + 4*i })
	}
	for x := 0; x < w; x++ {
		pass(h, func(i int) int { return i*img.Stride + 4*x })
	}
}

// frosted renvoie une version très floutée de la zone r de src (réduction → flou → agrandissement).
func frosted(src *image.RGBA, r image.Rectangle, factor, radius int) *image.RGBA {
	r = r.Intersect(src.Bounds())
	small := image.NewRGBA(image.Rect(0, 0, max(1, r.Dx()/factor), max(1, r.Dy()/factor)))
	xdraw.ApproxBiLinear.Scale(small, small.Bounds(), src, r, draw.Src, nil)
	for i := 0; i < 3; i++ {
		boxBlurRGBA(small, radius)
	}
	out := image.NewRGBA(r)
	xdraw.BiLinear.Scale(out, r, small, small.Bounds(), draw.Src, nil)
	return out
}

// blend mélange col (avec opacité a) dans dst au pixel (x, y).
func blend(dst *image.RGBA, x, y int, col color.NRGBA, a float64) {
	if a <= 0 || !(image.Point{x, y}.In(dst.Bounds())) {
		return
	}
	a *= float64(col.A) / 255
	o := dst.PixOffset(x, y)
	for c, v := range [3]uint8{col.R, col.G, col.B} {
		dst.Pix[o+c] = uint8(float64(dst.Pix[o+c])*(1-a) + float64(v)*a + 0.5)
	}
}

// ---------- icône lieu ----------

// pinIcon dessine un repère de carte (goutte avec un trou) de hauteur h, coin haut-gauche (x, y).
func pinIcon(dst *image.RGBA, x, y, h float64, col color.NRGBA) {
	w := h * 0.78
	rad := w / 2
	cx, cy := x+w/2, y+rad
	k := 0.5523 * rad

	ox, oy := math.Floor(x)-1, math.Floor(y)-1
	z := vector.NewRasterizer(int(math.Ceil(w))+3, int(math.Ceil(h))+3)
	f := func(v, o float64) float32 { return float32(v - o) }
	move := func(px, py float64) { z.MoveTo(f(px, ox), f(py, oy)) }
	cube := func(ax, ay, bx, by, ex, ey float64) {
		z.CubeTo(f(ax, ox), f(ay, oy), f(bx, ox), f(by, oy), f(ex, ox), f(ey, oy))
	}

	// Contour : arc supérieur + pointe en bas.
	tipY := y + h
	move(cx, tipY)
	cube(cx-rad*0.35, tipY-h*0.22, cx-rad, cy+rad*0.55, cx-rad, cy)
	cube(cx-rad, cy-k, cx-k, cy-rad, cx, cy-rad)
	cube(cx+k, cy-rad, cx+rad, cy-k, cx+rad, cy)
	cube(cx+rad, cy+rad*0.55, cx+rad*0.35, tipY-h*0.22, cx, tipY)
	z.ClosePath()
	// Trou central, tracé en sens inverse → évidé.
	hr := rad * 0.38
	hk := 0.5523 * hr
	move(cx+hr, cy)
	cube(cx+hr, cy-hk, cx+hk, cy-hr, cx, cy-hr)
	cube(cx-hk, cy-hr, cx-hr, cy-hk, cx-hr, cy)
	cube(cx-hr, cy+hk, cx-hk, cy+hr, cx, cy+hr)
	cube(cx+hk, cy+hr, cx+hr, cy+hk, cx+hr, cy)
	z.ClosePath()

	mask := image.NewAlpha(z.Bounds())
	z.Draw(mask, mask.Bounds(), image.Opaque, image.Point{})
	r := mask.Bounds().Add(image.Pt(int(ox), int(oy)))
	draw.DrawMask(dst, r, image.NewUniform(col), image.Point{}, mask, image.Point{}, draw.Over)
}

// ---------- carte ----------

// drawCaption dessine une carte « verre dépoli » (fond flouté, coins arrondis, voile sombre,
// liseré clair, ombre douce) contenant le lieu (avec icône) et la date.
func drawCaption(dst *image.RGBA, fonts *Fonts, place, date, pos string, bottomInset int) {
	if fonts == nil || (place == "" && date == "") {
		return
	}
	W, H := dst.Bounds().Dx(), dst.Bounds().Dy()
	u := float64(H) / 1080 // unité : 1 px à 1080p

	titleSize, bodySize := 21*u, 16*u
	newFace := func(f *opentype.Font, size float64) font.Face {
		fc, _ := opentype.NewFace(f, &opentype.FaceOptions{Size: size, DPI: 72, Hinting: font.HintingFull})
		return fc
	}
	title, sub := place, date
	if title == "" { // date seule
		title, sub = date, ""
	}
	tFace := newFace(fonts.Title, titleSize)
	defer tFace.Close()
	bFace := newFace(fonts.Body, bodySize)
	defer bFace.Close()

	padX, padY := 20*u, 13*u
	iconH := 0.0
	if place != "" {
		iconH = titleSize * 1.05
	}
	iconGap := 0.0
	if iconH > 0 {
		iconGap = 11 * u
	}
	tm := tFace.Metrics()
	bm := bFace.Metrics()
	lineGap := 3 * u
	tw := float64(font.MeasureString(tFace, title).Ceil())
	bw := 0.0
	textH := float64((tm.Ascent + tm.Descent).Ceil())
	if sub != "" {
		bw = float64(font.MeasureString(bFace, sub).Ceil())
		textH += lineGap + float64((bm.Ascent + bm.Descent).Ceil())
	}
	cw := padX*2 + iconH*0.78 + iconGap + math.Max(tw, bw)
	ch := padY*2 + textH

	margin := 28 * u
	x0 := margin
	if strings.HasSuffix(pos, "right") {
		x0 = float64(W) - margin - cw
	}
	y0 := margin
	if strings.HasPrefix(pos, "bottom") {
		y0 = float64(H-bottomInset) - margin - ch
	}
	radius := math.Min(16*u, ch/2)
	card := rrect{x0, y0, x0 + cw, y0 + ch, radius}

	// 1. Ombre douce sous la carte.
	sh := int(18 * u)
	sb := card.bounds(sh * 2).Add(image.Pt(0, int(6*u)))
	shadow := image.NewRGBA(sb)
	sc := rrect{card.x0, card.y0 + 6*u, card.x1, card.y1 + 6*u, card.r}
	for y := sb.Min.Y; y < sb.Max.Y; y++ {
		for x := sb.Min.X; x < sb.Max.X; x++ {
			shadow.Pix[shadow.PixOffset(x, y)+3] = uint8(255 * sc.coverage(x, y))
		}
	}
	for i := 0; i < 3; i++ {
		boxBlurRGBA(shadow, max(1, sh/3))
	}
	for y := sb.Min.Y; y < sb.Max.Y; y++ {
		for x := sb.Min.X; x < sb.Max.X; x++ {
			a := float64(shadow.Pix[shadow.PixOffset(x, y)+3]) / 255
			blend(dst, x, y, color.NRGBA{0, 0, 0, 255}, a*0.30*(1-card.coverage(x, y)))
		}
	}

	// 2. Verre dépoli : fond flouté + voile sombre, découpé en rectangle arrondi.
	cb := card.bounds(1)
	blurred := frosted(dst, cb.Inset(-int(24*u)), max(4, int(6*u)), 3)
	for y := cb.Min.Y; y < cb.Max.Y; y++ {
		for x := cb.Min.X; x < cb.Max.X; x++ {
			a := card.coverage(x, y)
			if a <= 0 || !(image.Point{x, y}.In(blurred.Bounds())) {
				continue
			}
			o := blurred.PixOffset(x, y)
			glass := color.NRGBA{
				uint8(float64(blurred.Pix[o])*0.62 + 12),
				uint8(float64(blurred.Pix[o+1])*0.62 + 12),
				uint8(float64(blurred.Pix[o+2])*0.62 + 14),
				255,
			}
			blend(dst, x, y, glass, a)
		}
	}

	// 3. Liseré clair d'un pixel (effet verre).
	inner := card.inset(math.Max(1, u))
	for y := cb.Min.Y; y < cb.Max.Y; y++ {
		for x := cb.Min.X; x < cb.Max.X; x++ {
			ring := card.coverage(x, y) - inner.coverage(x, y)
			blend(dst, x, y, color.NRGBA{255, 255, 255, 255}, ring*0.22)
		}
	}

	// 4. Contenu.
	white := color.NRGBA{255, 255, 255, 255}
	tx := x0 + padX
	ty := y0 + padY
	if iconH > 0 {
		iy := ty + (float64((tm.Ascent+tm.Descent).Ceil())-iconH)/2
		pinIcon(dst, tx, iy, iconH, color.NRGBA{255, 255, 255, 235})
		tx += iconH*0.78 + iconGap
	}
	d := font.Drawer{Dst: dst, Src: image.NewUniform(white), Face: tFace,
		Dot: fixed.P(int(tx), int(ty)+tm.Ascent.Ceil())}
	d.DrawString(title)
	if sub != "" {
		by := ty + float64((tm.Ascent + tm.Descent).Ceil()) + lineGap + float64(bm.Ascent.Ceil())
		d = font.Drawer{Dst: dst, Src: image.NewUniform(color.NRGBA{255, 255, 255, 190}), Face: bFace,
			Dot: fixed.P(int(tx), int(by))}
		d.DrawString(sub)
	}
}
