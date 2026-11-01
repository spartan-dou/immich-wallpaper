package main

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/jpeg"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"sync"
	"time"

	_ "image/gif"
	_ "image/png"

	_ "golang.org/x/image/webp"
)

const albumRefresh = 6 * time.Hour

var errNotLandscape = errors.New("image en portrait")

// Core contient la logique indépendante de Windows (testable sous Linux).
type Core struct {
	mu       sync.Mutex
	cfg      *Config
	client   *Client
	assets   []Asset
	assetsAt time.Time
	recent   []string
	rnd      *rand.Rand
	dir      string
	flip     bool
	last     Asset
	lastOK   bool

	setWallpaper  func(path, style string) error
	setLockScreen func(path string) error
	screen        func(style string) Screen
	fonts         *Fonts
	afterApply    func(cfg *Config, a Asset, webURL string)
	lockErr       error
}

func newCore(dir string, setWallpaper func(path, style string) error) *Core {
	return &Core{dir: dir, setWallpaper: setWallpaper, rnd: rand.New(rand.NewSource(time.Now().UnixNano()))}
}

func (c *Core) SetConfig(cfg *Config) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.cfg = cfg
	c.client = newClient(cfg)
	c.assets = nil // force le rechargement des albums
}

// LockError renvoie la dernière erreur de l'écran de verrouillage (nil si OK).
func (c *Core) LockError() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.lockErr
}

func (c *Core) InvalidateAlbums() {
	c.mu.Lock()
	c.assets = nil
	c.mu.Unlock()
}

func (c *Core) Last() (Asset, string, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if !c.lastOK || c.client == nil {
		return Asset{}, "", false
	}
	return c.last, c.client.WebURL(c.last.ID), true
}

func (c *Core) loadAssets() error {
	seen := map[string]bool{}
	var all []Asset
	for _, id := range c.cfg.Albums {
		name, imgs, err := c.client.AlbumImages(id)
		if err != nil {
			if errors.Is(err, errUnauthorized) {
				return err
			}
			log.Printf("album %s : %v", id, err)
			continue
		}
		log.Printf("album %q : %d images", name, len(imgs))
		for _, a := range imgs {
			if !seen[a.ID] {
				seen[a.ID] = true
				all = append(all, a)
			}
		}
	}
	if len(all) == 0 {
		return errors.New("aucune image trouvée dans les albums configurés")
	}
	c.assets = all
	c.assetsAt = time.Now()
	return nil
}

// order mélange les assets en repoussant à la fin ceux affichés récemment.
func (c *Core) order() []Asset {
	recent := map[string]bool{}
	for _, id := range c.recent {
		recent[id] = true
	}
	var fresh, old []Asset
	for _, a := range c.assets {
		if recent[a.ID] {
			old = append(old, a)
		} else {
			fresh = append(fresh, a)
		}
	}
	c.rnd.Shuffle(len(fresh), func(i, j int) { fresh[i], fresh[j] = fresh[j], fresh[i] })
	return append(fresh, old...)
}

func (c *Core) remember(id string) {
	max := len(c.assets) / 2
	if max > 100 {
		max = 100
	}
	c.recent = append(c.recent, id)
	if len(c.recent) > max {
		c.recent = c.recent[len(c.recent)-max:]
	}
}

// Next choisit, télécharge et applique une nouvelle image.
func (c *Core) Next() (Asset, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.cfg == nil {
		return Asset{}, errors.New("configuration incomplète : ouvre les paramètres")
	}
	if c.assets == nil || time.Since(c.assetsAt) > albumRefresh {
		if err := c.loadAssets(); err != nil {
			return Asset{}, err
		}
	}

	var fallback []byte
	var fallbackAsset Asset
	tries := 0
	for _, a := range c.order() {
		if tries >= 15 {
			break
		}
		tries++
		data, err := c.client.Download(a.ID, c.cfg.ImageSize)
		if err != nil {
			if errors.Is(err, errUnauthorized) {
				return Asset{}, err
			}
			log.Printf("%v", err)
			continue
		}
		jpg, err := prepareImage(data, c.cfg.LandscapeOnly)
		if errors.Is(err, errNotLandscape) {
			if fallback == nil {
				fallback, fallbackAsset = jpg, a
			}
			continue
		}
		if err != nil {
			log.Printf("%s (%s) : %v", a.ID, a.OriginalFileName, err)
			continue
		}
		return a, c.apply(a, jpg)
	}
	if fallback != nil {
		log.Printf("aucune image paysage trouvée, on prend une portrait")
		return fallbackAsset, c.apply(fallbackAsset, fallback)
	}
	return Asset{}, fmt.Errorf("aucune image utilisable après %d essais (voir le journal)", tries)
}

func (c *Core) apply(a Asset, data []byte) error {
	style := c.cfg.Style
	// Décodage + redressement EXIF + conversion du profil couleur (Display P3…) vers sRGB.
	img, _, changed, err := decodeNormalized(data)
	if err != nil {
		return err
	}
	jpg := data
	captioned := false
	if (c.cfg.ShowPlace || c.cfg.ShowDate) && c.screen != nil {
		if out, err := c.withCaption(a, img); err != nil {
			log.Printf("légende ignorée : %v", err)
		} else {
			jpg, style, captioned = out, "fill", true // l'image fait déjà la taille de l'écran
		}
	}
	if !captioned && changed {
		if jpg, err = encodeJPEG(img); err != nil {
			return err
		}
	}
	// On alterne entre deux fichiers : Windows ne rafraîchit pas toujours si le chemin ne change pas.
	name := "wallpaper_a.jpg"
	if c.flip {
		name = "wallpaper_b.jpg"
	}
	c.flip = !c.flip
	path := filepath.Join(c.dir, name)
	if err := os.WriteFile(path, jpg, 0o644); err != nil {
		return err
	}
	if err := c.setWallpaper(path, style); err != nil {
		return err
	}
	c.remember(a.ID)
	c.last, c.lastOK = a, true
	log.Printf("fond d'écran : %s (%s)", a.OriginalFileName, a.ID)
	if c.afterApply != nil {
		c.afterApply(c.cfg, a, c.client.WebURL(a.ID))
	}
	if c.cfg.LockScreen && c.setLockScreen != nil {
		// Écran de verrouillage : erreur journalisée mais non bloquante.
		if err := c.setLockScreen(path); err != nil {
			log.Printf("écran de verrouillage : %v", err)
			c.lockErr = err
		} else {
			c.lockErr = nil
		}
	}
	return nil
}

// withCaption recadre l'image à la taille de l'écran et y incruste lieu/date.
func (c *Core) withCaption(a Asset, img image.Image) ([]byte, error) {
	info, err := c.client.AssetInfo(a.ID)
	if err != nil {
		return nil, err
	}
	place, date := captionParts(info, c.cfg.ShowPlace, c.cfg.ShowDate)
	sc := c.screen(c.cfg.Style)
	if sc.W < 320 || sc.H < 240 {
		return nil, fmt.Errorf("taille d'écran inattendue %dx%d", sc.W, sc.H)
	}
	dst := compose(img, sc, c.cfg.Style)
	if c.fonts == nil {
		c.fonts = loadFonts()
	}
	drawCaption(dst, c.fonts, place, date, c.cfg.CaptionPosition, sc.BottomInset)
	out, err := encodeJPEG(dst)
	if err != nil {
		return nil, err
	}
	if place != "" || date != "" {
		log.Printf("légende : %s / %s", place, date)
	}
	return out, nil
}

// prepareImage vérifie que l'image est lisible et son orientation (EXIF compris).
// En cas de portrait, renvoie quand même les données (utilisables en secours) avec errNotLandscape.
func prepareImage(data []byte, landscapeOnly bool) ([]byte, error) {
	cfg, _, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("format non pris en charge (HEIC/RAW ? utilise image_size=fullsize) : %w", err)
	}
	w, h := orientedSize(data, cfg.Width, cfg.Height)
	if landscapeOnly && w <= h {
		return data, errNotLandscape
	}
	return data, nil
}

func encodeJPEG(img image.Image) ([]byte, error) {
	var buf bytes.Buffer
	err := jpeg.Encode(&buf, img, &jpeg.Options{Quality: 95})
	return buf.Bytes(), err
}
