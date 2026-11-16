package main

import (
	"crypto/rand"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"log"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"
)

//go:embed settings.html
var settingsHTML []byte

// settingsIdle : la page de réglages s'arrête après ce délai sans requête. Ouverte, elle
// interroge l'état toutes les 5 s (seulement quand l'onglet est visible) : le port ne reste
// donc ouvert que pendant que la page sert, pas en permanence.
const settingsIdle = 10 * time.Minute

// SettingsServer sert la page de réglages sur 127.0.0.1, protégée par un jeton aléatoire dans
// l'URL. Le port n'est ouvert qu'à la demande (Open) et se referme après inactivité.
type SettingsServer struct {
	cfgPath string
	token   string
	idle    time.Duration

	lmu    sync.Mutex // état de l'écoute
	addr   string     // "" : fermée
	srv    *http.Server
	last   time.Time // dernière requête valide
	active int       // requêtes en cours (« Image suivante » peut durer)

	mu sync.Mutex
	// Empreinte du certificat vu lors du dernier test réussi, par URL de serveur
	// (option « certificat auto-signé ») : c'est elle qui est épinglée à l'enregistrement.
	seenPins map[string]string

	OnSaved         func(*Config)
	OnNext          func() (string, error)
	Status          func() string
	Excluded        func() int   // nombre de photos masquées
	OnClearExcluded func() error // les réautorise toutes
}

// newSettingsServer prépare la page de réglages sans ouvrir de port.
func newSettingsServer(cfgPath string) (*SettingsServer, error) {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return nil, err
	}
	return &SettingsServer{cfgPath: cfgPath, token: hex.EncodeToString(b), idle: settingsIdle, seenPins: map[string]string{}}, nil
}

// startSettingsServer prépare la page et ouvre tout de suite son port.
func startSettingsServer(cfgPath string) (*SettingsServer, error) {
	s, err := newSettingsServer(cfgPath)
	if err != nil {
		return nil, err
	}
	if _, err := s.Open(); err != nil {
		return nil, err
	}
	return s, nil
}

// Open ouvre le port de la page s'il est fermé et renvoie l'adresse à ouvrir dans le navigateur.
func (s *SettingsServer) Open() (string, error) {
	s.lmu.Lock()
	defer s.lmu.Unlock()
	s.last = time.Now()
	if s.srv == nil {
		ln, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			return "", err
		}
		srv := &http.Server{
			Handler:           s,
			ReadHeaderTimeout: 10 * time.Second,
			ReadTimeout:       30 * time.Second,
			IdleTimeout:       2 * time.Minute,
			MaxHeaderBytes:    64 << 10,
			// Pas de WriteTimeout : « Image suivante » répond à la fin du téléchargement.
		}
		s.srv, s.addr = srv, ln.Addr().String()
		go func() {
			if err := srv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) {
				log.Printf("page de réglages arrêtée : %v", err)
			}
		}()
		if s.idle > 0 {
			go s.closeWhenIdle(srv)
		}
	}
	return s.urlLocked(), nil
}

// closeWhenIdle referme le port quand la page ne sert plus (onglet fermé ou caché).
func (s *SettingsServer) closeWhenIdle(srv *http.Server) {
	t := time.NewTicker(min(s.idle/4, time.Minute))
	defer t.Stop()
	for range t.C {
		s.lmu.Lock()
		if s.srv != srv {
			s.lmu.Unlock()
			return
		}
		if s.active > 0 || time.Since(s.last) < s.idle {
			s.lmu.Unlock()
			continue
		}
		s.srv, s.addr = nil, ""
		s.lmu.Unlock()
		srv.Close()
		log.Printf("page de réglages fermée après inactivité (plus aucun port ouvert)")
		return
	}
}

// Close referme le port de la page.
func (s *SettingsServer) Close() {
	s.lmu.Lock()
	srv := s.srv
	s.srv, s.addr = nil, ""
	s.lmu.Unlock()
	if srv != nil {
		srv.Close()
	}
}

// URL : adresse de la page ("" si son port est fermé).
func (s *SettingsServer) URL() string {
	s.lmu.Lock()
	defer s.lmu.Unlock()
	return s.urlLocked()
}

func (s *SettingsServer) urlLocked() string {
	if s.addr == "" {
		return ""
	}
	return "http://" + s.addr + "/" + s.token + "/"
}

type uiConfig struct {
	Config
	HasKey        bool `json:"has_key"`
	ExcludedCount int  `json:"excluded_count"`
	Portable      bool `json:"portable"` // la page adapte ses libellés (mises à jour sur clic)
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, err error) {
	writeJSON(w, 400, map[string]string{"error": err.Error()})
}

// Taille maximale du corps d'une requête de la page (la configuration fait quelques Ko).
const maxSettingsBody = 1 << 20

func (s *SettingsServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.lmu.Lock()
	addr := s.addr
	s.lmu.Unlock()
	// Host inattendu : requête forgée par un autre site (DNS rebinding).
	if addr == "" || r.Host != addr {
		http.Error(w, "hôte refusé", http.StatusForbidden)
		return
	}
	prefix := "/" + s.token + "/"
	if !strings.HasPrefix(r.URL.Path, prefix) {
		http.NotFound(w, r)
		return
	}
	// Requête de la page elle-même : elle garde le port ouvert.
	s.lmu.Lock()
	s.active++
	s.last = time.Now()
	s.lmu.Unlock()
	defer func() {
		s.lmu.Lock()
		s.active--
		s.last = time.Now()
		s.lmu.Unlock()
	}()
	h := w.Header()
	h.Set("Cache-Control", "no-store")
	h.Set("Referrer-Policy", "no-referrer")
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("X-Frame-Options", "DENY")
	h.Set("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "+
		"connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")

	route := strings.TrimPrefix(r.URL.Path, prefix)
	switch route {
	case "", "api/config", "api/status":
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			http.Error(w, "méthode refusée", http.StatusMethodNotAllowed)
			return
		}
	case "api/albums", "api/save", "api/next", "api/clear-excluded":
		// Routes d'action : POST JSON de la page elle-même uniquement.
		if r.Method != http.MethodPost {
			http.Error(w, "méthode refusée", http.StatusMethodNotAllowed)
			return
		}
		if o := r.Header.Get("Origin"); o != "" && o != "http://"+addr {
			http.Error(w, "origine refusée", http.StatusForbidden)
			return
		}
		if !strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
			http.Error(w, "type de contenu refusé", http.StatusUnsupportedMediaType)
			return
		}
		r.Body = http.MaxBytesReader(w, r.Body, maxSettingsBody)
	default:
		http.NotFound(w, r)
		return
	}

	switch route {
	case "":
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write(settingsHTML)
	case "api/config":
		s.mu.Lock()
		cfg, _ := readConfig(s.cfgPath)
		s.mu.Unlock()
		hasKey := cfg.APIKey != ""
		cfg.APIKey = "" // le token n'est jamais renvoyé à la page
		excluded := 0
		if s.Excluded != nil {
			excluded = s.Excluded()
		}
		writeJSON(w, 200, uiConfig{cfg, hasKey, excluded, isPortable()})
	case "api/albums":
		var in Config
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			writeErr(w, err)
			return
		}
		in, err := s.mergeKey(in)
		if err != nil {
			writeErr(w, err)
			return
		}
		in.Albums = []string{"00000000-0000-0000-0000-000000000000"} // pour passer la validation
		in.CertFingerprint = ""                                      // test : on relève le certificat présenté
		if err := in.normalize(); err != nil {
			writeErr(w, err)
			return
		}
		client := newClient(&in)
		defer client.Close()
		albums, err := client.Albums()
		if err != nil {
			writeErr(w, err)
			return
		}
		if fp := client.Fingerprint(); fp != "" {
			s.mu.Lock()
			s.seenPins[in.ServerURL] = fp
			s.mu.Unlock()
		}
		writeJSON(w, 200, albums)
	case "api/save":
		var in Config
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			writeErr(w, err)
			return
		}
		in, err := s.mergeKey(in)
		if err != nil {
			writeErr(w, err)
			return
		}
		check := in
		check.Albums = append([]string(nil), in.Albums...)
		check.CertFingerprint = ""
		if err := check.normalize(); err != nil {
			writeErr(w, err)
			return
		}
		s.mu.Lock()
		if check.InsecureSkipVerify {
			// Empreinte relevée au dernier test de ce serveur, sinon celle déjà enregistrée pour lui.
			if fp := s.seenPins[check.ServerURL]; fp != "" {
				check.CertFingerprint = fp
			} else if old, _ := readConfig(s.cfgPath); sameServer(old.ServerURL, check.ServerURL) {
				check.CertFingerprint = normalizeFingerprint(old.CertFingerprint)
			}
		}
		err = saveConfig(s.cfgPath, check)
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
	case "api/clear-excluded":
		if s.OnClearExcluded != nil {
			if err := s.OnClearExcluded(); err != nil {
				writeErr(w, err)
				return
			}
		}
		writeJSON(w, 200, map[string]string{"ok": "photos réaffichées"})
	case "api/status":
		st := ""
		if s.Status != nil {
			st = s.Status()
		}
		writeJSON(w, 200, map[string]string{"status": st})
	}
}

// sameServer : les deux adresses désignent le même serveur une fois normalisées.
func sameServer(a, b string) bool {
	na, err1 := normalizeServerURL(a)
	nb, err2 := normalizeServerURL(b)
	return err1 == nil && err2 == nil && strings.EqualFold(na, nb)
}

// mergeKey garde le token enregistré si le champ est laissé vide dans la page, mais seulement
// pour le serveur auquel il a été confié : sinon une requête pourrait le faire envoyer ailleurs.
func (s *SettingsServer) mergeKey(in Config) (Config, error) {
	if strings.TrimSpace(in.APIKey) != "" {
		return in, nil
	}
	s.mu.Lock()
	old, _ := readConfig(s.cfgPath)
	s.mu.Unlock()
	if old.APIKey != "" && !sameServer(old.ServerURL, in.ServerURL) {
		return in, errors.New("l'URL du serveur a changé : saisis de nouveau le token (clé API)")
	}
	in.APIKey = old.APIKey
	return in, nil
}
