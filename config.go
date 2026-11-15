package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/url"
	"os"
	"regexp"
	"strings"
)

// Config est lue depuis %APPDATA%\ImmichWallpaper\config.json.
type Config struct {
	ServerURL string `json:"server_url"`
	// APIKey : clé en clair, en mémoire. Sur disque elle est chiffrée dans APIKeyProtected
	// (DPAPI, liée au compte Windows) ; une clé saisie en clair à la main reste acceptée.
	APIKey          string `json:"api_key"`
	APIKeyProtected string `json:"api_key_protected,omitempty"`
	// CertFingerprint : empreinte SHA-256 du certificat accepté quand insecure_skip_verify est actif.
	CertFingerprint    string   `json:"cert_fingerprint,omitempty"`
	Albums             []string `json:"albums"`
	IntervalMinutes    int      `json:"interval_minutes"`
	ImageSize          string   `json:"image_size"` // fullsize | preview | original
	LandscapeOnly      bool     `json:"landscape_only"`
	Style              string   `json:"style"` // fill | fit | stretch | center | tile | span
	ChangeOnStart      bool     `json:"change_on_start"`
	InsecureSkipVerify bool     `json:"insecure_skip_verify"`
	LockScreen         bool     `json:"lock_screen"`
	ShowPlace          bool     `json:"show_place"`
	ShowDate           bool     `json:"show_date"`
	CaptionPosition    string   `json:"caption_position"` // bottom-right | bottom-left | top-right | top-left
	DesktopLink        bool     `json:"desktop_link"`     // icône « Voir cette photo dans Immich » sur le bureau
	AutoUpdate         bool     `json:"auto_update"`      // mises à jour automatiques depuis GitHub
	MultiMonitor       string   `json:"multi_monitor"`    // same | different
	OfflineCache       int      `json:"offline_cache"`    // nombre de photos gardées pour le mode hors ligne (0 = désactivé)
	SplitWide          bool     `json:"split_wide"`       // écran très large (32:9) : deux photos côte à côte
	ChangeOnUnlock     bool     `json:"change_on_unlock"` // nouvelle photo à chaque déverrouillage de session
}

func defaultConfig() Config {
	return Config{
		ServerURL:       "https://immich.exemple.fr",
		APIKey:          "",
		Albums:          []string{},
		IntervalMinutes: 60,
		ImageSize:       "fullsize",
		LandscapeOnly:   true,
		Style:           "fill",
		ChangeOnStart:   true,
		ShowPlace:       true,
		CaptionPosition: "bottom-right",
		DesktopLink:     true,
		AutoUpdate:      true,
		MultiMonitor:    "same",
		OfflineCache:    40,
		SplitWide:       true,
		ChangeOnUnlock:  true,
	}
}

func writeDefaultConfig(path string) error {
	b, _ := json.MarshalIndent(defaultConfig(), "", "  ")
	return os.WriteFile(path, b, 0o600)
}

// readConfig lit le fichier sans valider (pour la page de réglages).
func readConfig(path string) (Config, error) {
	cfg := defaultConfig()
	cfg.Albums = nil
	b, err := os.ReadFile(path)
	if err != nil {
		return cfg, err
	}
	// Notepad peut ajouter un BOM UTF-8.
	b = bytes.TrimPrefix(b, []byte{0xEF, 0xBB, 0xBF})
	if err := json.Unmarshal(b, &cfg); err != nil {
		return cfg, fmt.Errorf("JSON invalide : %w", err)
	}
	if cfg.APIKey == "" && cfg.APIKeyProtected != "" {
		key, err := unprotectSecret(cfg.APIKeyProtected)
		if err != nil {
			// Fichier copié depuis un autre PC ou un autre compte : la clé doit être ressaisie.
			log.Printf("clé API illisible (chiffrée pour un autre compte Windows ?) : %v", err)
		}
		cfg.APIKey = key
	}
	cfg.APIKeyProtected = ""
	return cfg, nil
}

// keyStoredPlain indique si le fichier contient encore la clé API en clair.
func keyStoredPlain(path string) bool {
	b, err := os.ReadFile(path)
	if err != nil {
		return false
	}
	var raw struct {
		APIKey string `json:"api_key"`
	}
	return json.Unmarshal(bytes.TrimPrefix(b, []byte{0xEF, 0xBB, 0xBF}), &raw) == nil && raw.APIKey != ""
}

// loadConfig lit et valide la config.
func loadConfig(path string) (*Config, error) {
	cfg, err := readConfig(path)
	if err != nil {
		return nil, err
	}
	if err := cfg.normalize(); err != nil {
		return nil, err
	}
	return &cfg, nil
}

func saveConfig(path string, cfg Config) error {
	cfg.APIKeyProtected = ""
	if enc, ok := protectSecret(cfg.APIKey); ok {
		cfg.APIKey, cfg.APIKeyProtected = "", enc
	}
	b, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

var (
	uuidRe     = regexp.MustCompile(`(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`)
	fullUUIDRe = regexp.MustCompile(`(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
)

// isUUID : la chaîne est exactement un UUID (rien avant, rien après).
func isUUID(s string) bool { return fullUUIDRe.MatchString(s) }

// normalizeServerURL nettoie et valide l'adresse du serveur Immich.
func normalizeServerURL(s string) (string, error) {
	u := strings.TrimSpace(s)
	u = strings.TrimRight(u, "/")
	u = strings.TrimSuffix(u, "/api")
	if !strings.HasPrefix(u, "http://") && !strings.HasPrefix(u, "https://") {
		return "", errors.New("server_url doit commencer par http:// ou https://")
	}
	p, err := url.Parse(u) // refuse notamment les caractères de contrôle (retours à la ligne…)
	if err != nil || p.Hostname() == "" {
		return "", errors.New("server_url invalide")
	}
	if p.User != nil || p.RawQuery != "" || p.ForceQuery || p.Fragment != "" {
		return "", errors.New("server_url ne doit contenir ni identifiants, ni « ? », ni « # »")
	}
	return u, nil
}

// parseAlbumID accepte un UUID brut ou une URL du type https://immich/albums/<uuid>.
func parseAlbumID(s string) (string, error) {
	m := uuidRe.FindAllString(s, -1)
	if len(m) == 0 {
		return "", fmt.Errorf("album %q : aucun identifiant trouvé", s)
	}
	return strings.ToLower(m[len(m)-1]), nil
}

func (c *Config) normalize() error {
	u, err := normalizeServerURL(c.ServerURL)
	if err != nil {
		return err
	}
	c.ServerURL = u
	c.CertFingerprint = normalizeFingerprint(c.CertFingerprint)
	if !c.InsecureSkipVerify {
		c.CertFingerprint = ""
	}

	c.APIKey = strings.TrimSpace(c.APIKey)
	if c.APIKey == "" {
		return errors.New("api_key est vide (Immich → Paramètres du compte → Clés API)")
	}

	var ids []string
	seen := map[string]bool{}
	for _, a := range c.Albums {
		id, err := parseAlbumID(a)
		if err != nil {
			return err
		}
		if !seen[id] {
			seen[id] = true
			ids = append(ids, id)
		}
	}
	if len(ids) == 0 {
		return errors.New("albums est vide")
	}
	c.Albums = ids

	if c.IntervalMinutes < 1 {
		c.IntervalMinutes = 60
	}
	c.ImageSize = strings.ToLower(strings.TrimSpace(c.ImageSize))
	switch c.ImageSize {
	case "":
		c.ImageSize = "fullsize"
	case "fullsize", "preview", "original":
	default:
		return fmt.Errorf("image_size %q inconnu (fullsize, preview ou original)", c.ImageSize)
	}
	c.Style = strings.ToLower(strings.TrimSpace(c.Style))
	if c.Style == "" {
		c.Style = "fill"
	}
	if c.MultiMonitor != "different" {
		c.MultiMonitor = "same"
	}
	c.OfflineCache = min(max(c.OfflineCache, 0), 500)
	switch c.CaptionPosition {
	case "bottom-right", "bottom-left", "top-right", "top-left":
	default:
		c.CaptionPosition = "bottom-right"
	}
	if _, _, ok := styleValues(c.Style); !ok {
		return fmt.Errorf("style %q inconnu (fill, fit, stretch, center, tile, span)", c.Style)
	}
	return nil
}

// styleValues renvoie WallpaperStyle / TileWallpaper pour le registre Windows.
func styleValues(style string) (wallpaperStyle, tile string, ok bool) {
	switch style {
	case "fill":
		return "10", "0", true
	case "fit":
		return "6", "0", true
	case "stretch":
		return "2", "0", true
	case "center":
		return "0", "0", true
	case "tile":
		return "0", "1", true
	case "span":
		return "22", "0", true
	}
	return "", "", false
}
