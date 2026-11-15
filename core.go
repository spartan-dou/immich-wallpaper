package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/draw"
	"image/jpeg"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"runtime/debug"
	"sort"
	"strings"
	"sync"
	"time"

	_ "image/gif"
	_ "image/png"

	xdraw "golang.org/x/image/draw"
	_ "golang.org/x/image/webp"
)

// Fréquence de vérification des albums (une requête légère ; le contenu n'est rechargé que s'il a changé).
const albumCheckEvery = 30 * time.Minute

// Côté maximal des photos gardées (cache hors ligne, photos affichées).
const keptMaxSide = 3840

// errNeedDraw : Reapply ne peut pas couvrir les écrans actuels avec les photos mémorisées.
var errNeedDraw = errors.New("pas assez de photos mémorisées pour les écrans actuels")

var errNoPrevious = errors.New("aucune photo précédente en mémoire")

// Nombre de fonds d'écran précédents gardés pour « Photo précédente ».
const historyDepth = 5

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
	jpg   []byte // version réduite (keptMaxSide) encodée en JPEG
}

// keptPhoto : photo affichée, gardée en JPEG (≈10× plus léger que l'image décodée).
type keptPhoto struct {
	Asset Asset
	Info  AssetInfo
	JPEG  []byte
}

type albumState struct {
	sig    string
	name   string
	assets []Asset
}

// Core contient la logique indépendante de Windows (testable sous Linux).
type Core struct {
	// run sérialise les opérations longues (Next, Reapply : réseau, rendu, PowerShell).
	// flip et fonts ne sont utilisés que sous ce verrou.
	run   sync.Mutex
	flip  bool
	fonts *Fonts

	// mu protège l'état ci-dessous ; il n'est jamais tenu pendant une entrée/sortie,
	// pour que le menu et la page de réglages ne soient pas bloqués par un tirage.
	mu            sync.Mutex
	cfg           *Config
	client        *Client
	gen           int // incrémenté à chaque changement de configuration
	albums        map[string]*albumState
	albumsChecked time.Time
	assets        []Asset
	recent        []string
	rnd           *rand.Rand
	last          Asset
	lastOK        bool
	lockErr       error
	lastPhotos    []keptPhoto     // photos affichées (compressées), réutilisées si les écrans changent
	history       [][]keptPhoto   // fonds d'écran précédents, du plus ancien au plus récent
	excluded      map[string]bool // photos que l'utilisateur ne veut plus voir
	preloaded     []preItem       // photos du prochain changement, déjà téléchargées (sur disque)
	preGen        int             // configuration pour laquelle elles ont été préparées
	preKey        string          // albums pour lesquels elles ont été préparées

	dir string
	now func() time.Time // horloge (remplaçable dans les tests) : règles d'albums par heure/saison

	// Fonctions fournies par la plateforme (Windows) ; remplacées par des faux dans les tests.
	setWallpaper         func(path, style string) error
	setMonitorWallpapers func(paths map[string]string, style string) error
	monitors             func() []Monitor
	setLockScreen        func(path string) error
	screen               func(style string) Screen
	afterApply           func(cfg *Config, a Asset, webURL string)
}

func newCore(dir string, setWallpaper func(path, style string) error) *Core {
	c := &Core{
		dir:          dir,
		setWallpaper: setWallpaper,
		albums:       map[string]*albumState{},
		excluded:     map[string]bool{},
		now:          time.Now,
		rnd:          rand.New(rand.NewSource(time.Now().UnixNano())),
	}
	if b, err := os.ReadFile(c.excludedPath()); err == nil {
		var ids []string
		if json.Unmarshal(b, &ids) == nil {
			for _, id := range ids {
				c.excluded[id] = true
			}
		}
	}
	os.RemoveAll(c.preloadDir()) // préchargement d'un lancement précédent : périmé
	// Historique des photos récentes : conservé d'un lancement à l'autre.
	if b, err := os.ReadFile(c.recentPath()); err == nil {
		var ids []string
		if json.Unmarshal(b, &ids) == nil && len(ids) <= 500 {
			c.recent = ids
		}
	}
	return c
}

func (c *Core) recentPath() string   { return filepath.Join(c.dir, "recent.json") }
func (c *Core) excludedPath() string { return filepath.Join(c.dir, "excluded.json") }
func (c *Core) preloadDir() string   { return filepath.Join(c.dir, "preload") }

func (c *Core) SetConfig(cfg *Config) {
	c.mu.Lock()
	old := c.client
	if c.cfg == nil || c.cfg.ServerURL != cfg.ServerURL || c.cfg.APIKey != cfg.APIKey {
		c.albums = map[string]*albumState{} // autre serveur ou autre compte : on repart de zéro
	}
	c.cfg = cfg
	c.client = newClient(cfg)
	c.gen++
	c.preloaded = nil
	c.assets = nil
	c.albumsChecked = time.Time{}
	c.mu.Unlock()
	if old != nil {
		old.Close() // annule les requêtes en cours de l'ancienne configuration
	}
}

// LockError renvoie la dernière erreur de l'écran de verrouillage (nil si OK).
func (c *Core) LockError() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.lockErr
}

func (c *Core) Last() (Asset, string, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if !c.lastOK || c.client == nil {
		return Asset{}, "", false
	}
	return c.last, c.client.WebURL(c.last.ID), true
}

// job : une opération longue, avec la configuration figée à son démarrage.
type job struct {
	c      *Core
	cfg    *Config
	client *Client
	gen    int
}

func (c *Core) begin() (*job, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.cfg == nil {
		return nil, errors.New("configuration incomplète : ouvre les paramètres")
	}
	return &job{c: c, cfg: c.cfg, client: c.client, gen: c.gen}, nil
}

// ---------- albums (rechargement incrémental) ----------

// refreshAlbums interroge la liste des albums (une requête) et ne recharge le contenu
// que des albums dont la signature (nombre de photos, dates de modification) a changé.
// Renvoie la liste des images à utiliser pour ce tirage.
func (j *job) refreshAlbums() ([]Asset, error) {
	c := j.c
	c.mu.Lock()
	loaded, checked := c.assets != nil, c.albumsChecked
	albums := make(map[string]*albumState, len(c.albums))
	for id, st := range c.albums {
		albums[id] = st
	}
	c.mu.Unlock()

	if loaded && time.Since(checked) < albumCheckEvery {
		return j.pick(albums)
	}
	sigs, err := j.client.AlbumSignatures()
	if err != nil {
		if isAuthErr(err) || !loaded {
			return nil, err
		}
		log.Printf("vérification des albums impossible, liste précédente conservée : %v", err)
		return j.pick(albums)
	}
	for _, id := range j.cfg.allAlbums() {
		sig, known := sigs[id]
		if st := albums[id]; st != nil && known && st.sig == sig {
			continue
		}
		name, imgs, err := j.client.AlbumImages(id)
		if err != nil {
			if errors.Is(err, errUnauthorized) {
				return nil, err
			}
			if !known && albums[id] != nil {
				// Ni listé ni lisible : album supprimé ou partage retiré.
				log.Printf("album %s inaccessible (%v) : retiré du tirage", id, err)
				delete(albums, id)
			} else {
				log.Printf("album %s : %v", id, err)
			}
			continue
		}
		log.Printf("album %q : %d images (rechargé)", name, len(imgs))
		albums[id] = &albumState{sig: sig, name: name, assets: imgs}
	}
	all := mergeAlbums(albums, j.cfg.allAlbums())

	c.mu.Lock()
	if c.gen == j.gen { // sinon la configuration a changé entre-temps : résultat non mémorisé
		c.albums, c.albumsChecked, c.assets = albums, time.Now(), all
	}
	c.mu.Unlock()
	return j.pick(albums)
}

// mergeAlbums réunit sans doublon les images des albums demandés.
func mergeAlbums(albums map[string]*albumState, ids []string) []Asset {
	seen := map[string]bool{}
	var all []Asset
	for _, id := range ids {
		if st := albums[id]; st != nil {
			for _, a := range st.assets {
				if !seen[a.ID] {
					seen[a.ID] = true
					all = append(all, a)
				}
			}
		}
	}
	return all
}

// pick renvoie les images des albums à utiliser maintenant : ceux des règles (saison, heure)
// qui s'appliquent ; à défaut, ou s'ils sont vides, les albums habituels.
func (j *job) pick(albums map[string]*albumState) ([]Asset, error) {
	for _, ids := range [][]string{j.cfg.albumsAt(j.c.now()), j.cfg.Albums, j.cfg.allAlbums()} {
		if list := mergeAlbums(albums, ids); len(list) > 0 {
			return list, nil
		}
	}
	return nil, errors.New("aucune image trouvée dans les albums configurés")
}

// albumKey identifie les albums utilisés à cet instant (validité du préchargement).
func (j *job) albumKey() string { return strings.Join(j.cfg.albumsAt(j.c.now()), ",") }

// ---------- choix ----------

// order mélange les assets en repoussant à la fin ceux affichés récemment,
// et écarte ceux que l'utilisateur ne veut plus voir.
func (c *Core) order(list []Asset) []Asset {
	c.mu.Lock()
	defer c.mu.Unlock()
	recent := map[string]bool{}
	for _, id := range c.recent {
		recent[id] = true
	}
	var fresh, old []Asset
	for _, a := range list {
		if c.excluded[a.ID] {
			continue
		}
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

// remember note une photo affichée (appelé sous c.mu).
func (c *Core) remember(id string, pool, keepMin int) {
	limit := max(min(pool/2, 100), keepMin)
	c.recent = append(c.recent, id)
	if len(c.recent) > limit {
		c.recent = c.recent[len(c.recent)-limit:]
	}
}

func (j *job) wantInfo() bool { return j.cfg.ShowPlace || j.cfg.ShowDate }

// safeDecode décode une image ; un panic du décodeur (fichier malformé) devient une erreur.
func safeDecode(decode func() (image.Image, error)) (img image.Image, err error) {
	defer func() {
		if r := recover(); r != nil {
			img, err = nil, fmt.Errorf("décodage interrompu : %v", r)
		}
	}()
	return decode()
}

// rawPhoto : image téléchargée, vérifiée par son en-tête mais pas encore décodée.
type rawPhoto struct {
	asset   Asset
	data    []byte
	meta    imageMeta
	info    AssetInfo
	hasInfo bool
}

// rawSource renvoie une source d'images téléchargées depuis Immich, une par appel.
// Rien n'est décodé ici : les portraits écartés (landscape_only) ne sont ni décodés ni mis
// en cache ; au plus n sont gardés compressés, en secours s'il manque des paysages.
func (j *job) rawSource(n int, assets []Asset) func() (rawPhoto, error) {
	order := j.c.order(assets)
	var portraits []rawPhoto
	var lastErr error
	i, tries := 0, 0
	return func() (rawPhoto, error) {
		for i < len(order) && tries < 15*n {
			a := order[i]
			i++
			tries++
			data, err := j.client.Download(a.ID, j.cfg.ImageSize)
			if err != nil {
				if errors.Is(err, errUnauthorized) {
					return rawPhoto{}, err
				}
				lastErr = err
				log.Printf("%v", err)
				continue
			}
			meta, err := probeImage(data)
			if err != nil {
				log.Printf("%s (%s) : %v", a.ID, a.OriginalFileName, err)
				continue
			}
			r := rawPhoto{asset: a, data: data, meta: meta}
			if j.cfg.LandscapeOnly && meta.W <= meta.H {
				if len(portraits) < n {
					portraits = append(portraits, r)
				}
				continue
			}
			return r, nil
		}
		if len(portraits) > 0 { // pas assez de paysages : on complète
			r := portraits[0]
			portraits = portraits[1:]
			return r, nil
		}
		if lastErr != nil {
			return rawPhoto{}, lastErr
		}
		return rawPhoto{}, fmt.Errorf("aucune image utilisable après %d essais (voir le journal)", tries)
	}
}

// fetchInfo complète la photo avec le lieu et la date (si la légende est demandée).
func (j *job) fetchInfo(r *rawPhoto) {
	if r.hasInfo || !j.wantInfo() {
		return
	}
	if info, err := j.client.AssetInfo(r.asset.ID); err == nil {
		r.info, r.hasInfo = info, true
	} else {
		log.Printf("infos %s : %v", r.asset.ID, err)
	}
}

// build décode l'image (sRGB, redressée) et la complète de ses informations.
func (j *job) build(r rawPhoto) (Photo, error) {
	img, err := safeDecode(func() (image.Image, error) { return r.meta.decode(r.data) })
	if err != nil {
		return Photo{}, err
	}
	j.fetchInfo(&r)
	return Photo{Asset: r.asset, Img: img, Info: r.info}, nil
}

// online renvoie une source de photos téléchargées depuis Immich et décodées, une par appel :
// seule la photo demandée est en mémoire.
func (j *job) online(n int, assets []Asset) func() (Photo, error) {
	next := j.rawSource(n, assets)
	return func() (Photo, error) {
		for {
			r, err := next()
			if err != nil {
				return Photo{}, err
			}
			p, err := j.build(r)
			if err != nil {
				log.Printf("%s : %v", r.asset.ID, err)
				continue
			}
			return p, nil
		}
	}
}

// ---------- préchargement ----------

// preItem : photo du prochain changement, déjà téléchargée et gardée sur disque (pas en RAM).
type preItem struct {
	asset   Asset
	info    AssetInfo
	hasInfo bool
	path    string
}

// Preload télécharge à l'avance les photos du prochain changement (et leurs informations) :
// le changement suivant n'a alors plus besoin du réseau — utile au déverrouillage du PC.
func (c *Core) Preload() (err error) {
	c.run.Lock()
	defer c.run.Unlock()
	defer debug.FreeOSMemory()
	defer guard("le préchargement", &err)
	j, err := c.begin()
	if err != nil || !j.cfg.Preload {
		return err
	}
	_, _, n := j.targets()
	key := j.albumKey()
	assets, err := j.refreshAlbums()
	if err != nil {
		return err
	}
	dir := c.preloadDir()
	os.RemoveAll(dir)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	next := j.rawSource(n, assets)
	var items []preItem
	for len(items) < n {
		r, err := next()
		if err != nil {
			if len(items) == 0 {
				return err
			}
			break
		}
		j.fetchInfo(&r)
		path := filepath.Join(dir, r.asset.ID+".img")
		if err := os.WriteFile(path, r.data, 0o644); err != nil {
			return err
		}
		items = append(items, preItem{r.asset, r.info, r.hasInfo, path})
	}
	c.mu.Lock()
	if c.gen == j.gen {
		c.preloaded, c.preGen, c.preKey = items, j.gen, key
	}
	c.mu.Unlock()
	log.Printf("photo suivante préchargée (%d)", len(items))
	return nil
}

// takePreloaded renvoie les photos préchargées si elles valent encore pour ce tirage
// (même configuration, mêmes albums, photos non exclues depuis).
func (j *job) takePreloaded() []preItem {
	c := j.c
	c.mu.Lock()
	items, ok := c.preloaded, c.preGen == j.gen && c.preKey == j.albumKey()
	c.preloaded = nil
	var keep []preItem
	for _, it := range items {
		if ok && !c.excluded[it.asset.ID] {
			keep = append(keep, it)
		} else {
			os.Remove(it.path)
		}
	}
	c.mu.Unlock()
	return keep
}

// fromPreload décode une photo préchargée ; son fichier est supprimé.
func (j *job) fromPreload(it preItem) (Photo, error) {
	data, err := os.ReadFile(it.path)
	os.Remove(it.path)
	if err != nil {
		return Photo{}, err
	}
	meta, err := probeImage(data)
	if err != nil {
		return Photo{}, err
	}
	return j.build(rawPhoto{asset: it.asset, data: data, meta: meta, info: it.info, hasInfo: it.hasInfo})
}

// ---------- cache hors ligne ----------

func (c *Core) cacheDir() string { return filepath.Join(c.dir, "cache") }

type cacheMeta struct {
	Asset Asset     `json:"asset"`
	Info  AssetInfo `json:"info"`
}

// saveToCache garde une copie (redimensionnée à 3840 px max) pour l'affichage hors ligne.
func (j *job) saveToCache(p Photo) {
	if j.cfg.OfflineCache <= 0 || !isUUID(p.Asset.ID) {
		return
	}
	dir := j.c.cacheDir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		log.Printf("cache hors ligne : %v", err)
		return
	}
	meta, _ := json.Marshal(cacheMeta{p.Asset, p.Info})
	base := filepath.Join(dir, p.Asset.ID)
	err := os.WriteFile(base+".jpg", p.jpg, 0o644)
	if err == nil {
		err = os.WriteFile(base+".json", meta, 0o644)
	}
	if err != nil {
		log.Printf("cache hors ligne : %v", err)
	}
	j.pruneCache()
}

// shrinkJPEG encode l'image en JPEG, redimensionnée à 3840 px max.
func shrinkJPEG(img image.Image) ([]byte, error) {
	if b := img.Bounds(); max(b.Dx(), b.Dy()) > keptMaxSide {
		s := keptMaxSide / float64(max(b.Dx(), b.Dy()))
		dst := image.NewRGBA(image.Rect(0, 0, max(1, int(float64(b.Dx())*s)), max(1, int(float64(b.Dy())*s))))
		xdraw.BiLinear.Scale(dst, dst.Bounds(), img, b, draw.Src, nil)
		img = dst
	}
	return encodeJPEG(img)
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

func (j *job) pruneCache() {
	ids := j.c.cachedIDs()
	for _, id := range ids[min(len(ids), j.cfg.OfflineCache):] {
		os.Remove(filepath.Join(j.c.cacheDir(), id+".jpg"))
		os.Remove(filepath.Join(j.c.cacheDir(), id+".json"))
	}
}

// cached renvoie une source de photos tirées du cache local (Immich injoignable).
// Avec landscape_only, les portraits du cache ne servent qu'en dernier recours.
func (j *job) cached() func() (Photo, error) {
	var pool []Asset
	for _, id := range j.c.cachedIDs() {
		pool = append(pool, Asset{ID: id})
	}
	order := j.c.order(pool)
	var portraits []string
	i := 0

	load := func(id string, skipPortrait bool) (Photo, bool) {
		base := filepath.Join(j.c.cacheDir(), id)
		var meta cacheMeta
		mb, err1 := os.ReadFile(base + ".json")
		jb, err2 := os.ReadFile(base + ".jpg")
		if err1 != nil || err2 != nil || json.Unmarshal(mb, &meta) != nil {
			return Photo{}, false
		}
		if skipPortrait {
			if ic, err := jpeg.DecodeConfig(bytes.NewReader(jb)); err == nil && ic.Width <= ic.Height {
				portraits = append(portraits, id)
				return Photo{}, false
			}
		}
		img, err := jpeg.Decode(bytes.NewReader(jb))
		if err != nil {
			return Photo{}, false
		}
		return Photo{Asset: meta.Asset, Img: img, Info: meta.Info, jpg: jb}, true
	}

	return func() (Photo, error) {
		for i < len(order) {
			id := order[i].ID
			i++
			if p, ok := load(id, j.cfg.LandscapeOnly); ok {
				return p, nil
			}
		}
		for len(portraits) > 0 {
			id := portraits[0]
			portraits = portraits[1:]
			if p, ok := load(id, false); ok {
				return p, nil
			}
		}
		return Photo{}, errors.New("cache hors ligne vide")
	}
}

// ---------- distribution des photos aux écrans ----------

// feeder fournit les photos une par une au rendu : chaque écran est traité entièrement
// (téléchargement → décodage → rendu → écriture) avant de passer au suivant, et seules les
// versions compressées sont conservées entre deux écrans.
type feeder struct {
	j        *job
	src      func() (Photo, error)
	cache    bool        // photos venant d'Immich : à copier dans le cache hors ligne
	head     *Photo      // photo déjà obtenue, à servir en premier
	distinct []keptPhoto // photos différentes obtenues
	shown    []keptPhoto // photos dans l'ordre d'affichage (répétitions comprises)
	done     bool
}

func (f *feeder) next() (Photo, error) {
	if f.head != nil {
		p := *f.head
		f.head = nil
		f.shown = append(f.shown, keptPhoto{p.Asset, p.Info, p.jpg})
		return p, nil
	}
	if !f.done {
		p, err := f.fetch()
		if err == nil {
			f.shown = append(f.shown, keptPhoto{p.Asset, p.Info, p.jpg})
			return p, nil
		}
		f.done = true
		if len(f.distinct) == 0 {
			return Photo{}, err
		}
		log.Printf("moins de photos que d'emplacements (%v) : réutilisation", err)
	}
	// Moins de photos que d'écrans : on réutilise celles déjà obtenues.
	k := f.distinct[(len(f.shown)-len(f.distinct))%len(f.distinct)]
	img, err := jpeg.Decode(bytes.NewReader(k.JPEG))
	if err != nil {
		return Photo{}, err
	}
	f.shown = append(f.shown, k)
	return Photo{Asset: k.Asset, Img: img, Info: k.Info, jpg: k.JPEG}, nil
}

// fetch tire une nouvelle photo de la source et calcule (une seule fois) sa version réduite,
// partagée entre le cache hors ligne et les photos mémorisées.
func (f *feeder) fetch() (Photo, error) {
	p, err := f.src()
	if err != nil {
		return Photo{}, err
	}
	if p.jpg == nil {
		if p.jpg, err = shrinkJPEG(p.Img); err != nil {
			return Photo{}, err
		}
		if f.cache {
			f.j.saveToCache(p)
		}
	}
	f.distinct = append(f.distinct, keptPhoto{p.Asset, p.Info, p.jpg})
	return p, nil
}

// ---------- application ----------

// Result décrit le dernier changement.
type Result struct {
	Asset   Asset
	Offline bool  // photo tirée du cache local
	Cause   error // pourquoi on est passé hors ligne
}

// targets détermine les écrans à couvrir et le nombre de photos nécessaires
// (une par écran, deux pour un écran très large découpé en deux panneaux).
func (j *job) targets() (mons []Monitor, single Screen, n int) {
	c := j.c
	if j.cfg.MultiMonitor == "different" && c.monitors != nil && c.setMonitorWallpapers != nil {
		if m := c.monitors(); len(m) > 1 {
			mons = m
		}
	}
	if mons == nil {
		if c.screen != nil {
			single = c.screen(j.cfg.Style)
		}
		return nil, single, j.panels(single)
	}
	for _, m := range mons {
		n += j.panels(m.Screen)
	}
	return mons, single, n
}

func (j *job) applyTo(next func() (Photo, error), mons []Monitor, single Screen) (string, error) {
	if mons == nil {
		return j.applySingle(next, single)
	}
	return j.applyMulti(next, mons)
}

func take(next func() (Photo, error), n int) ([]Photo, error) {
	photos := make([]Photo, 0, n)
	for len(photos) < n {
		p, err := next()
		if err != nil {
			return nil, err
		}
		photos = append(photos, p)
	}
	return photos, nil
}

// guard transforme un panic (décodeur, rendu) en erreur journalisée : l'appli est un binaire
// sans console, un panic non récupéré la ferait disparaître sans message.
func guard(what string, err *error) {
	if r := recover(); r != nil {
		log.Printf("panic pendant %s : %v\n%s", what, r, debug.Stack())
		*err = fmt.Errorf("erreur interne pendant %s : %v (voir le journal)", what, r)
	}
}

// Reapply recalcule le fond d'écran avec les photos actuelles pour la configuration d'écrans
// courante (branchement, résolution…). S'il faut plus de photos qu'avant, renvoie errNeedDraw :
// c'est à l'appelant de décider d'un nouveau tirage (pause, planification).
func (c *Core) Reapply() (res Result, err error) {
	c.run.Lock()
	defer c.run.Unlock()
	defer debug.FreeOSMemory()
	defer guard("le recalcul", &err)
	j, err := c.begin()
	if err != nil {
		return Result{}, err
	}
	mons, single, n := j.targets()
	c.mu.Lock()
	last := c.lastPhotos
	c.mu.Unlock()
	if len(last) < n {
		return Result{}, errNeedDraw
	}
	_, path, err := j.applyKept(last, mons, single)
	if err != nil {
		return Result{}, err
	}
	log.Printf("écrans modifiés : fond d'écran recalculé (%d photo(s))", n)
	j.lockScreen(path)
	return Result{Asset: last[0].Asset}, nil
}

// applyKept applique des photos déjà mémorisées (compressées), en les répétant s'il y a plus
// d'emplacements que de photos. Renvoie les photos dans l'ordre d'affichage.
func (j *job) applyKept(set []keptPhoto, mons []Monitor, single Screen) ([]keptPhoto, string, error) {
	var shown []keptPhoto
	next := func() (Photo, error) {
		k := set[len(shown)%len(set)]
		img, err := jpeg.Decode(bytes.NewReader(k.JPEG))
		if err != nil {
			return Photo{}, err
		}
		shown = append(shown, k)
		return Photo{Asset: k.Asset, Img: img, Info: k.Info, jpg: k.JPEG}, nil
	}
	path, err := j.applyTo(next, mons, single)
	return shown, path, err
}

// Previous réaffiche le fond d'écran précédent (jusqu'à historyDepth retours en arrière).
func (c *Core) Previous() (res Result, err error) {
	c.run.Lock()
	defer c.run.Unlock()
	defer debug.FreeOSMemory()
	defer guard("le retour à la photo précédente", &err)
	j, err := c.begin()
	if err != nil {
		return Result{}, err
	}
	c.mu.Lock()
	var set []keptPhoto
	if n := len(c.history); n > 0 {
		set = c.history[n-1]
	}
	c.mu.Unlock()
	if len(set) == 0 {
		return Result{}, errNoPrevious
	}
	mons, single, _ := j.targets()
	shown, path, err := j.applyKept(set, mons, single)
	if err != nil {
		return Result{}, err
	}
	first := shown[0].Asset
	c.mu.Lock()
	if n := len(c.history); n > 0 {
		c.history = c.history[:n-1]
	}
	c.lastPhotos = shown
	c.last, c.lastOK = first, true
	c.mu.Unlock()
	log.Printf("photo précédente : %s (%s)", first.OriginalFileName, first.ID)
	if c.afterApply != nil {
		c.afterApply(j.cfg, first, j.client.WebURL(first.ID))
	}
	j.lockScreen(path)
	return Result{Asset: first}, nil
}

// ExcludeCurrent ajoute la photo affichée (celle de l'écran principal) à la liste des photos
// à ne plus afficher. La liste est locale (excluded.json) : rien n'est modifié dans Immich.
func (c *Core) ExcludeCurrent() (Asset, error) {
	c.mu.Lock()
	if !c.lastOK {
		c.mu.Unlock()
		return Asset{}, errors.New("aucune photo affichée")
	}
	a := c.last
	c.excluded[a.ID] = true
	var history [][]keptPhoto
	for _, set := range c.history { // elle ne doit pas revenir par « Photo précédente »
		var kept []keptPhoto
		for _, k := range set {
			if k.Asset.ID != a.ID {
				kept = append(kept, k)
			}
		}
		if len(kept) > 0 {
			history = append(history, kept)
		}
	}
	c.history = history
	err := c.saveExcluded()
	c.mu.Unlock()
	if isUUID(a.ID) {
		os.Remove(filepath.Join(c.cacheDir(), a.ID+".jpg"))
		os.Remove(filepath.Join(c.cacheDir(), a.ID+".json"))
	}
	log.Printf("photo masquée : %s (%s)", a.OriginalFileName, a.ID)
	return a, err
}

// saveExcluded écrit la liste d'exclusion (appelé sous c.mu).
func (c *Core) saveExcluded() error {
	ids := make([]string, 0, len(c.excluded))
	for id := range c.excluded {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	b, _ := json.Marshal(ids)
	return os.WriteFile(c.excludedPath(), b, 0o644)
}

// ExcludedCount : nombre de photos masquées.
func (c *Core) ExcludedCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.excluded)
}

// ClearExcluded réautorise toutes les photos masquées.
func (c *Core) ClearExcluded() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.excluded = map[string]bool{}
	return c.saveExcluded()
}

// Favorite ajoute la photo affichée (écran principal) aux favoris Immich.
func (c *Core) Favorite() (Asset, error) {
	c.mu.Lock()
	a, ok, client := c.last, c.lastOK, c.client
	c.mu.Unlock()
	if !ok || client == nil {
		return Asset{}, errors.New("aucune photo affichée")
	}
	return a, client.SetFavorite(a.ID)
}

// Next choisit et applique de nouvelles images (une par écran si demandé).
func (c *Core) Next() (res Result, err error) {
	c.run.Lock()
	defer c.run.Unlock()
	defer debug.FreeOSMemory() // rend la RAM à Windows après le pic de décodage
	defer guard("le changement d'image", &err)
	j, err := c.begin()
	if err != nil {
		return Result{}, err
	}
	return j.next()
}

func (j *job) next() (Result, error) {
	c := j.c
	mons, single, n := j.targets()

	res := Result{}
	f := &feeder{j: j, cache: true}
	// Photos préchargées d'abord (aucun accès réseau), puis téléchargement à la demande.
	pre := j.takePreloaded()
	var online func() (Photo, error)
	f.src = func() (Photo, error) {
		for len(pre) > 0 {
			it := pre[0]
			pre = pre[1:]
			p, err := j.fromPreload(it)
			if err == nil {
				return p, nil
			}
			log.Printf("photo préchargée %s inutilisable : %v", it.asset.ID, err)
		}
		if online == nil {
			assets, err := j.refreshAlbums()
			if err != nil {
				return Photo{}, err
			}
			online = j.online(n, assets)
		}
		return online()
	}
	defer func() { // préchargées non utilisées (moins d'écrans que prévu, erreur)
		for _, it := range pre {
			os.Remove(it.path)
		}
	}()
	if p, err := f.fetch(); err == nil {
		f.head = &p
	} else {
		off := &feeder{j: j, src: j.cached()}
		cp, cerr := off.fetch()
		if cerr != nil {
			return Result{}, err
		}
		log.Printf("Immich injoignable (%v) : photo du cache hors ligne", err)
		off.head = &cp
		f, res.Offline, res.Cause = off, true, err
	}

	primaryPath, err := j.applyTo(f.next, mons, single)
	if err != nil {
		return Result{}, err
	}
	first := f.shown[0].Asset

	offlinePool := len(c.cachedIDs())
	c.mu.Lock()
	pool := len(c.assets)
	if res.Offline {
		pool = offlinePool
	}
	if len(c.lastPhotos) > 0 { // pour « Photo précédente »
		c.history = append(c.history, c.lastPhotos)
		if len(c.history) > historyDepth {
			c.history = c.history[len(c.history)-historyDepth:]
		}
	}
	c.lastPhotos = f.shown
	for _, k := range f.distinct {
		c.remember(k.Asset.ID, pool, n)
		log.Printf("fond d'écran : %s (%s)", k.Asset.OriginalFileName, k.Asset.ID)
	}
	c.last, c.lastOK = first, true
	recent, _ := json.Marshal(c.recent)
	c.mu.Unlock()
	if err := os.WriteFile(c.recentPath(), recent, 0o644); err != nil {
		log.Printf("historique des photos : %v", err)
	}

	if c.afterApply != nil {
		c.afterApply(j.cfg, first, j.client.WebURL(first.ID))
	}
	j.lockScreen(primaryPath)
	res.Asset = first
	return res, nil
}

// lockScreen applique la photo de l'écran principal à l'écran de verrouillage ;
// l'erreur est journalisée et mémorisée, mais non bloquante.
func (j *job) lockScreen(path string) {
	c := j.c
	if !j.cfg.LockScreen || c.setLockScreen == nil {
		return
	}
	err := c.setLockScreen(path)
	if err != nil {
		log.Printf("écran de verrouillage : %v", err)
	}
	c.mu.Lock()
	c.lockErr = err
	c.mu.Unlock()
}

func (c *Core) nextName(prefix string) string {
	s := "a"
	if c.flip {
		s = "b"
	}
	// On alterne entre deux fichiers : Windows ne rafraîchit pas toujours si le chemin ne change pas.
	return filepath.Join(c.dir, prefix+"_"+s+".jpg")
}

// Écran « très large » à partir de ce rapport largeur/hauteur (32:9 ≈ 3,56 ; 21:9 ≈ 2,39 n'est pas découpé).
const splitRatio = 2.8

// panels : nombre de photos à afficher sur cet écran (2 si très large et option active).
func (j *job) panels(sc Screen) int {
	if j.cfg.SplitWide && sc.H > 0 && float64(sc.W)/float64(sc.H) >= splitRatio {
		return 2
	}
	return 1
}

func (j *job) applySingle(next func() (Photo, error), sc Screen) (string, error) {
	c := j.c
	photos, err := take(next, j.panels(sc))
	if err != nil {
		return "", err
	}
	style := j.cfg.Style
	var jpg []byte
	if (j.wantInfo() || len(photos) > 1) && sc.W > 0 {
		jpg, err = j.render(photos, sc)
		// L'image fait déjà la taille de l'écran. En « Étendre », elle fait celle du bureau
		// virtuel : il faut garder ce style pour qu'elle s'étale sur tous les écrans.
		if style != "span" {
			style = "fill"
		}
	} else if p := photos[0]; p.jpg != nil && max(p.Img.Bounds().Dx(), p.Img.Bounds().Dy()) <= keptMaxSide {
		jpg = p.jpg // pas de réduction : la version gardée est déjà l'image complète
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

// applyMulti rend les photos de chaque écran à sa taille exacte puis les applique.
// Les photos d'un écran sont libérées avant de traiter l'écran suivant.
func (j *job) applyMulti(next func() (Photo, error), mons []Monitor) (string, error) {
	c := j.c
	paths := map[string]string{}
	var primary string
	for i, m := range mons {
		photos, err := take(next, j.panels(m.Screen))
		if err != nil {
			return "", err
		}
		jpg, err := j.render(photos, m.Screen)
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

// render produit l'image d'un écran : une photo, ou deux côte à côte (écran très large),
// séparées par une fine bande sombre, chacune avec sa légende vers le bord extérieur.
func (j *job) render(photos []Photo, sc Screen) ([]byte, error) {
	if sc.W < 320 || sc.H < 240 {
		return nil, fmt.Errorf("taille d'écran inattendue %dx%d", sc.W, sc.H)
	}
	if len(photos) == 1 {
		return encodeJPEG(j.renderPanel(photos[0], sc, j.cfg.CaptionPosition))
	}
	dst := image.NewRGBA(image.Rect(0, 0, sc.W, sc.H))
	gap := max(2, sc.H/270) // ≈ 4 px en 1080p, 5 px en 1440p
	pw := (sc.W - gap*(len(photos)-1)) / len(photos)
	x := 0
	for i, p := range photos {
		w := pw
		if i == len(photos)-1 {
			w = sc.W - x
		}
		pos := j.cfg.CaptionPosition
		if i == 0 && len(photos) == 2 { // panneau gauche : légende côté gauche (bord extérieur)
			pos = strings.Replace(pos, "right", "left", 1)
		} else if i == len(photos)-1 {
			pos = strings.Replace(pos, "left", "right", 1)
		}
		panel := j.renderPanel(p, Screen{W: w, H: sc.H, BottomInset: sc.BottomInset}, pos)
		draw.Draw(dst, image.Rect(x, 0, x+w, sc.H), panel, image.Point{}, draw.Src)
		x += w + gap
	}
	return encodeJPEG(dst)
}

func (j *job) renderPanel(p Photo, sc Screen, pos string) *image.RGBA {
	dst := compose(p.Img, sc, j.cfg.Style)
	if j.wantInfo() {
		place, date := captionParts(p.Info, j.cfg.ShowPlace, j.cfg.ShowDate)
		if j.c.fonts == nil {
			j.c.fonts = loadFonts()
		}
		drawCaption(dst, j.c.fonts, place, date, pos, sc.BottomInset)
	}
	return dst
}

func encodeJPEG(img image.Image) ([]byte, error) {
	var buf bytes.Buffer
	err := jpeg.Encode(&buf, img, &jpeg.Options{Quality: 95})
	return buf.Bytes(), err
}
