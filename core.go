package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/jpeg"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	_ "image/gif"
	_ "image/png"

	_ "golang.org/x/image/webp"
)

// Fréquence de vérification des albums (une requête légère ; le contenu n'est rechargé que s'il a changé).
const albumCheckEvery = 30 * time.Minute

var errNotLandscape = errors.New("image en portrait")

// Monitor est un écran cible (mode « une image par écran »). ID = chemin de périphérique Windows.
type Monitor struct {
	ID string
	Screen
}

// Photo : image prête à être rendue (déjà redressée et convertie en sRGB).
type Photo struct {
	Asset Asset
	Img   image.Image
	Info  AssetInfo
}

type albumState struct {
	sig    string
	name   string
	assets []Asset
}

// Core contient la logique indépendante de Windows (testable sous Linux).
type Core struct {
	mu            sync.Mutex
	cfg           *Config
	client        *Client
	albums        map[string]*albumState
	albumsChecked time.Time
	assets        []Asset
	recent        []string
	rnd           *rand.Rand
	dir           string
	flip          bool
	last          Asset
	lastOK        bool
	lockErr       error
	fonts         *Fonts

	// Fonctions fournies par la plateforme (Windows) ; remplacées par des faux dans les tests.
	setWallpaper         func(path, style string) error
	setMonitorWallpapers func(paths map[string]string, style string) error
	monitors             func() []Monitor
	setLockScreen        func(path string) error
	screen               func(style string) Screen
	afterApply           func(cfg *Config, a Asset, webURL string)
}

func newCore(dir string, setWallpaper func(path, style string) error) *Core {
	return &Core{
		dir:          dir,
		setWallpaper: setWallpaper,
		albums:       map[string]*albumState{},
		rnd:          rand.New(rand.NewSource(time.Now().UnixNano())),
	}
}

func (c *Core) SetConfig(cfg *Config) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.cfg == nil || c.cfg.ServerURL != cfg.ServerURL || c.cfg.APIKey != cfg.APIKey {
		c.albums = map[string]*albumState{} // autre serveur ou autre compte : on repart de zéro
	}
	c.cfg = cfg
	c.client = newClient(cfg)
	c.assets = nil
	c.albumsChecked = time.Time{}
}

// LockError renvoie la dernière erreur de l'écran de verrouillage (nil si OK).
func (c *Core) LockError() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.lockErr
}

func (c *Core) InvalidateAlbums() {
	c.mu.Lock()
	c.albums = map[string]*albumState{}
	c.assets = nil
	c.albumsChecked = time.Time{}
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

// ---------- albums (rechargement incrémental) ----------

// refreshAlbums interroge la liste des albums (une requête) et ne recharge le contenu
// que des albums dont la signature (nombre de photos, dates de modification) a changé.
func (c *Core) refreshAlbums() error {
	if c.assets != nil && time.Since(c.albumsChecked) < albumCheckEvery {
		return nil
	}
	sigs, err := c.client.AlbumSignatures()
	if err != nil {
		if errors.Is(err, errUnauthorized) || c.assets == nil {
			return err
		}
		log.Printf("vérification des albums impossible, liste précédente conservée : %v", err)
		return nil
	}
	changed := c.assets == nil
	for _, id := range c.cfg.Albums {
		sig, known := sigs[id]
		if st := c.albums[id]; st != nil && known && st.sig == sig {
			continue
		}
		name, imgs, err := c.client.AlbumImages(id)
		if err != nil {
			if errors.Is(err, errUnauthorized) {
				return err
			}
			log.Printf("album %s : %v", id, err)
			continue
		}
		log.Printf("album %q : %d images (rechargé)", name, len(imgs))
		c.albums[id] = &albumState{sig: sig, name: name, assets: imgs}
		changed = true
	}
	c.albumsChecked = time.Now()
	if !changed {
		return nil
	}
	seen := map[string]bool{}
	var all []Asset
	for _, id := range c.cfg.Albums {
		if st := c.albums[id]; st != nil {
			for _, a := range st.assets {
				if !seen[a.ID] {
					seen[a.ID] = true
					all = append(all, a)
				}
			}
		}
	}
	if len(all) == 0 {
		return errors.New("aucune image trouvée dans les albums configurés")
	}
	c.assets = all
	return nil
}

// ---------- choix ----------

// order mélange les assets en repoussant à la fin ceux affichés récemment.
func (c *Core) order(list []Asset) []Asset {
	recent := map[string]bool{}
	for _, id := range c.recent {
		recent[id] = true
	}
	var fresh, old []Asset
	for _, a := range list {
		if recent[a.ID] {
			old = append(old, a)
		} else {
			fresh = append(fresh, a)
		}
	}
	c.rnd.Shuffle(len(fresh), func(i, j int) { fresh[i], fresh[j] = fresh[j], fresh[i] })
	c.rnd.Shuffle(len(old), func(i, j int) { old[i], old[j] = old[j], old[i] })
	return append(fresh, old...)
}

func (c *Core) remember(id string, pool, keepMin int) {
	max := max(min(pool/2, 100), keepMin)
	c.recent = append(c.recent, id)
	if len(c.recent) > max {
		c.recent = c.recent[len(c.recent)-max:]
	}
}

func (c *Core) wantInfo() bool { return c.cfg.ShowPlace || c.cfg.ShowDate }

// fetchPhotos télécharge n photos distinctes depuis Immich.
func (c *Core) fetchPhotos(n int) ([]Photo, error) {
	if err := c.refreshAlbums(); err != nil {
		return nil, err
	}
	var photos, portraits []Photo
	tries := 0
	var lastErr error
	for _, a := range c.order(c.assets) {
		if len(photos) >= n || tries >= 15*n {
			break
		}
		tries++
		data, err := c.client.Download(a.ID, c.cfg.ImageSize)
		if err != nil {
			if errors.Is(err, errUnauthorized) {
				return nil, err
			}
			lastErr = err
			log.Printf("%v", err)
			continue
		}
		_, perr := prepareImage(data, c.cfg.LandscapeOnly)
		if perr != nil && !errors.Is(perr, errNotLandscape) {
			log.Printf("%s (%s) : %v", a.ID, a.OriginalFileName, perr)
			continue
		}
		img, _, _, err := decodeNormalized(data)
		if err != nil {
			log.Printf("%s : %v", a.ID, err)
			continue
		}
		p := Photo{Asset: a, Img: img}
		if c.wantInfo() {
			if info, err := c.client.AssetInfo(a.ID); err == nil {
				p.Info = info
			} else {
				log.Printf("infos %s : %v", a.ID, err)
			}
		}
		c.saveToCache(p)
		if errors.Is(perr, errNotLandscape) {
			portraits = append(portraits, p)
			continue
		}
		photos = append(photos, p)
	}
	for len(photos) < n && len(portraits) > 0 { // pas assez de paysages : on complète
		photos, portraits = append(photos, portraits[0]), portraits[1:]
	}
	if len(photos) == 0 {
		if lastErr != nil {
			return nil, lastErr
		}
		return nil, fmt.Errorf("aucune image utilisable après %d essais (voir le journal)", tries)
	}
	return photos, nil
}

// ---------- cache hors ligne ----------

func (c *Core) cacheDir() string { return filepath.Join(c.dir, "cache") }

type cacheMeta struct {
	Asset Asset     `json:"asset"`
	Info  AssetInfo `json:"info"`
}

// saveToCache garde une copie (redimensionnée à 3840 px max) pour l'affichage hors ligne.
func (c *Core) saveToCache(p Photo) {
	if c.cfg.OfflineCache <= 0 {
		return
	}
	dir := c.cacheDir()
	os.MkdirAll(dir, 0o755)
	img := p.Img
	if b := img.Bounds(); max(b.Dx(), b.Dy()) > 3840 {
		s := 3840 / float64(max(b.Dx(), b.Dy()))
		img = compose(img, Screen{W: int(float64(b.Dx()) * s), H: int(float64(b.Dy()) * s)}, "stretch")
	}
	jpg, err := encodeJPEG(img)
	if err != nil {
		return
	}
	meta, _ := json.Marshal(cacheMeta{p.Asset, p.Info})
	base := filepath.Join(dir, p.Asset.ID)
	if os.WriteFile(base+".jpg", jpg, 0o644) == nil {
		os.WriteFile(base+".json", meta, 0o644)
	}
	c.pruneCache()
}

func (c *Core) cachedIDs() []string {
	entries, _ := os.ReadDir(c.cacheDir())
	type item struct {
		id string
		t  time.Time
	}
	var items []item
	for _, e := range entries {
		if id, ok := strings.CutSuffix(e.Name(), ".json"); ok {
			if info, err := e.Info(); err == nil {
				items = append(items, item{id, info.ModTime()})
			}
		}
	}
	sort.Slice(items, func(i, j int) bool { return items[i].t.After(items[j].t) })
	ids := make([]string, len(items))
	for i, it := range items {
		ids[i] = it.id
	}
	return ids
}

func (c *Core) pruneCache() {
	ids := c.cachedIDs()
	for _, id := range ids[min(len(ids), c.cfg.OfflineCache):] {
		os.Remove(filepath.Join(c.cacheDir(), id+".jpg"))
		os.Remove(filepath.Join(c.cacheDir(), id+".json"))
	}
}

// cachedPhotos tire n photos du cache local (Immich injoignable).
func (c *Core) cachedPhotos(n int) ([]Photo, error) {
	var pool []Asset
	for _, id := range c.cachedIDs() {
		pool = append(pool, Asset{ID: id})
	}
	var photos []Photo
	for _, a := range c.order(pool) {
		if len(photos) >= n {
			break
		}
		base := filepath.Join(c.cacheDir(), a.ID)
		var meta cacheMeta
		mb, err1 := os.ReadFile(base + ".json")
		jb, err2 := os.ReadFile(base + ".jpg")
		if err1 != nil || err2 != nil || json.Unmarshal(mb, &meta) != nil {
			continue
		}
		img, err := jpeg.Decode(bytes.NewReader(jb))
		if err != nil {
			continue
		}
		photos = append(photos, Photo{Asset: meta.Asset, Img: img, Info: meta.Info})
	}
	if len(photos) == 0 {
		return nil, errors.New("cache hors ligne vide")
	}
	return photos, nil
}

// ---------- application ----------

// Result décrit le dernier changement.
type Result struct {
	Asset   Asset
	Offline bool  // photo tirée du cache local
	Cause   error // pourquoi on est passé hors ligne
}

// Next choisit et applique de nouvelles images (une par écran si demandé).
func (c *Core) Next() (Result, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.cfg == nil {
		return Result{}, errors.New("configuration incomplète : ouvre les paramètres")
	}

	var mons []Monitor
	if c.cfg.MultiMonitor == "different" && c.monitors != nil && c.setMonitorWallpapers != nil {
		if m := c.monitors(); len(m) > 1 {
			mons = m
		}
	}
	n := max(1, len(mons))

	res := Result{}
	photos, err := c.fetchPhotos(n)
	if err != nil {
		cached, cerr := c.cachedPhotos(n)
		if cerr != nil {
			return Result{}, err
		}
		log.Printf("Immich injoignable (%v) : photo du cache hors ligne", err)
		photos, res.Offline, res.Cause = cached, true, err
	}
	for i, orig := 0, len(photos); len(photos) < n; i++ { // moins de photos que d'écrans : on réutilise
		photos = append(photos, photos[i%orig])
	}

	var primaryPath string
	if mons == nil {
		primaryPath, err = c.applySingle(photos[0])
	} else {
		primaryPath, err = c.applyMulti(photos, mons)
	}
	if err != nil {
		return Result{}, err
	}

	pool := len(c.assets)
	if res.Offline {
		pool = len(c.cachedIDs())
	}
	for _, p := range photos {
		c.remember(p.Asset.ID, pool, n)
		log.Printf("fond d'écran : %s (%s)", p.Asset.OriginalFileName, p.Asset.ID)
	}
	c.last, c.lastOK = photos[0].Asset, true
	if c.afterApply != nil {
		c.afterApply(c.cfg, photos[0].Asset, c.client.WebURL(photos[0].Asset.ID))
	}
	if c.cfg.LockScreen && c.setLockScreen != nil {
		// Écran de verrouillage : photo de l'écran principal ; erreur journalisée mais non bloquante.
		if err := c.setLockScreen(primaryPath); err != nil {
			log.Printf("écran de verrouillage : %v", err)
			c.lockErr = err
		} else {
			c.lockErr = nil
		}
	}
	res.Asset = photos[0].Asset
	return res, nil
}

func (c *Core) nextName(prefix string) string {
	s := "a"
	if c.flip {
		s = "b"
	}
	// On alterne entre deux fichiers : Windows ne rafraîchit pas toujours si le chemin ne change pas.
	return filepath.Join(c.dir, prefix+"_"+s+".jpg")
}

func (c *Core) applySingle(p Photo) (string, error) {
	style := c.cfg.Style
	var jpg []byte
	var err error
	if c.wantInfo() && c.screen != nil {
		sc := c.screen(c.cfg.Style)
		jpg, err = c.render(p, sc)
		style = "fill" // l'image fait déjà la taille de l'écran
	} else {
		jpg, err = encodeJPEG(p.Img)
	}
	if err != nil {
		return "", err
	}
	path := c.nextName("wallpaper")
	c.flip = !c.flip
	if err := os.WriteFile(path, jpg, 0o644); err != nil {
		return "", err
	}
	return path, c.setWallpaper(path, style)
}

// applyMulti rend chaque photo à la taille exacte de son écran puis les applique.
func (c *Core) applyMulti(photos []Photo, mons []Monitor) (string, error) {
	paths := map[string]string{}
	var primary string
	for i, m := range mons {
		jpg, err := c.render(photos[i], m.Screen)
		if err != nil {
			return "", err
		}
		path := c.nextName(fmt.Sprintf("wallpaper_ecran%d", i+1))
		if err := os.WriteFile(path, jpg, 0o644); err != nil {
			return "", err
		}
		paths[m.ID] = path
		if i == 0 {
			primary = path
		}
	}
	c.flip = !c.flip
	return primary, c.setMonitorWallpapers(paths, "fill")
}

// render recadre la photo à la taille de l'écran et y incruste lieu/date si demandé.
func (c *Core) render(p Photo, sc Screen) ([]byte, error) {
	if sc.W < 320 || sc.H < 240 {
		return nil, fmt.Errorf("taille d'écran inattendue %dx%d", sc.W, sc.H)
	}
	dst := compose(p.Img, sc, c.cfg.Style)
	if c.wantInfo() {
		place, date := captionParts(p.Info, c.cfg.ShowPlace, c.cfg.ShowDate)
		if c.fonts == nil {
			c.fonts = loadFonts()
		}
		drawCaption(dst, c.fonts, place, date, c.cfg.CaptionPosition, sc.BottomInset)
	}
	return encodeJPEG(dst)
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
