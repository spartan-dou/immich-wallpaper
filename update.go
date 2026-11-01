package main

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// Dépôt GitHub (public) dont les releases servent aux mises à jour.
const updateRepo = "spartan-dou/immich-wallpaper"

var updateAPI = "https://api.github.com/repos/" + updateRepo + "/releases/latest"

type ghRelease struct {
	TagName    string `json:"tag_name"`
	Draft      bool   `json:"draft"`
	Prerelease bool   `json:"prerelease"`
	Assets     []struct {
		Name string `json:"name"`
		URL  string `json:"browser_download_url"`
	} `json:"assets"`
}

// parseVersion transforme « v1.4.0 » / « 1.4 » en [1 4 0]. ok=false pour une version de dev.
func parseVersion(s string) ([3]int, bool) {
	var v [3]int
	s = strings.TrimPrefix(strings.TrimSpace(s), "v")
	parts := strings.Split(s, ".")
	if len(parts) == 0 || len(parts) > 3 {
		return v, false
	}
	for i, p := range parts {
		n, err := strconv.Atoi(p)
		if err != nil || n < 0 {
			return v, false
		}
		v[i] = n
	}
	return v, true
}

func newerVersion(latest, current string) bool {
	l, ok1 := parseVersion(latest)
	c, ok2 := parseVersion(current)
	if !ok1 || !ok2 {
		return false
	}
	for i := 0; i < 3; i++ {
		if l[i] != c[i] {
			return l[i] > c[i]
		}
	}
	return false
}

func assetName(goarch string) string {
	if goarch == "arm64" {
		return "ImmichWallpaper-arm64.exe"
	}
	return "ImmichWallpaper-x64.exe"
}

// Update décrit une mise à jour disponible.
type Update struct {
	Version string
	exeURL  string
	sumsURL string
	name    string
}

var updateHTTP = &http.Client{Timeout: 5 * time.Minute}

func ghGet(url string) (*http.Response, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "ImmichWallpaper/"+appVersion)
	req.Header.Set("Accept", "application/vnd.github+json")
	resp, err := updateHTTP.Do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode != 200 {
		resp.Body.Close()
		return nil, fmt.Errorf("GitHub : HTTP %d sur %s", resp.StatusCode, url)
	}
	return resp, nil
}

// checkUpdate renvoie la mise à jour disponible, ou nil si on est à jour.
func checkUpdate(current string) (*Update, error) {
	if _, ok := parseVersion(current); !ok {
		return nil, fmt.Errorf("version de développement (%s) : mise à jour automatique désactivée", current)
	}
	resp, err := ghGet(updateAPI)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var rel ghRelease
	if err := json.NewDecoder(resp.Body).Decode(&rel); err != nil {
		return nil, err
	}
	if rel.Draft || rel.Prerelease || !newerVersion(rel.TagName, current) {
		return nil, nil
	}
	u := &Update{Version: strings.TrimPrefix(rel.TagName, "v"), name: assetName(runtime.GOARCH)}
	for _, a := range rel.Assets {
		switch a.Name {
		case u.name:
			u.exeURL = a.URL
		case "SHA256SUMS.txt":
			u.sumsURL = a.URL
		}
	}
	if u.exeURL == "" || u.sumsURL == "" {
		return nil, fmt.Errorf("release %s incomplète (exe ou SHA256SUMS.txt manquant)", rel.TagName)
	}
	return u, nil
}

// Download récupère l'exe et vérifie son empreinte SHA256 avant de le renvoyer.
func (u *Update) Download() ([]byte, error) {
	resp, err := ghGet(u.sumsURL)
	if err != nil {
		return nil, err
	}
	sums, _ := io.ReadAll(io.LimitReader(resp.Body, 64<<10))
	resp.Body.Close()
	want := ""
	sc := bufio.NewScanner(bytes.NewReader(sums))
	for sc.Scan() {
		f := strings.Fields(sc.Text())
		if len(f) == 2 && strings.TrimPrefix(f[1], "*") == u.name {
			want = strings.ToLower(f[0])
		}
	}
	if want == "" {
		return nil, errors.New("empreinte introuvable dans SHA256SUMS.txt")
	}

	resp, err = ghGet(u.exeURL)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	bin, err := io.ReadAll(io.LimitReader(resp.Body, 100<<20))
	if err != nil {
		return nil, err
	}
	sum := sha256.Sum256(bin)
	if hex.EncodeToString(sum[:]) != want {
		return nil, errors.New("empreinte SHA256 incorrecte : fichier téléchargé corrompu, mise à jour annulée")
	}
	if len(bin) < 2 || bin[0] != 'M' || bin[1] != 'Z' {
		return nil, errors.New("le fichier téléchargé n'est pas un exécutable Windows")
	}
	return bin, nil
}
