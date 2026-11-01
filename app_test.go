package main

import (
	"bytes"
	"encoding/json"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const (
	alb1 = "11111111-1111-1111-1111-111111111111"
	alb2 = "22222222-2222-2222-2222-222222222222"
	key  = "secret-token"
)

func mkImg(w, h int, format string) []byte {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	img.Set(1, 1, color.RGBA{255, 0, 0, 255})
	var b bytes.Buffer
	if format == "png" {
		png.Encode(&b, img)
	} else {
		jpeg.Encode(&b, img, nil)
	}
	return b.Bytes()
}

// fakeImmich : alb1 renvoie ses assets directement (ancienne API),
// alb2 ne les renvoie pas → passage par /api/search/metadata (paginé).
func fakeImmich(t *testing.T) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("x-api-key") != key {
			w.WriteHeader(401)
			return
		}
		switch {
		case r.URL.Path == "/api/albums" && r.URL.Query().Get("shared") == "":
			json.NewEncoder(w).Encode([]map[string]any{{"id": alb1, "albumName": "Finlande", "assetCount": 3}})
		case r.URL.Path == "/api/albums":
			json.NewEncoder(w).Encode([]map[string]any{{"id": alb1, "albumName": "Finlande", "assetCount": 3}, {"id": alb2, "albumName": "Bassin", "assetCount": 2}})
		case r.URL.Path == "/api/albums/"+alb1:
			json.NewEncoder(w).Encode(map[string]any{"albumName": "Finlande", "assetCount": 3, "assets": []map[string]any{
				{"id": "p1", "type": "IMAGE", "originalFileName": "portrait.jpg"},
				{"id": "v1", "type": "VIDEO"},
				{"id": "t1", "type": "IMAGE", "isTrashed": true},
			}})
		case r.URL.Path == "/api/albums/"+alb2:
			json.NewEncoder(w).Encode(map[string]any{"albumName": "Bassin", "assetCount": 2})
		case r.URL.Path == "/api/search/metadata":
			var body struct{ Page int }
			json.NewDecoder(r.Body).Decode(&body)
			if body.Page == 1 {
				json.NewEncoder(w).Encode(map[string]any{"assets": map[string]any{"items": []map[string]any{{"id": "l1", "type": "IMAGE", "originalFileName": "paysage.png"}}, "nextPage": "2"}})
			} else {
				json.NewEncoder(w).Encode(map[string]any{"assets": map[string]any{"items": []map[string]any{{"id": "l2", "type": "IMAGE", "originalFileName": "paysage2.jpg"}}, "nextPage": nil}})
			}
		case r.URL.Path == "/api/assets/l2" || r.URL.Path == "/api/assets/l1":
			json.NewEncoder(w).Encode(map[string]any{"localDateTime": "2025-03-14T10:00:00.000Z",
				"exifInfo": map[string]any{"city": "Rovaniemi", "state": "Laponie", "country": "Finlande"}})
		case r.URL.Path == "/api/assets/p1/thumbnail":
			w.Write(mkImg(30, 60, "jpeg"))
		case r.URL.Path == "/api/assets/l1/thumbnail":
			if r.URL.Query().Get("size") == "fullsize" {
				w.WriteHeader(404) // simule un vieux Immich sans fullsize
				return
			}
			w.Write(mkImg(80, 40, "png"))
		case r.URL.Path == "/api/assets/l2/thumbnail" && r.URL.Query().Get("size") == "fullsize":
			http.Redirect(w, r, "original?c=1", http.StatusFound) // comme Immich sans fullsize généré
		case r.URL.Path == "/api/assets/l2/original":
			w.WriteHeader(403) // token sans asset.download
		case r.URL.Path == "/api/assets/l2/thumbnail":
			w.Write(mkImg(80, 40, "jpeg"))
		default:
			t.Logf("404 %s", r.URL)
			w.WriteHeader(404)
		}
	}))
}

func TestParseAlbumID(t *testing.T) {
	id, err := parseAlbumID("https://immich.local/albums/" + strings.ToUpper(alb1) + "?x=1")
	if err != nil || id != alb1 {
		t.Fatal(id, err)
	}
}

func TestCoreNext(t *testing.T) {
	srv := fakeImmich(t)
	defer srv.Close()
	dir := t.TempDir()
	var applied []string
	core := newCore(dir, func(p, s string) error { applied = append(applied, p); return nil })
	cfg := &Config{ServerURL: srv.URL + "/api/", APIKey: key, Albums: []string{alb1, alb2}, LandscapeOnly: true}
	if err := cfg.normalize(); err != nil {
		t.Fatal(err)
	}
	core.SetConfig(cfg)
	seen := map[string]bool{}
	for i := 0; i < 6; i++ {
		a, err := core.Next()
		if err != nil {
			t.Fatal(err)
		}
		if a.ID == "p1" {
			t.Fatal("portrait choisi alors que landscape_only")
		}
		seen[a.ID] = true
	}
	if !seen["l1"] || !seen["l2"] || len(core.assets) != 3 {
		t.Fatalf("seen=%v assets=%d", seen, len(core.assets))
	}
	b, _ := os.ReadFile(applied[len(applied)-1])
	if _, f, err := image.DecodeConfig(bytes.NewReader(b)); err != nil || f != "jpeg" {
		t.Fatal("fichier écrit non JPEG", f, err)
	}
	// Mauvais token
	cfg2 := *cfg
	cfg2.APIKey = "nope"
	core.SetConfig(&cfg2)
	if _, err := core.Next(); err != errUnauthorized {
		t.Fatal("attendu errUnauthorized, eu", err)
	}
}

func TestPortraitFallback(t *testing.T) {
	srv := fakeImmich(t)
	defer srv.Close()
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	cfg := &Config{ServerURL: srv.URL, APIKey: key, Albums: []string{alb1}, LandscapeOnly: true}
	cfg.normalize()
	core.SetConfig(cfg)
	a, err := core.Next()
	if err != nil || a.ID != "p1" {
		t.Fatal(a, err)
	}
}

func TestSettingsServer(t *testing.T) {
	srv := fakeImmich(t)
	defer srv.Close()
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	writeDefaultConfig(cfgPath)
	s, err := startSettingsServer(cfgPath)
	if err != nil {
		t.Fatal(err)
	}
	var saved *Config
	s.OnSaved = func(c *Config) { saved = c }
	post := func(p string, v any) (int, map[string]any, []byte) {
		b, _ := json.Marshal(v)
		r, err := http.Post(s.URL()+p, "application/json", bytes.NewReader(b))
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		raw := new(bytes.Buffer)
		raw.ReadFrom(r.Body)
		m := map[string]any{}
		json.Unmarshal(raw.Bytes(), &m)
		return r.StatusCode, m, raw.Bytes()
	}
	// page sans jeton → 404
	if r, _ := http.Get("http://" + s.addr + "/"); r.StatusCode != 404 {
		t.Fatal("page accessible sans jeton")
	}
	if r, _ := http.Get(s.URL()); r.StatusCode != 200 {
		t.Fatal("page", r.StatusCode)
	}
	// liste des albums
	code, _, raw := post("api/albums", map[string]any{"server_url": srv.URL, "api_key": key})
	if code != 200 || !strings.Contains(string(raw), "Bassin") {
		t.Fatal(code, string(raw))
	}
	code, m, _ := post("api/albums", map[string]any{"server_url": srv.URL, "api_key": "bad"})
	if code != 400 || !strings.Contains(m["error"].(string), "token") {
		t.Fatal(code, m)
	}
	// enregistrement
	code, m, _ = post("api/save", map[string]any{"server_url": srv.URL, "api_key": key, "albums": []string{alb2}, "interval_minutes": 15, "style": "fit"})
	if code != 200 || saved == nil || saved.Style != "fit" {
		t.Fatal(code, m)
	}
	// nouvel enregistrement sans token → le token est conservé
	code, m, _ = post("api/save", map[string]any{"server_url": srv.URL, "api_key": "", "albums": []string{alb1, alb2}})
	if code != 200 || saved.APIKey != key || len(saved.Albums) != 2 {
		t.Fatal(code, m, saved)
	}
	// le token n'est jamais renvoyé
	r, _ := http.Get(s.URL() + "api/config")
	var uc map[string]any
	json.NewDecoder(r.Body).Decode(&uc)
	if uc["api_key"] != "" || uc["has_key"] != true {
		t.Fatal(uc)
	}
	// album manquant → erreur
	code, _, _ = post("api/save", map[string]any{"server_url": srv.URL, "albums": []string{}})
	if code != 400 {
		t.Fatal("save sans album accepté")
	}
}

func TestCaptionPipeline(t *testing.T) {
	srv := fakeImmich(t)
	defer srv.Close()
	var wpPath, wpStyle, lockPath string
	core := newCore(t.TempDir(), func(p, s string) error { wpPath, wpStyle = p, s; return nil })
	core.screen = func(string) Screen { return Screen{W: 1920, H: 1080, BottomInset: 48} }
	core.setLockScreen = func(p string) error { lockPath = p; return nil }
	cfg := &Config{ServerURL: srv.URL, APIKey: key, Albums: []string{alb2}, Style: "fit", ShowPlace: true, ShowDate: true, LockScreen: true}
	if err := cfg.normalize(); err != nil {
		t.Fatal(err)
	}
	core.SetConfig(cfg)
	if _, err := core.Next(); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(wpPath)
	ic, _, _ := image.DecodeConfig(bytes.NewReader(b))
	if ic.Width != 1920 || ic.Height != 1080 || wpStyle != "fill" || lockPath != wpPath {
		t.Fatalf("%dx%d style=%s lock=%q", ic.Width, ic.Height, wpStyle, lockPath)
	}
}

func TestCaptionText(t *testing.T) {
	var i AssetInfo
	i.Exif.City, i.Exif.State, i.Exif.Country = "Paris", "Paris", "France"
	i.LocalDateTime = "2024-08-02T18:00:00.000Z"
	if p, d := captionParts(i, true, true); p != "Paris, France" || d != "août 2024" {
		t.Fatal(p, d)
	}
	if p, d := captionParts(AssetInfo{}, true, false); p != "" || d != "" {
		t.Fatal(p, d)
	}
}

func TestRenderSample(t *testing.T) {
	out := os.Getenv("SAMPLE_OUT")
	if out == "" {
		t.Skip()
	}
	src := image.NewRGBA(image.Rect(0, 0, 3000, 2000))
	for y := 0; y < 2000; y++ {
		for x := 0; x < 3000; x++ {
			src.Set(x, y, color.RGBA{uint8(40 + y/12), uint8(90 + y/16), uint8(160 + y/30), 255})
		}
	}
	for y := 1300; y < 2000; y++ { // "sol" clair pour tester la lisibilité
		for x := 0; x < 3000; x++ {
			src.Set(x, y, color.RGBA{235, 238, 242, 255})
		}
	}
	var img image.Image = src
	if in := os.Getenv("SAMPLE_IN"); in != "" {
		f, _ := os.Open(in)
		img, _, _ = image.Decode(f)
		f.Close()
	}
	dst := compose(img, Screen{W: 1920, H: 1080, BottomInset: 48}, "fill")
	drawCaption(dst, loadFonts(), "Rovaniemi, Laponie, Finlande", "mars 2025", "bottom-right", 48)
	f, _ := os.Create(out)
	jpeg.Encode(f, dst, &jpeg.Options{Quality: 90})
	f.Close()
}
