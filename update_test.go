package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"runtime"
	"strings"
	"testing"
)

func TestNewerVersion(t *testing.T) {
	cases := []struct {
		l, c string
		want bool
	}{
		{"v1.5.0", "1.4.0", true}, {"v1.4.0", "1.4.0", false}, {"v1.10.0", "1.9.3", true},
		{"v1.4", "1.4.0", false}, {"v2.0.0", "dev-abc1234", false}, {"v1.3.9", "1.4.0", false},
	}
	for _, c := range cases {
		if got := newerVersion(c.l, c.c); got != c.want {
			t.Errorf("%s vs %s : %v", c.l, c.c, got)
		}
	}
}

func TestCheckAndDownloadUpdate(t *testing.T) {
	bin := append([]byte("MZ"), []byte(strings.Repeat("x", 1000))...)
	sum := sha256.Sum256(bin)
	name := assetName(runtime.GOARCH)
	corrupt := false
	var srv *httptest.Server
	srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/latest":
			json.NewEncoder(w).Encode(map[string]any{"tag_name": "v1.5.0", "assets": []map[string]any{
				{"name": name, "browser_download_url": srv.URL + "/exe"},
				{"name": "SHA256SUMS.txt", "browser_download_url": srv.URL + "/sums"},
			}})
		case "/sums":
			w.Write([]byte(hex.EncodeToString(sum[:]) + "  " + name + "\nabc  autre.exe\n"))
		case "/exe":
			if corrupt {
				w.Write([]byte("MZcorrompu"))
				return
			}
			w.Write(bin)
		}
	}))
	defer srv.Close()
	updateAPI = srv.URL + "/latest"

	if u, err := checkUpdate("1.5.0"); err != nil || u != nil {
		t.Fatal("déjà à jour attendu", u, err)
	}
	u, err := checkUpdate("1.4.0")
	if err != nil || u == nil || u.Version != "1.5.0" {
		t.Fatal(u, err)
	}
	got, err := u.Download()
	if err != nil || len(got) != len(bin) {
		t.Fatal(err)
	}
	corrupt = true
	if _, err := u.Download(); err == nil || !strings.Contains(err.Error(), "SHA256") {
		t.Fatal("corruption non détectée", err)
	}
	if _, err := checkUpdate("dev-1234567"); err == nil {
		t.Fatal("une version de dev ne doit pas se mettre à jour")
	}
}
