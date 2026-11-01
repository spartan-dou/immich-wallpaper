package main

import (
	"bytes"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"time"
)

var errUnauthorized = errors.New("token refusé par Immich (401/403) : vérifie api_key et ses permissions")

type Asset struct {
	ID               string `json:"id"`
	Type             string `json:"type"`
	IsTrashed        bool   `json:"isTrashed"`
	OriginalFileName string `json:"originalFileName"`
}

type Client struct {
	base string
	key  string
	http *http.Client
}

func newClient(cfg *Config) *Client {
	tr := http.DefaultTransport.(*http.Transport).Clone()
	if cfg.InsecureSkipVerify {
		tr.TLSClientConfig = &tls.Config{InsecureSkipVerify: true}
	}
	return &Client{
		base: cfg.ServerURL,
		key:  cfg.APIKey,
		http: &http.Client{Timeout: 2 * time.Minute, Transport: tr},
	}
}

func (c *Client) do(method, path string, body any) (*http.Response, error) {
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, c.base+path, rd)
	if err != nil {
		return nil, err
	}
	req.Header.Set("x-api-key", c.key)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == 401 || resp.StatusCode == 403 {
		resp.Body.Close()
		return nil, errUnauthorized
	}
	return resp, nil
}

func (c *Client) getJSON(method, path string, body, out any) error {
	resp, err := c.do(method, path, body)
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
		if a.Type == "IMAGE" && !a.IsTrashed {
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
	resp, err := c.do("GET", path, nil)
	if err != nil {
		// « fullsize » peut rediriger vers l'original, qui exige asset.download :
		// sans cette permission, on se rabat sur l'aperçu plutôt que d'échouer.
		if size == "fullsize" && errors.Is(err, errUnauthorized) {
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
