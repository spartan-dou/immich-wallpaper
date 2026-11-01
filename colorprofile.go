package main

import (
	"bytes"
	"encoding/binary"
	"image"
	"image/draw"
	"math"
)

// ---------- lecture des métadonnées (profil ICC, orientation EXIF) ----------

// jpegMeta extrait le profil ICC (segments APP2 « ICC_PROFILE ») et l'orientation EXIF (APP1).
func jpegMeta(b []byte) (icc []byte, orientation int) {
	orientation = 1
	if len(b) < 4 || b[0] != 0xFF || b[1] != 0xD8 {
		return nil, 1
	}
	chunks := map[int][]byte{}
	i := 2
	for i+4 <= len(b) {
		if b[i] != 0xFF {
			break
		}
		marker := b[i+1]
		if marker == 0xD8 || (marker >= 0xD0 && marker <= 0xD7) || marker == 0x01 {
			i += 2
			continue
		}
		if marker == 0xDA || marker == 0xD9 { // début des données image
			break
		}
		n := int(binary.BigEndian.Uint16(b[i+2:]))
		if n < 2 || i+2+n > len(b) {
			break
		}
		seg := b[i+4 : i+2+n]
		switch {
		case marker == 0xE2 && len(seg) > 14 && string(seg[:12]) == "ICC_PROFILE\x00":
			chunks[int(seg[12])] = seg[14:]
		case marker == 0xE1 && len(seg) > 6 && string(seg[:6]) == "Exif\x00\x00":
			if o := exifOrientation(seg[6:]); o >= 1 && o <= 8 {
				orientation = o
			}
		}
		i += 2 + n
	}
	for k := 1; k <= len(chunks); k++ {
		c, ok := chunks[k]
		if !ok {
			return nil, orientation
		}
		icc = append(icc, c...)
	}
	return icc, orientation
}

func exifOrientation(t []byte) int {
	if len(t) < 8 {
		return 1
	}
	var bo binary.ByteOrder
	switch string(t[:2]) {
	case "II":
		bo = binary.LittleEndian
	case "MM":
		bo = binary.BigEndian
	default:
		return 1
	}
	off := int(bo.Uint32(t[4:]))
	if off+2 > len(t) {
		return 1
	}
	n := int(bo.Uint16(t[off:]))
	for k := 0; k < n; k++ {
		e := off + 2 + 12*k
		if e+12 > len(t) {
			break
		}
		if bo.Uint16(t[e:]) == 0x0112 {
			return int(bo.Uint16(t[e+8:]))
		}
	}
	return 1
}

// webpICC extrait le chunk ICCP d'un WebP étendu.
func webpICC(b []byte) []byte {
	if len(b) < 12 || string(b[:4]) != "RIFF" || string(b[8:12]) != "WEBP" {
		return nil
	}
	for i := 12; i+8 <= len(b); {
		id := string(b[i : i+4])
		n := int(binary.LittleEndian.Uint32(b[i+4:]))
		if i+8+n > len(b) {
			break
		}
		if id == "ICCP" {
			return b[i+8 : i+8+n]
		}
		i += 8 + n + n%2
	}
	return nil
}

// ---------- profil ICC matrice/courbe ----------

type iccProfile struct {
	m   [3][3]float64 // colonnes : XYZ (D50) des primaires R, G, B
	trc [3]func(float64) float64
}

// XYZ D50 des primaires sRGB (adaptation Bradford), tels qu'on les trouve dans le profil sRGB standard.
var srgbD50 = [3][3]float64{
	{0.4360747, 0.3850649, 0.1430804},
	{0.2225045, 0.7168786, 0.0606169},
	{0.0139322, 0.0971045, 0.7141733},
}

func s15(b []byte) float64 { return float64(int32(binary.BigEndian.Uint32(b))) / 65536 }

func parseICC(b []byte) (*iccProfile, bool) {
	if len(b) < 132 || string(b[16:20]) != "RGB " {
		return nil, false
	}
	tags := map[string][]byte{}
	n := int(binary.BigEndian.Uint32(b[128:]))
	for k := 0; k < n; k++ {
		e := 132 + 12*k
		if e+12 > len(b) {
			return nil, false
		}
		off, size := int(binary.BigEndian.Uint32(b[e+4:])), int(binary.BigEndian.Uint32(b[e+8:]))
		if off+size > len(b) || size < 8 {
			continue
		}
		tags[string(b[e:e+4])] = b[off : off+size]
	}
	p := &iccProfile{}
	for c, sig := range []string{"rXYZ", "gXYZ", "bXYZ"} {
		t := tags[sig]
		if len(t) < 20 || string(t[:4]) != "XYZ " {
			return nil, false // profil LUT (A2B0…) non géré : on laisse tel quel
		}
		for r := 0; r < 3; r++ {
			p.m[r][c] = s15(t[8+4*r:])
		}
	}
	for c, sig := range []string{"rTRC", "gTRC", "bTRC"} {
		f, ok := parseCurve(tags[sig])
		if !ok {
			return nil, false
		}
		p.trc[c] = f
	}
	return p, true
}

func parseCurve(t []byte) (func(float64) float64, bool) {
	if len(t) < 12 {
		return nil, false
	}
	switch string(t[:4]) {
	case "curv":
		n := int(binary.BigEndian.Uint32(t[8:]))
		switch {
		case n == 0:
			return func(x float64) float64 { return x }, true
		case n == 1 && len(t) >= 14:
			g := float64(binary.BigEndian.Uint16(t[12:])) / 256
			return func(x float64) float64 { return math.Pow(x, g) }, true
		case len(t) >= 12+2*n:
			tab := make([]float64, n)
			for i := range tab {
				tab[i] = float64(binary.BigEndian.Uint16(t[12+2*i:])) / 65535
			}
			return func(x float64) float64 {
				f := x * float64(n-1)
				i := int(f)
				if i >= n-1 {
					return tab[n-1]
				}
				return tab[i] + (tab[i+1]-tab[i])*(f-float64(i))
			}, true
		}
	case "para":
		ft := int(binary.BigEndian.Uint16(t[8:]))
		np := []int{1, 3, 4, 5, 7}
		if ft > 4 || len(t) < 12+4*np[ft] {
			return nil, false
		}
		var v [7]float64
		for i := 0; i < np[ft]; i++ {
			v[i] = s15(t[12+4*i:])
		}
		g, a, bb, c, d, e, f := v[0], v[1], v[2], v[3], v[4], v[5], v[6]
		pw := func(x float64) float64 { return math.Pow(math.Max(x, 0), g) }
		switch ft {
		case 0:
			return func(x float64) float64 { return pw(x) }, true
		case 1:
			return func(x float64) float64 {
				if x >= -bb/a {
					return pw(a*x + bb)
				}
				return 0
			}, true
		case 2:
			return func(x float64) float64 {
				if x >= -bb/a {
					return pw(a*x+bb) + c
				}
				return c
			}, true
		case 3:
			return func(x float64) float64 {
				if x >= d {
					return pw(a*x + bb)
				}
				return c * x
			}, true
		case 4:
			return func(x float64) float64 {
				if x >= d {
					return pw(a*x+bb) + e
				}
				return c*x + f
			}, true
		}
	}
	return nil, false
}

func inv3(m [3][3]float64) [3][3]float64 {
	a, b, c := m[0][0], m[0][1], m[0][2]
	d, e, f := m[1][0], m[1][1], m[1][2]
	g, h, i := m[2][0], m[2][1], m[2][2]
	det := a*(e*i-f*h) - b*(d*i-f*g) + c*(d*h-e*g)
	return [3][3]float64{
		{(e*i - f*h) / det, (c*h - b*i) / det, (b*f - c*e) / det},
		{(f*g - d*i) / det, (a*i - c*g) / det, (c*d - a*f) / det},
		{(d*h - e*g) / det, (b*g - a*h) / det, (a*e - b*d) / det},
	}
}

func mul3(a, b [3][3]float64) (r [3][3]float64) {
	for i := 0; i < 3; i++ {
		for j := 0; j < 3; j++ {
			for k := 0; k < 3; k++ {
				r[i][j] += a[i][k] * b[k][j]
			}
		}
	}
	return
}

func srgbEncode(l float64) float64 {
	if l <= 0.0031308 {
		return 12.92 * l
	}
	return 1.055*math.Pow(l, 1/2.4) - 0.055
}

func srgbDecode(v float64) float64 {
	if v <= 0.04045 {
		return v / 12.92
	}
	return math.Pow((v+0.055)/1.055, 2.4)
}

// isSRGB : le profil est (à peu près) sRGB → aucune conversion utile.
func (p *iccProfile) isSRGB() bool {
	for r := 0; r < 3; r++ {
		for c := 0; c < 3; c++ {
			if math.Abs(p.m[r][c]-srgbD50[r][c]) > 0.004 {
				return false
			}
		}
	}
	for c := 0; c < 3; c++ {
		for _, x := range []float64{0.02, 0.2, 0.5, 0.8} {
			if math.Abs(p.trc[c](x)-srgbDecode(x)) > 0.01 {
				return false
			}
		}
	}
	return true
}

// toSRGB convertit l'image décrite par le profil p (Display P3, Adobe RGB…) vers sRGB.
// Sans ça, les valeurs P3 sont lues comme du sRGB : couleurs délavées.
func toSRGB(src image.Image, p *iccProfile) *image.RGBA {
	t := mul3(inv3(srgbD50), p.m)
	var lin [3][256]float64
	for c := 0; c < 3; c++ {
		for v := 0; v < 256; v++ {
			lin[c][v] = p.trc[c](float64(v) / 255)
		}
	}
	const n = 4096
	var enc [n + 1]uint8
	for i := 0; i <= n; i++ {
		enc[i] = uint8(math.Round(srgbEncode(float64(i)/n) * 255))
	}
	q := func(x float64) uint8 {
		if x <= 0 {
			return 0
		}
		if x >= 1 {
			return 255
		}
		return enc[int(x*n+0.5)]
	}

	b := src.Bounds()
	dst := image.NewRGBA(image.Rect(0, 0, b.Dx(), b.Dy()))
	draw.Draw(dst, dst.Bounds(), src, b.Min, draw.Src)
	pix := dst.Pix
	for o := 0; o+3 < len(pix); o += 4 {
		r, g, bl := lin[0][pix[o]], lin[1][pix[o+1]], lin[2][pix[o+2]]
		pix[o] = q(t[0][0]*r + t[0][1]*g + t[0][2]*bl)
		pix[o+1] = q(t[1][0]*r + t[1][1]*g + t[1][2]*bl)
		pix[o+2] = q(t[2][0]*r + t[2][1]*g + t[2][2]*bl)
	}
	return dst
}

// ---------- orientation ----------

// orient applique l'orientation EXIF (2..8) pour obtenir l'image « debout ».
func orient(src image.Image, o int) image.Image {
	if o <= 1 || o > 8 {
		return src
	}
	b := src.Bounds()
	w, h := b.Dx(), b.Dy()
	dw, dh := w, h
	if o >= 5 {
		dw, dh = h, w
	}
	dst := image.NewRGBA(image.Rect(0, 0, dw, dh))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			var dx, dy int
			switch o {
			case 2:
				dx, dy = w-1-x, y
			case 3:
				dx, dy = w-1-x, h-1-y
			case 4:
				dx, dy = x, h-1-y
			case 5:
				dx, dy = y, x
			case 6:
				dx, dy = h-1-y, x
			case 7:
				dx, dy = h-1-y, w-1-x
			case 8:
				dx, dy = y, w-1-x
			}
			dst.Set(dx, dy, src.At(b.Min.X+x, b.Min.Y+y))
		}
	}
	return dst
}

// decodeNormalized décode l'image, la redresse selon l'EXIF et la convertit en sRGB.
// changed=false si l'octet-à-octet d'origine peut être conservé tel quel (JPEG sRGB, non tourné).
func decodeNormalized(data []byte) (img image.Image, format string, changed bool, err error) {
	img, format, err = image.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, "", false, err
	}
	var icc []byte
	orientation := 1
	switch format {
	case "jpeg":
		icc, orientation = jpegMeta(data)
	case "webp":
		icc = webpICC(data)
	}
	if p, ok := parseICC(icc); ok && !p.isSRGB() {
		img = toSRGB(img, p)
		changed = true
	}
	if orientation > 1 {
		img = orient(img, orientation)
		changed = true
	}
	return img, format, changed || format != "jpeg", nil
}

// orientedSize renvoie les dimensions affichées (en tenant compte de l'EXIF).
func orientedSize(data []byte, w, h int) (int, int) {
	if _, o := jpegMeta(data); o >= 5 {
		return h, w
	}
	return w, h
}
