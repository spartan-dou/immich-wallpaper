package main

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// Fichier témoin portable.txt à côté de l'exe (zip portable, à la manière d'omnictl).
func TestPortableMarker(t *testing.T) {
	dir := t.TempDir()
	if hasPortableMarker(dir) {
		t.Fatal("dossier vide vu comme portable")
	}
	if err := os.Mkdir(filepath.Join(dir, portableMarker), 0o755); err != nil {
		t.Fatal(err)
	}
	if hasPortableMarker(dir) {
		t.Fatal("un dossier portant le nom du témoin ne doit pas compter")
	}
	other := t.TempDir()
	if err := os.WriteFile(filepath.Join(other, portableMarker), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if !hasPortableMarker(other) {
		t.Fatal("témoin non détecté")
	}
	// L'exe de test n'a pas de témoin à côté de lui : les autres tests voient la version classique.
	if portableByMarker() {
		t.Fatal("témoin inattendu à côté de l'exe de test")
	}
}

// Version portable : rien n'est déposé hors de son dossier par défaut (icône du bureau), et
// le zip (exe classique + témoin) se met à jour avec l'exe classique.
func TestPortableDefaults(t *testing.T) {
	defer func(f string) { flavor = f }(flavor)
	flavor = ""
	if !defaultConfig().DesktopLink {
		t.Fatal("version classique : icône du bureau attendue par défaut")
	}
	if assetName("amd64") != "ImmichWallpaper-x64.exe" {
		t.Fatal(assetName("amd64"))
	}
	flavor = "portable"
	if defaultConfig().DesktopLink {
		t.Fatal("version portable : pas d'icône sur le bureau par défaut")
	}
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	if err := writeDefaultConfig(cfgPath); err != nil {
		t.Fatal(err)
	}
	if cfg, err := readConfig(cfgPath); err != nil || cfg.DesktopLink {
		t.Fatal("configuration portable par défaut :", cfg.DesktopLink, err)
	}
}

// La page de réglages n'ouvre son port qu'à la demande et le referme après inactivité.
func TestSettingsServerOnDemand(t *testing.T) {
	cfgPath := filepath.Join(t.TempDir(), "config.json")
	writeDefaultConfig(cfgPath)
	s, err := newSettingsServer(cfgPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if s.URL() != "" {
		t.Fatal("port ouvert avant la première ouverture")
	}
	s.idle = 300 * time.Millisecond
	u, err := s.Open()
	if err != nil || u == "" || u != s.URL() {
		t.Fatal(u, err)
	}
	if u2, _ := s.Open(); u2 != u {
		t.Fatal("une seconde ouverture doit réutiliser le port ouvert", u, u2)
	}
	r, err := http.Get(u + "api/status")
	if err != nil || r.StatusCode != 200 {
		t.Fatal("page inaccessible", err)
	}
	r.Body.Close()

	deadline := time.Now().Add(5 * time.Second)
	for s.URL() != "" {
		if time.Now().After(deadline) {
			t.Fatal("port toujours ouvert après inactivité")
		}
		time.Sleep(20 * time.Millisecond)
	}
	client := &http.Client{Timeout: 2 * time.Second}
	if r, err := client.Get(u + "api/status"); err == nil {
		r.Body.Close()
		t.Fatal("la page répond encore après fermeture", r.StatusCode)
	}

	u3, err := s.Open()
	if err != nil || u3 == "" {
		t.Fatal(u3, err)
	}
	if r, err := http.Get(u3); err != nil || r.StatusCode != 200 {
		t.Fatal("page non rouverte", err)
	} else {
		r.Body.Close()
	}
	s.Close()
	if s.URL() != "" {
		t.Fatal("Close n'a pas fermé le port")
	}
}
