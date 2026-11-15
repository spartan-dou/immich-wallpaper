package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func at(month time.Month, hour, min int) time.Time {
	return time.Date(2026, month, 15, hour, min, 0, 0, time.Local)
}

func TestClockRanges(t *testing.T) {
	cases := []struct {
		from, to string
		h, m     int
		want     bool
	}{
		{"", "", 3, 0, true}, {"08:00", "", 3, 0, true}, {"08:00", "08:00", 3, 0, true},
		{"08:00", "18:30", 8, 0, true}, {"08:00", "18:30", 18, 29, true}, {"08:00", "18:30", 18, 30, false},
		{"08:00", "18:30", 7, 59, false},
		{"22:00", "07:00", 23, 0, true}, {"22:00", "07:00", 6, 59, true}, {"22:00", "07:00", 7, 0, false}, {"22:00", "07:00", 12, 0, false},
	}
	for _, c := range cases {
		if got := inClockRange(c.from, c.to, at(time.May, c.h, c.m)); got != c.want {
			t.Errorf("%s-%s à %02d:%02d : %v", c.from, c.to, c.h, c.m, got)
		}
	}
	for m, want := range map[time.Month]string{time.January: "winter", time.April: "spring", time.July: "summer", time.October: "autumn", time.December: "winter"} {
		if seasonOf(m) != want {
			t.Errorf("%v : %s", m, seasonOf(m))
		}
	}
	if v, err := normalizeClock("7:05"); err != nil || v != "07:05" {
		t.Error(v, err)
	}
	for _, bad := range []string{"25:00", "12", "12:60", "midi"} {
		if _, err := normalizeClock(bad); err == nil {
			t.Errorf("%q accepté", bad)
		}
	}
}

func TestAlbumRulesConfig(t *testing.T) {
	c := defaultConfig()
	c.ServerURL, c.APIKey, c.Albums = "https://immich.exemple.fr", key, []string{alb1}
	c.ActiveFrom, c.ActiveTo = "8:00", "22:00"
	c.AlbumRules = []AlbumRule{
		{Albums: []string{"https://immich.exemple.fr/albums/" + alb2, alb2}, Season: "Summer"},
		{Albums: nil, Season: "winter"}, // sans album : ignorée
	}
	if err := c.normalize(); err != nil {
		t.Fatal(err)
	}
	if len(c.AlbumRules) != 1 || len(c.AlbumRules[0].Albums) != 1 || c.AlbumRules[0].Albums[0] != alb2 || c.ActiveFrom != "08:00" {
		t.Fatalf("%+v %q", c.AlbumRules, c.ActiveFrom)
	}
	if got := c.albumsAt(at(time.July, 12, 0)); len(got) != 1 || got[0] != alb2 {
		t.Fatal("été :", got)
	}
	if got := c.albumsAt(at(time.January, 12, 0)); len(got) != 1 || got[0] != alb1 {
		t.Fatal("hiver :", got)
	}
	if len(c.allAlbums()) != 2 || c.activeAt(at(time.May, 23, 0)) || !c.activeAt(at(time.May, 9, 0)) {
		t.Fatal("allAlbums / activeAt")
	}
	for _, bad := range []func(*Config){
		func(c *Config) { c.ActiveTo = "" },
		func(c *Config) { c.AlbumRules = []AlbumRule{{Albums: []string{alb2}, Season: "mousson"}} },
		func(c *Config) { c.AlbumRules = []AlbumRule{{Albums: []string{alb2}, From: "9h"}} },
		func(c *Config) { c.AlbumRules = []AlbumRule{{Albums: []string{"pas-un-album"}}} },
	} {
		d := c
		bad(&d)
		if err := d.normalize(); err == nil {
			t.Errorf("configuration invalide acceptée : %+v", d.AlbumRules)
		}
	}
}

// Les albums d'une règle remplacent les albums habituels pendant sa saison.
func TestAlbumRulesDraw(t *testing.T) {
	srv, _ := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	now := at(time.July, 12, 0)
	core.now = func() time.Time { return now }
	cfg := testCfg(srv.URL, alb1) // habituel : un seul portrait (p1)
	cfg.AlbumRules = []AlbumRule{{Albums: []string{alb2}, Season: "summer"}}
	core.SetConfig(cfg)
	for i := 0; i < 4; i++ {
		r, err := core.Next()
		if err != nil || r.Asset.ID == p1 {
			t.Fatal("en été, seul l'album de la règle doit servir :", r.Asset.ID, err)
		}
	}
	if len(core.assets) != 3 {
		t.Fatal("tous les albums référencés doivent être chargés :", len(core.assets))
	}
	now = at(time.January, 12, 0)
	if r, err := core.Next(); err != nil || r.Asset.ID != p1 {
		t.Fatal("hors saison, retour aux albums habituels :", r.Asset.ID, err)
	}
}

func assetFetches(st *proxyState) int {
	st.mu.Lock()
	defer st.mu.Unlock()
	n := 0
	for k, v := range st.fetches {
		if len(k) > 11 && k[:11] == "/api/assets" {
			n += v
		}
	}
	return n
}

// Après un préchargement, le changement suivant n'a besoin d'aucune requête.
func TestPreload(t *testing.T) {
	srv, st := newProxy(t)
	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	cfg := testCfg(srv.URL, alb2)
	cfg.ShowPlace = true
	core.SetConfig(cfg)
	first, err := core.Next()
	if err != nil {
		t.Fatal(err)
	}
	if err := core.Preload(); err != nil || len(core.preloaded) != 1 {
		t.Fatal("préchargement :", err, len(core.preloaded))
	}
	want := core.preloaded[0].asset.ID
	if want == first.Asset.ID {
		t.Fatal("la photo préchargée doit différer de la photo affichée")
	}
	st.mu.Lock()
	st.down = true // plus de réseau
	total := 0
	for _, v := range st.fetches {
		total += v
	}
	st.mu.Unlock()
	r, err := core.Next()
	if err != nil || r.Offline || r.Asset.ID != want {
		t.Fatalf("photo préchargée attendue (%s), obtenu %+v, %v", want, r, err)
	}
	st.mu.Lock()
	after := 0
	for _, v := range st.fetches {
		after += v
	}
	st.mu.Unlock()
	if after != total {
		t.Fatalf("requêtes pendant un changement préchargé : %d → %d", total, after)
	}
	if e, _ := os.ReadDir(core.preloadDir()); len(e) != 0 || len(core.preloaded) != 0 {
		t.Fatal("fichiers de préchargement non nettoyés")
	}
	// La légende a été préparée à l'avance elle aussi.
	if core.lastPhotos[0].Info.Exif.City != "Rovaniemi" {
		t.Fatal("informations de la photo préchargée absentes")
	}

	// Un changement de configuration périme le préchargement ; l'option peut être désactivée.
	st.mu.Lock()
	st.down = false
	st.mu.Unlock()
	if err := core.Preload(); err != nil || len(core.preloaded) != 1 {
		t.Fatal(err)
	}
	cfg2 := *cfg
	cfg2.Preload = false
	core.SetConfig(&cfg2)
	if len(core.preloaded) != 0 {
		t.Fatal("préchargement conservé après un changement de configuration")
	}
	before := assetFetches(st)
	if err := core.Preload(); err != nil || len(core.preloaded) != 0 || assetFetches(st) != before {
		t.Fatal("préchargement effectué alors que l'option est désactivée")
	}
}

func TestPrevious(t *testing.T) {
	srv, st := newProxy(t)
	var applied string
	core := newCore(t.TempDir(), func(p, s string) error { applied = p; return nil })
	core.SetConfig(testCfg(srv.URL, alb2))
	if _, err := core.Previous(); !errors.Is(err, errNoPrevious) {
		t.Fatal("errNoPrevious attendu :", err)
	}
	a, _ := core.Next()
	b, err := core.Next()
	if err != nil || a.Asset.ID == b.Asset.ID {
		t.Fatal(a, b, err)
	}
	before, path := assetFetches(st), applied
	r, err := core.Previous()
	if err != nil || r.Asset.ID != a.Asset.ID || assetFetches(st) != before || applied == path {
		t.Fatalf("photo précédente : %+v, %v", r, err)
	}
	if as, _, ok := core.Last(); !ok || as.ID != a.Asset.ID || core.lastPhotos[0].Asset.ID != a.Asset.ID {
		t.Fatal("photo courante non mise à jour")
	}
	if _, err := core.Previous(); !errors.Is(err, errNoPrevious) {
		t.Fatal("historique épuisé :", err)
	}
	// L'historique est borné.
	for i := 0; i < historyDepth+3; i++ {
		core.Next()
	}
	if len(core.history) != historyDepth {
		t.Fatal("historique :", len(core.history))
	}
}

func TestExclude(t *testing.T) {
	srv, _ := newProxy(t)
	dir := t.TempDir()
	core := newCore(dir, func(p, s string) error { return nil })
	if _, err := core.ExcludeCurrent(); err == nil {
		t.Fatal("rien à masquer avant le premier affichage")
	}
	core.SetConfig(testCfg(srv.URL, alb2))
	core.Next()
	first, _ := core.Next()
	hidden, err := core.ExcludeCurrent()
	if err != nil || hidden.ID != first.Asset.ID || core.ExcludedCount() != 1 {
		t.Fatal(hidden, err)
	}
	if _, err := os.Stat(filepath.Join(core.cacheDir(), hidden.ID+".jpg")); err == nil {
		t.Fatal("photo masquée encore dans le cache hors ligne")
	}
	for i := 0; i < 6; i++ {
		r, err := core.Next()
		if err != nil || r.Asset.ID == hidden.ID {
			t.Fatal("photo masquée réaffichée :", r.Asset.ID, err)
		}
	}
	for _, set := range core.history {
		for _, k := range set {
			if k.Asset.ID == hidden.ID {
				t.Fatal("photo masquée encore dans l'historique")
			}
		}
	}
	// La liste survit à un redémarrage, puis peut être vidée.
	again := newCore(dir, func(p, s string) error { return nil })
	if again.ExcludedCount() != 1 {
		t.Fatal("liste d'exclusion non relue")
	}
	if err := again.ClearExcluded(); err != nil || again.ExcludedCount() != 0 {
		t.Fatal(err)
	}
	if newCore(dir, nil).ExcludedCount() != 0 {
		t.Fatal("liste d'exclusion non vidée sur disque")
	}
}

func TestFavorite(t *testing.T) {
	var method, path string
	var body map[string]any
	status := 200
	fav := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		method, path = r.Method, r.URL.Path
		json.NewDecoder(r.Body).Decode(&body)
		w.WriteHeader(status)
		w.Write([]byte(`{"id":"x","isFavorite":true}`))
	}))
	defer fav.Close()
	c := newClient(&Config{ServerURL: fav.URL, APIKey: key})
	defer c.Close()
	if err := c.SetFavorite(l1); err != nil || method != "PUT" || path != "/api/assets/"+l1 || body["isFavorite"] != true {
		t.Fatal(err, method, path, body)
	}
	status = 403
	if err := c.SetFavorite(l1); err == nil || !bytes.Contains([]byte(err.Error()), []byte("asset.update")) {
		t.Fatal("message de permission attendu :", err)
	}

	core := newCore(t.TempDir(), func(p, s string) error { return nil })
	if _, err := core.Favorite(); err == nil {
		t.Fatal("aucune photo affichée : erreur attendue")
	}
}

func TestSettingsExcluded(t *testing.T) {
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	writeDefaultConfig(cfgPath)
	s, err := startSettingsServer(cfgPath)
	if err != nil {
		t.Fatal(err)
	}
	n := 3
	s.Excluded = func() int { return n }
	s.OnClearExcluded = func() error { n = 0; return nil }
	get := func() map[string]any {
		r, err := http.Get(s.URL() + "api/config")
		if err != nil {
			t.Fatal(err)
		}
		defer r.Body.Close()
		m := map[string]any{}
		json.NewDecoder(r.Body).Decode(&m)
		return m
	}
	if m := get(); m["excluded_count"] != float64(3) || m["preload"] != true {
		t.Fatal(m)
	}
	if r, _ := http.Post(s.URL()+"api/clear-excluded", "application/json", bytes.NewReader([]byte("{}"))); r.StatusCode != 200 {
		t.Fatal(r.StatusCode)
	}
	if m := get(); m["excluded_count"] != float64(0) {
		t.Fatal(m)
	}
}
