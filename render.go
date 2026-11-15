package main

import (
	"image"
	"image/draw"

	xdraw "golang.org/x/image/draw"
)

// Screen décrit la zone à couvrir (en pixels physiques) et la hauteur de la barre des tâches.
type Screen struct {
	W, H        int
	BottomInset int
}

// compose redimensionne l'image à la taille de l'écran en reproduisant le style Windows,
// pour que la légende incrustée ne soit jamais rognée.
func compose(src image.Image, sc Screen, style string) *image.RGBA {
	dst := image.NewRGBA(image.Rect(0, 0, sc.W, sc.H))
	sb := src.Bounds()
	sw, sh := float64(sb.Dx()), float64(sb.Dy())
	W, H := float64(sc.W), float64(sc.H)

	switch style {
	case "fit":
		draw.Draw(dst, dst.Bounds(), image.Black, image.Point{}, draw.Src)
		s := min(W/sw, H/sh)
		w, h := int(sw*s+0.5), int(sh*s+0.5)
		x, y := (sc.W-w)/2, (sc.H-h)/2
		xdraw.CatmullRom.Scale(dst, image.Rect(x, y, x+w, y+h), src, sb, draw.Src, nil)
	case "stretch":
		xdraw.CatmullRom.Scale(dst, dst.Bounds(), src, sb, draw.Src, nil)
	case "center": // taille d'origine, centrée (rognée si plus grande que l'écran), fond noir
		draw.Draw(dst, dst.Bounds(), image.Black, image.Point{}, draw.Src)
		off := image.Pt((sc.W-sb.Dx())/2, (sc.H-sb.Dy())/2)
		draw.Draw(dst, sb.Sub(sb.Min).Add(off), src, sb.Min, draw.Src)
	case "tile": // taille d'origine, répétée depuis le coin supérieur gauche
		for y := 0; y < sc.H; y += sb.Dy() {
			for x := 0; x < sc.W; x += sb.Dx() {
				draw.Draw(dst, sb.Sub(sb.Min).Add(image.Pt(x, y)), src, sb.Min, draw.Src)
			}
		}
	default: // fill ; span : même recadrage, l'écran passé est alors le bureau virtuel
		s := max(W/sw, H/sh)
		cw, ch := int(W/s+0.5), int(H/s+0.5)
		cx, cy := sb.Min.X+(sb.Dx()-cw)/2, sb.Min.Y+(sb.Dy()-ch)/2
		xdraw.CatmullRom.Scale(dst, dst.Bounds(), src, image.Rect(cx, cy, cx+cw, cy+ch), draw.Src, nil)
	}
	return dst
}
