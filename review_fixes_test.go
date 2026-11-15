package main

import (
	"bytes"
	"errors"
	"image"
	"image/color"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Les portraits écartés (landscape_only) ne doivent ni entrer dans le cache hors ligne,
// ni coûter un appel d'informations ; hors ligne, les paysages du cache passent d'abord.
func TestPortraitsNotCached(t *testing.T) {
	srv, st := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	cfg := testCfg(srv.URL, alb1, alb2)
	cfg.ShowPlace = true
	core.SetConfig(cfg)
	for i := 0; i < 6; i++ {
		if _, err := core.Next(); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := os.Stat(filepath.Join(core.cacheDir(), p1+".jpg")); err == nil {
		t.Fatal("portrait écarté présent dans le cache hors ligne")
	}
	st.mu.Lock()
	infos := st.fetches["/api/assets/"+p1]
	st.mu.Unlock()
	if infos != 0 {
		t.Fatal("informations demandées pour un portrait écarté :", infos)
	}
	// Identifiant non UUID renvoyé par le serveur : jamais utilisé.
	for _, a := range core.assets {
		if !isUUID(a.ID) {
			t.Fatal("identifiant non valide accepté :", a.ID)
		}
	}
}

func TestOfflinePrefersLandscape(t *testing.T) {
	srv, st := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	cfg := testCfg(srv.URL, alb1) // un seul portrait : affiché en secours, donc mis en cache
	core.SetConfig(cfg)
	if r, err := core.Next(); err != nil || r.Asset.ID != p1 {
		t.Fatal(r, err)
	}
	cfg2 := testCfg(srv.URL, alb1, alb2)
	core.SetConfig(cfg2)
	for i := 0; i < 4; i++ {
		core.Next()
	}
	st.mu.Lock()
	st.down = true
	st.mu.Unlock()
	core.albumsChecked = core.albumsChecked.AddDate(-1, 0, 0)
	for i := 0; i < 5; i++ {
		r, err := core.Next()
		if err != nil || !r.Offline {
			t.Fatal(r, err)
		}
		if r.Asset.ID == p1 {
			t.Fatal("portrait tiré du cache alors que des paysages y sont disponibles")
		}
	}
}

// Le recadrage maison reproduit chaque style, et « Étendre » reste appliqué en « Étendre ».
func TestStyles(t *testing.T) {
	src := image.NewRGBA(image.Rect(0, 0, 100, 50))
	for i := range src.Pix {
		src.Pix[i] = 255
	}
	sc := Screen{W: 400, H: 300}
	black := func(img *image.RGBA, x, y int) bool { return img.RGBAAt(x, y) == color.RGBA{0, 0, 0, 255} }
	white := func(img *image.RGBA, x, y int) bool { return img.RGBAAt(x, y) == color.RGBA{255, 255, 255, 255} }
	for _, style := range []string{"fill", "fit", "stretch", "center", "tile", "span"} {
		dst := compose(src, sc, style)
		if dst.Bounds().Dx() != 400 || dst.Bounds().Dy() != 300 || !white(dst, 200, 150) {
			t.Fatalf("%s : rendu inattendu", style)
		}
		corner := white(dst, 2, 2)
		switch style {
		case "fit", "center": // bandes noires autour de l'image
			if corner || !black(dst, 2, 2) {
				t.Fatalf("%s : bandes noires attendues", style)
			}
		default:
			if !corner {
				t.Fatalf("%s : l'image doit couvrir tout l'écran", style)
			}
		}
	}
	// Centrer : l'image garde sa taille d'origine (100 × 50 au milieu de 400 × 300).
	c := compose(src, sc, "center")
	if !white(c, 151, 126) || !black(c, 148, 150) || !white(c, 248, 173) || !black(c, 200, 176) {
		t.Fatal("center : image mal placée")
	}

	srv := fakeImmich(t)
	defer srv.Close()
	var style string
	core := newCore(t.TempDir(), func(p, s string) error { style = s; return nil })
	core.screen = func(string) Screen { return Screen{W: 3840, H: 1080} } // bureau virtuel de deux écrans
	cfg := testCfg(srv.URL, alb2)
	cfg.Style, cfg.ShowDate, cfg.SplitWide = "span", true, false
	core.SetConfig(cfg)
	if _, err := core.Next(); err != nil || style != "span" {
		t.Fatalf("style appliqué %q (attendu span) : %v", style, err)
	}
}

// Image aux dimensions démesurées : refusée à la lecture de l'en-tête, avant toute allocation.
func TestPixelLimit(t *testing.T) {
	huge := []byte{0xFF, 0xD8,
		0xFF, 0xE0, 0x00, 0x10, 'J', 'F', 'I', 'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
		0xFF, 0xC0, 0x00, 0x11, 8, 0xFF, 0xFF, 0xFF, 0xFF, 3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1}
	if _, _, err := image.DecodeConfig(bytes.NewReader(huge)); err != nil {
		t.Fatal("en-tête de test invalide :", err)
	}
	if _, err := probeImage(huge); err == nil || !strings.Contains(err.Error(), "dimensions") {
		t.Fatal("image de 65535 × 65535 acceptée :", err)
	}
	if m, err := probeImage(mkImg(30, 60, "jpeg")); err != nil || m.W != 30 || m.H != 60 {
		t.Fatal(m, err)
	}
}

// Profil ICC aberrant (gamma négatif → NaN) : la conversion ne doit pas paniquer.
func TestToSRGBNaN(t *testing.T) {
	nan := func(x float64) float64 { return (x - x) / (x - x) }
	p := &iccProfile{m: srgbD50, trc: [3]func(float64) float64{nan, nan, nan}}
	out := toSRGB(image.NewRGBA(image.Rect(0, 0, 8, 200)), p)
	if out.Bounds().Dy() != 200 {
		t.Fatal(out.Bounds())
	}
}

func TestOrient(t *testing.T) {
	src := image.NewRGBA(image.Rect(0, 0, 3, 2))
	src.SetRGBA(0, 0, color.RGBA{255, 0, 0, 255})
	for o, want := range map[int][3]int{1: {3, 2, 0}, 2: {3, 2, 2}, 3: {3, 2, 5}, 4: {3, 2, 3}, 5: {2, 3, 0}, 6: {2, 3, 1}, 7: {2, 3, 5}, 8: {2, 3, 4}} {
		d := orient(src, o)
		b := d.Bounds()
		idx := -1
		for i := 0; i < b.Dx()*b.Dy(); i++ {
			if r, _, _, _ := d.At(i%b.Dx(), i/b.Dx()).RGBA(); r != 0 {
				idx = i
			}
		}
		if b.Dx() != want[0] || b.Dy() != want[1] || idx != want[2] {
			t.Errorf("orientation %d : %dx%d, pixel en %d (attendu %v)", o, b.Dx(), b.Dy(), idx, want)
		}
	}
}

func TestServerURLValidation(t *testing.T) {
	for in, want := range map[string]string{
		" https://immich.exemple.fr/api/ ": "https://immich.exemple.fr",
		"http://192.168.1.10:2283":         "http://192.168.1.10:2283",
		"https://exemple.fr/immich":        "https://exemple.fr/immich",
	} {
		if got, err := normalizeServerURL(in); err != nil || got != want {
			t.Errorf("%q → %q, %v", in, got, err)
		}
	}
	for _, bad := range []string{"immich.local", "https://", "https://a.fr\r\nIconFile=x", "https://a.fr?x=1", "https://a.fr#x", "https://u:p@a.fr", "ftp://a.fr"} {
		if got, err := normalizeServerURL(bad); err == nil {
			t.Errorf("%q accepté (%q)", bad, got)
		}
	}
	if isUUID("x"+alb1) || isUUID(alb1+"\n") || isUUID(`..\config`) || !isUUID(alb1) {
		t.Error("isUUID")
	}
}

// 401 = clé refusée (erreur globale) ; 403 = ressource interdite (on passe à la suivante).
func TestAuthErrors(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "forbidden") {
			w.WriteHeader(403)
			return
		}
		w.WriteHeader(401)
	}))
	defer srv.Close()
	c := newClient(&Config{ServerURL: srv.URL, APIKey: key})
	defer c.Close()
	if _, err := c.Download("forbidden", "preview"); !errors.Is(err, errForbidden) || errors.Is(err, errUnauthorized) {
		t.Fatal(err)
	}
	if _, err := c.Download(l1, "preview"); !errors.Is(err, errUnauthorized) {
		t.Fatal(err)
	}
	c.Close()
	if _, err := c.Download(l1, "preview"); err == nil || isAuthErr(err) {
		t.Fatal("requête après Close non annulée :", err)
	}
}

// Certificat auto-signé : épinglé au premier contact, refusé s'il change.
func TestCertPinning(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("[]")) })
	a := httptest.NewTLSServer(handler)
	defer a.Close()
	cfg := &Config{ServerURL: a.URL, APIKey: key, InsecureSkipVerify: true}
	c := newClient(cfg)
	defer c.Close()
	if _, err := c.Albums(); err != nil {
		t.Fatal(err)
	}
	fp := c.Fingerprint()
	if len(fp) != 64 {
		t.Fatal("empreinte non relevée :", fp)
	}
	// Même certificat, empreinte fournie (avec séparateurs) : accepté.
	cfg.CertFingerprint = strings.ToUpper(fp[:2]) + ":" + fp[2:]
	ok := newClient(cfg)
	defer ok.Close()
	if _, err := ok.Albums(); err != nil {
		t.Fatal(err)
	}
	// Autre empreinte attendue : refusé.
	cfg.CertFingerprint = strings.Repeat("ab", 32)
	bad := newClient(cfg)
	defer bad.Close()
	if _, err := bad.Albums(); err == nil || !strings.Contains(err.Error(), "certificat") {
		t.Fatal("certificat inattendu accepté :", err)
	}
	// Sans l'option, un certificat non reconnu reste refusé.
	strict := newClient(&Config{ServerURL: a.URL, APIKey: key})
	defer strict.Close()
	if _, err := strict.Albums(); err == nil {
		t.Fatal("certificat non vérifié accepté sans l'option")
	}
}

func TestRotatingLog(t *testing.T) {
	path := filepath.Join(t.TempDir(), "journal.txt")
	l, err := openRotatingLog(path, 100)
	if err != nil {
		t.Fatal(err)
	}
	line := []byte(strings.Repeat("x", 39) + "\n")
	for i := 0; i < 5; i++ {
		if _, err := l.Write(line); err != nil {
			t.Fatal(err)
		}
	}
	cur, _ := os.ReadFile(path)
	old, _ := os.ReadFile(l.backupPath())
	if len(cur) == 0 || len(cur) > 100 || len(old) == 0 || len(old) > 100 {
		t.Fatalf("journal %d octets, ancien %d octets", len(cur), len(old))
	}
	l.f.Close()
}

// La clé API ne reste en clair sur disque que si le système ne sait pas la chiffrer.
func TestConfigKeyRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.json")
	cfg := defaultConfig()
	cfg.APIKey = key
	if err := saveConfig(path, cfg); err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(path)
	if _, canProtect := protectSecret("x"); canProtect && (bytes.Contains(raw, []byte(key)) || keyStoredPlain(path)) {
		t.Fatal("clé en clair dans le fichier :", string(raw))
	}
	got, err := readConfig(path)
	if err != nil || got.APIKey != key || got.APIKeyProtected != "" {
		t.Fatal(got.APIKey, err)
	}
}

func TestPanicBecomesError(t *testing.T) {
	srv := fakeImmich(t)
	defer srv.Close()
	core := newCore(t.TempDir(), func(p, s string) error { panic("boum") })
	core.SetConfig(testCfg(srv.URL, alb2))
	if _, err := core.Next(); err == nil || !strings.Contains(err.Error(), "boum") {
		t.Fatal("panic non converti en erreur :", err)
	}
	// Le verrou est bien relâché : l'appel suivant ne bloque pas.
	core.setWallpaper = func(p, s string) error { return nil }
	if _, err := core.Next(); err != nil {
		t.Fatal(err)
	}
}

func FuzzImageMeta(f *testing.F) {
	f.Add(mkImg(8, 4, "jpeg"))
	f.Add(mkImg(8, 4, "png"))
	f.Add([]byte("RIFF\x10\x00\x00\x00WEBPICCP\x04\x00\x00\x00abcd"))
	f.Add([]byte("\xFF\xD8\xFF\xE1\x00\x10Exif\x00\x00II*\x00\x08\x00\x00\x00"))
	f.Fuzz(func(t *testing.T, data []byte) {
		jpegMeta(data)
		exifOrientation(data)
		webpICC(data)
		probeImage(data)
	})
}

func FuzzICC(f *testing.F) {
	f.Add(make([]byte, 200))
	f.Add([]byte("para\x00\x00\x00\x00\x00\x00\x00\x00\xFF\xFF\x00\x00"))
	f.Add([]byte("curv\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\xFF\xFF"))
	img := image.NewRGBA(image.Rect(0, 0, 4, 4))
	for i := range img.Pix {
		img.Pix[i] = uint8(i * 5)
	}
	f.Fuzz(func(t *testing.T, data []byte) {
		if p, ok := parseICC(data); ok {
			p.isSRGB()
			toSRGB(img, p)
		}
		if fn, ok := parseCurve(data); ok {
			p := &iccProfile{m: srgbD50, trc: [3]func(float64) float64{fn, fn, fn}}
			toSRGB(img, p)
		}
	})
}
