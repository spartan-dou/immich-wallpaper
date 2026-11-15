package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

var (
	// 401 : la clé elle-même est refusée (erreur globale).
	errUnauthorized = errors.New("token refusé par Immich (401) : vérifie api_key")
	// 403 : la clé est valide mais n'a pas le droit d'accéder à cette ressource.
	errForbidden = errors.New("accès refusé par Immich (403) : vérifie les permissions du token")
)

func isAuthErr(err error) bool {
	return errors.Is(err, errUnauthorized) || errors.Is(err, errForbidden)
}

const (
	apiTimeout      = 2 * time.Minute  // requêtes JSON, et attente des en-têtes de réponse
	downloadTimeout = 15 * time.Minute // téléchargement d'une image, corps compris
)

type Asset struct {
	ID               string `json:"id"`
	Type             string `json:"type"`
	IsTrashed        bool   `json:"isTrashed"`
	OriginalFileName string `json:"originalFileName"`
}

type Client struct {
	base   string
	key    string
	http   *http.Client
	ctx    context.Context
	cancel context.CancelFunc

	pinMu sync.Mutex
	pin   string // empreinte SHA-256 (hex) du certificat accepté, si insecure_skip_verify
}

func newClient(cfg *Config) *Client {
	c := &Client{base: cfg.ServerURL, key: cfg.APIKey, pin: normalizeFingerprint(cfg.CertFingerprint)}
	c.ctx, c.cancel = context.WithCancel(context.Background())
	tr := http.DefaultTransport.(*http.Transport).Clone()
	tr.ResponseHeaderTimeout = apiTimeout
	if cfg.InsecureSkipVerify {
		// Certificat auto-signé : la chaîne n'est pas vérifiée, mais le certificat est épinglé.
		tr.TLSClientConfig = &tls.Config{InsecureSkipVerify: true, VerifyPeerCertificate: c.verifyPin}
	}
	c.http = &http.Client{Transport: tr}
	return c
}

// Close annule les requêtes en cours (changement de configuration, arrêt).
func (c *Client) Close() {
	c.cancel()
	c.http.CloseIdleConnections()
}

func normalizeFingerprint(s string) string {
	return strings.ToLower(strings.NewReplacer(":", "", " ", "").Replace(strings.TrimSpace(s)))
}

// verifyPin n'accepte que le certificat épinglé. Sans empreinte connue, le premier certificat
// présenté est adopté (confiance au premier contact) et exigé pour toutes les connexions suivantes.
func (c *Client) verifyPin(rawCerts [][]byte, _ [][]*x509.Certificate) error {
	if len(rawCerts) == 0 {
		return errors.New("le serveur n'a présenté aucun certificat")
	}
	sum := sha256.Sum256(rawCerts[0])
	got := hex.EncodeToString(sum[:])
	c.pinMu.Lock()
	defer c.pinMu.Unlock()
	if c.pin == "" {
		c.pin = got
		log.Printf("certificat non vérifié accepté et épinglé (SHA-256 %s)", got)
		return nil
	}
	if got != c.pin {
		return fmt.Errorf("le certificat du serveur a changé (empreinte %s… au lieu de %s…) : "+
			"reteste la connexion dans les paramètres puis enregistre si ce changement est attendu", got[:16], c.pin[:min(16, len(c.pin))])
	}
	return nil
}

// Fingerprint renvoie l'empreinte du certificat épinglé ("" si aucun).
func (c *Client) Fingerprint() string {
	c.pinMu.Lock()
	defer c.pinMu.Unlock()
	return c.pin
}

// cancelBody libère le contexte de la requête à la fermeture du corps.
type cancelBody struct {
	io.ReadCloser
	cancel context.CancelFunc
}

func (b cancelBody) Close() error {
	err := b.ReadCloser.Close()
	b.cancel()
	return err
}

func (c *Client) do(method, path string, body any, timeout time.Duration) (*http.Response, error) {
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		rd = bytes.NewReader(b)
	}
	ctx, cancel := context.WithTimeout(c.ctx, timeout)
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, rd)
	if err != nil {
		cancel()
		return nil, err
	}
	req.Header.Set("x-api-key", c.key)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.http.Do(req)
	if err != nil {
		cancel()
		return nil, err
	}
	resp.Body = cancelBody{resp.Body, cancel}
	switch resp.StatusCode {
	case 401:
		resp.Body.Close()
		return nil, errUnauthorized
	case 403:
		resp.Body.Close()
		return nil, errForbidden
	}
	return resp, nil
}

func (c *Client) getJSON(method, path string, body, out any) error {
	resp, err := c.do(method, path, body, apiTimeout)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 && resp.StatusCode != 201 {
		msg, _ := io.ReadAll(io.LimitReader(resp.Body, 300))
		return fmt.Errorf("%s %s : HTTP %d %s", method, path, resp.StatusCode, bytes.TrimSpace(msg))
	}
	return json.NewDecoder(resp.Body).Decode(out)
}

// AlbumImages renvoie les images (hors corbeille) d'un album.
func (c *Client) AlbumImages(albumID string) (name string, imgs []Asset, err error) {
	var alb struct {
		AlbumName  string  `json:"albumName"`
		AssetCount int     `json:"assetCount"`
		Assets     []Asset `json:"assets"`
	}
	if err := c.getJSON("GET", "/api/albums/"+url.PathEscape(albumID), nil, &alb); err != nil {
		return "", nil, err
	}
	all := alb.Assets
	// Les versions récentes d'Immich peuvent ne plus renvoyer les assets avec l'album :
	// on passe alors par la recherche par métadonnées.
	if len(all) == 0 && alb.AssetCount > 0 {
		all, err = c.searchAlbum(albumID)
		if err != nil {
			return alb.AlbumName, nil, err
		}
	}
	for _, a := range all {
		// L'identifiant sert de nom de fichier (cache) et dans des URL : UUID strict uniquement.
		if a.Type == "IMAGE" && !a.IsTrashed && isUUID(a.ID) {
			imgs = append(imgs, a)
		}
	}
	return alb.AlbumName, imgs, nil
}

func (c *Client) searchAlbum(albumID string) ([]Asset, error) {
	var out []Asset
	for page := 1; page <= 200; page++ {
		var res struct {
			Assets struct {
				Items    []Asset `json:"items"`
				NextPage *string `json:"nextPage"`
			} `json:"assets"`
		}
		body := map[string]any{"albumIds": []string{albumID}, "type": "IMAGE", "page": page, "size": 1000}
		if err := c.getJSON("POST", "/api/search/metadata", body, &res); err != nil {
			return nil, err
		}
		out = append(out, res.Assets.Items...)
		if res.Assets.NextPage == nil || *res.Assets.NextPage == "" {
			break
		}
	}
	return out, nil
}

// Download récupère l'image dans la taille demandée (fullsize retombe sur preview si absent).
func (c *Client) Download(assetID, size string) ([]byte, error) {
	path := "/api/assets/" + url.PathEscape(assetID) + "/thumbnail?size=" + size
	if size == "original" {
		path = "/api/assets/" + url.PathEscape(assetID) + "/original"
	}
	resp, err := c.do("GET", path, nil, downloadTimeout)
	if err != nil {
		// « fullsize » peut rediriger vers l'original, qui exige asset.download :
		// sans cette permission, on se rabat sur l'aperçu plutôt que d'échouer.
		if size == "fullsize" && errors.Is(err, errForbidden) {
			log.Printf("fullsize refusé (permission asset.download absente ?), utilisation de l'aperçu")
			return c.Download(assetID, "preview")
		}
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		if size == "fullsize" && (resp.StatusCode == 400 || resp.StatusCode == 404) {
			return c.Download(assetID, "preview")
		}
		return nil, fmt.Errorf("téléchargement %s : HTTP %d", assetID, resp.StatusCode)
	}
	return io.ReadAll(io.LimitReader(resp.Body, 200<<20))
}

type AssetInfo struct {
	LocalDateTime string `json:"localDateTime"`
	Exif          struct {
		City             string `json:"city"`
		State            string `json:"state"`
		Country          string `json:"country"`
		DateTimeOriginal string `json:"dateTimeOriginal"`
	} `json:"exifInfo"`
}

func (c *Client) AssetInfo(id string) (AssetInfo, error) {
	var info AssetInfo
	err := c.getJSON("GET", "/api/assets/"+url.PathEscape(id), nil, &info)
	return info, err
}

type AlbumInfo struct {
	ID                         string `json:"id"`
	AlbumName                  string `json:"albumName"`
	AssetCount                 int    `json:"assetCount"`
	UpdatedAt                  string `json:"updatedAt"`
	LastModifiedAssetTimestamp string `json:"lastModifiedAssetTimestamp"`
}

// AlbumSignatures renvoie, pour chaque album accessible, une empreinte qui change dès qu'une
// photo est ajoutée, retirée ou modifiée (une seule requête pour tous les albums).
func (c *Client) AlbumSignatures() (map[string]string, error) {
	albums, err := c.Albums()
	if err != nil {
		return nil, err
	}
	sigs := make(map[string]string, len(albums))
	for _, a := range albums {
		sigs[a.ID] = fmt.Sprintf("%d|%s|%s", a.AssetCount, a.LastModifiedAssetTimestamp, a.UpdatedAt)
	}
	return sigs, nil
}

// Albums liste les albums possédés et partagés avec l'utilisateur du token.
func (c *Client) Albums() ([]AlbumInfo, error) {
	var own, shared []AlbumInfo
	if err := c.getJSON("GET", "/api/albums", nil, &own); err != nil {
		return nil, err
	}
	if err := c.getJSON("GET", "/api/albums?shared=true", nil, &shared); err != nil {
		return own, nil // pas bloquant
	}
	seen := map[string]bool{}
	var out []AlbumInfo
	for _, a := range append(own, shared...) {
		if !seen[a.ID] {
			seen[a.ID] = true
			out = append(out, a)
		}
	}
	return out, nil
}

func (c *Client) WebURL(assetID string) string {
	return c.base + "/photos/" + assetID
}
