package main

import (
	"encoding/json"
	"image"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"os"
	"sync"
	"testing"
	"time"
)

// proxy devant fakeImmich : compte les requêtes, peut simuler une panne,
// et contrôle la signature de l'album 2 (nombre de photos).
type proxyState struct {
	mu      sync.Mutex
	down    bool
	count2  int
	fetches map[string]int
}

func newProxy(t *testing.T) (*httptest.Server, *proxyState) {
	inner := fakeImmich(t)
	t.Cleanup(inner.Close)
	u, _ := url.Parse(inner.URL)
	rp := httputil.NewSingleHostReverseProxy(u)
	st := &proxyState{count2: 2, fetches: map[string]int{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		st.mu.Lock()
		down := st.down
		st.fetches[r.URL.Path]++
		c2 := st.count2
		st.mu.Unlock()
		if down {
			hj, _ := w.(http.Hijacker)
			conn, _, _ := hj.Hijack()
			conn.Close() // coupure réseau
			return
		}
		if r.URL.Path == "/api/albums" && r.URL.Query().Get("shared") == "true" {
			json.NewEncoder(w).Encode([]map[string]any{
				{"id": alb1, "albumName": "Finlande", "assetCount": 3, "updatedAt": "2026-01-01T00:00:00Z"},
				{"id": alb2, "albumName": "Bassin", "assetCount": c2, "updatedAt": "2026-01-01T00:00:00Z"},
			})
			return
		}
		rp.ServeHTTP(w, r)
	}))
	t.Cleanup(srv.Close)
	return srv, st
}

func testCfg(srvURL string, albums ...string) *Config {
	c := defaultConfig()
	c.ServerURL, c.APIKey, c.Albums = srvURL, key, albums
	c.ShowPlace = false
	if err := c.normalize(); err != nil {
		panic(err)
	}
	return &c
}

func TestIncrementalAlbums(t *testing.T) {
	srv, st := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	core.SetConfig(testCfg(srv.URL, alb1, alb2))
	if _, err := core.Next(); err != nil {
		t.Fatal(err)
	}
	if st.fetches["/api/albums/"+alb2] != 1 || st.fetches["/api/albums/"+alb1] != 1 {
		t.Fatal("chargement initial", st.fetches)
	}
	// Vérification suivante sans changement : aucun album rechargé.
	core.albumsChecked = time.Time{}
	core.Next()
	if st.fetches["/api/albums/"+alb2] != 1 || st.fetches["/api/albums/"+alb1] != 1 {
		t.Fatal("album rechargé alors qu'inchangé", st.fetches)
	}
	// Album 2 modifié : lui seul est rechargé.
	st.mu.Lock()
	st.count2 = 3
	st.mu.Unlock()
	core.albumsChecked = time.Time{}
	core.Next()
	if st.fetches["/api/albums/"+alb2] != 2 || st.fetches["/api/albums/"+alb1] != 1 {
		t.Fatal("rechargement incrémental incorrect", st.fetches)
	}
}

func TestOfflineCache(t *testing.T) {
	srv, st := newProxy(t)
	dir := t.TempDir()
	var applied string
	core := newCore(dir, func(p, s string) error { applied = p; return nil })
	cfg := testCfg(srv.URL, alb2)
	cfg.OfflineCache = 1
	core.SetConfig(cfg)
	for i := 0; i < 3; i++ {
		if _, err := core.Next(); err != nil {
			t.Fatal(err)
		}
	}
	if n := len(core.cachedIDs()); n != 1 {
		t.Fatal("le cache doit être limité à 1, contient", n)
	}
	st.mu.Lock()
	st.down = true
	st.mu.Unlock()
	core.albumsChecked = time.Time{}
	applied = ""
	r, err := core.Next()
	if err != nil || !r.Offline || r.Cause == nil || applied == "" {
		t.Fatal("bascule hors ligne attendue", r, err)
	}
	// Cache désactivé : l'erreur remonte.
	os.RemoveAll(core.cacheDir())
	if _, err := core.Next(); err == nil {
		t.Fatal("erreur attendue sans cache")
	}
}

func TestMultiMonitor(t *testing.T) {
	srv, _ := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { t.Fatal("mode écran unique utilisé"); return nil })
	core.monitors = func() []Monitor {
		return []Monitor{{"ecranA", Screen{W: 1920, H: 1080, BottomInset: 48}}, {"ecranB", Screen{W: 2560, H: 1440}}}
	}
	var got map[string]string
	core.setMonitorWallpapers = func(p map[string]string, style string) error { got = p; return nil }
	var lock string
	core.setLockScreen = func(p string) error { lock = p; return nil }
	cfg := testCfg(srv.URL, alb2)
	cfg.MultiMonitor, cfg.LockScreen = "different", true
	core.SetConfig(cfg)
	if _, err := core.Next(); err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || lock != got["ecranA"] {
		t.Fatal(got, lock)
	}
	for id, want := range map[string][2]int{"ecranA": {1920, 1080}, "ecranB": {2560, 1440}} {
		f, _ := os.Open(got[id])
		ic, _, _ := image.DecodeConfig(f)
		f.Close()
		if ic.Width != want[0] || ic.Height != want[1] {
			t.Fatalf("%s : %dx%d", id, ic.Width, ic.Height)
		}
	}
	if len(core.recent) != 2 || core.recent[0] == core.recent[1] {
		t.Fatal("deux photos différentes attendues", core.recent)
	}
}
