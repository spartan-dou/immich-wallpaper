package main

import (
	"crypto/rand"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"net"
	"net/http"
	"strings"
	"sync"
)

//go:embed settings.html
var settingsHTML []byte

// SettingsServer sert la page de réglages sur 127.0.0.1, protégée par un jeton aléatoire dans l'URL.
type SettingsServer struct {
	cfgPath string
	token   string
	addr    string
	mu      sync.Mutex

	OnSaved func(*Config)
	OnNext  func() (string, error)
	Status  func() string
}

func startSettingsServer(cfgPath string) (*SettingsServer, error) {
	b := make([]byte, 16)
	rand.Read(b)
	s := &SettingsServer{cfgPath: cfgPath, token: hex.EncodeToString(b)}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	s.addr = ln.Addr().String()
	go http.Serve(ln, s)
	return s, nil
}

func (s *SettingsServer) URL() string { return "http://" + s.addr + "/" + s.token + "/" }

type uiConfig struct {
	Config
	HasKey bool `json:"has_key"`
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, err error) {
	writeJSON(w, 400, map[string]string{"error": err.Error()})
}

func (s *SettingsServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	prefix := "/" + s.token + "/"
	if !strings.HasPrefix(r.URL.Path, prefix) {
		http.NotFound(w, r)
		return
	}
	// Protection basique contre les requêtes venant d'autres sites.
	if r.Method == "POST" {
		if o := r.Header.Get("Origin"); o != "" && o != "http://"+s.addr {
			http.Error(w, "origine refusée", 403)
			return
		}
	}
	switch strings.TrimPrefix(r.URL.Path, prefix) {
	case "":
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write(settingsHTML)
	case "api/config":
		s.mu.Lock()
		cfg, _ := readConfig(s.cfgPath)
		s.mu.Unlock()
		hasKey := cfg.APIKey != ""
		cfg.APIKey = "" // le token n'est jamais renvoyé à la page
		writeJSON(w, 200, uiConfig{cfg, hasKey})
	case "api/albums":
		var in Config
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			writeErr(w, err)
			return
		}
		in = s.mergeKey(in)
		in.Albums = []string{"00000000-0000-0000-0000-000000000000"} // pour passer la validation
		if err := in.normalize(); err != nil {
			writeErr(w, err)
			return
		}
		albums, err := newClient(&in).Albums()
		if err != nil {
			writeErr(w, err)
			return
		}
		writeJSON(w, 200, albums)
	case "api/save":
		var in Config
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			writeErr(w, err)
			return
		}
		in = s.mergeKey(in)
		check := in
		check.Albums = append([]string(nil), in.Albums...)
		if err := check.normalize(); err != nil {
			writeErr(w, err)
			return
		}
		s.mu.Lock()
		err := saveConfig(s.cfgPath, check)
		s.mu.Unlock()
		if err != nil {
			writeErr(w, err)
			return
		}
		if s.OnSaved != nil {
			s.OnSaved(&check)
		}
		writeJSON(w, 200, map[string]string{"ok": "enregistré"})
	case "api/next":
		if s.OnNext == nil {
			writeJSON(w, 200, map[string]string{})
			return
		}
		msg, err := s.OnNext()
		if err != nil {
			writeErr(w, err)
			return
		}
		writeJSON(w, 200, map[string]string{"ok": msg})
	case "api/status":
		st := ""
		if s.Status != nil {
			st = s.Status()
		}
		writeJSON(w, 200, map[string]string{"status": st})
	default:
		http.NotFound(w, r)
	}
}

// mergeKey garde le token enregistré si le champ est laissé vide dans la page.
func (s *SettingsServer) mergeKey(in Config) Config {
	if strings.TrimSpace(in.APIKey) == "" {
		s.mu.Lock()
		old, _ := readConfig(s.cfgPath)
		s.mu.Unlock()
		in.APIKey = old.APIKey
	}
	return in
}
