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
	lastPhotos    []keptPhoto // photos affichées (compressées), réutilisées si les écrans changent

	dir string

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
		rnd:          rand.New(rand.NewSource(time.Now().UnixNano())),
	}
	// Historique des photos récentes : conservé d'un lancement à l'autre.
	if b, err := os.ReadFile(c.recentPath()); err == nil {
		var ids []string
		if json.Unmarshal(b, &ids) == nil && len(ids) <= 500 {
			c.recent = ids
		}
	}
	return c
}

func (c *Core) recentPath() string { return filepath.Join(c.dir, "recent.json") }

func (c *Core) SetConfig(cfg *Config) {
	c.mu.Lock()
	old := c.client
	if c.cfg == nil || c.cfg.ServerURL != cfg.ServerURL || c.cfg.APIKey != cfg.APIKey {
		c.albums = map[string]*albumState{} // autre serveur ou autre compte : on repart de zéro
	}
	c.cfg = cfg
	c.client = newClient(cfg)
	c.gen++
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
	assets, checked := c.assets, c.albumsChecked
	albums := make(map[string]*albumState, len(c.albums))
	for id, st := range c.albums {
		albums[id] = st
	}
	c.mu.Unlock()

	if assets != nil && time.Since(checked) < albumCheckEvery {
		return assets, nil
	}
	sigs, err := j.client.AlbumSignatures()
	if err != nil {
		if isAuthErr(err) || assets == nil {
			return nil, err
		}
		log.Printf("vérification des albums impossible, liste précédente conservée : %v", err)
		return assets, nil
	}
	changed := assets == nil
	for _, id := range j.cfg.Albums {
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
				changed = true
			} else {
				log.Printf("album %s : %v", id, err)
			}
			continue
		}
		log.Printf("album %q : %d images (rechargé)", name, len(imgs))
		albums[id] = &albumState{sig: sig, name: name, assets: imgs}
		changed = true
	}
	all := assets
	if changed {
		all = nil
		seen := map[string]bool{}
		for _, id := range j.cfg.Albums {
			if st := albums[id]; st != nil {
				for _, a := range st.assets {
					if !seen[a.ID] {
						seen[a.ID] = true
						all = append(all, a)
					}
				}
			}
		}
	}

	c.mu.Lock()
	if c.gen == j.gen { // sinon la configuration a changé entre-temps : résultat non mémorisé
		c.albums, c.albumsChecked, c.assets = albums, time.Now(), all
	}
	c.mu.Unlock()
	if len(all) == 0 {
		return nil, errors.New("aucune image trouvée dans les albums configurés")
	}
	return all, nil
}

// ---------- choix ----------

// order mélange les assets en repoussant à la fin ceux affichés récemment.
func (c *Core) order(list []Asset) []Asset {
	c.mu.Lock()
	defer c.mu.Unlock()
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

// online renvoie une source de photos téléchargées depuis Immich, une par appel.
// Seule la photo demandée est décodée : les portraits écartés (landscape_only) ne sont ni
// décodés ni mis en cache ; au plus n sont gardés compressés, en secours s'il manque des paysages.
func (j *job) online(n int, assets []Asset) func() (Photo, error) {
	type raw struct {
		asset Asset
		data  []byte
		meta  imageMeta
	}
	order := j.c.order(assets)
	var portraits []raw
	var lastErr error
	i, tries := 0, 0

	build := func(r raw) (Photo, error) {
		img, err := safeDecode(func() (image.Image, error) { return r.meta.decode(r.data) })
		if err != nil {
			return Photo{}, err
		}
		p := Photo{Asset: r.asset, Img: img}
		if j.wantInfo() {
			if info, err := j.client.AssetInfo(r.asset.ID); err == nil {
				p.Info = info
			} else {
				log.Printf("infos %s : %v", r.asset.ID, err)
			}
		}
		return p, nil
	}

	return func() (Photo, error) {
		for i < len(order) && tries < 15*n {
			a := order[i]
			i++
			tries++
			data, err := j.client.Download(a.ID, j.cfg.ImageSize)
			if err != nil {
				if errors.Is(err, errUnauthorized) {
					return Photo{}, err
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
			r := raw{a, data, meta}
			if j.cfg.LandscapeOnly && meta.W <= meta.H {
				if len(portraits) < n {
					portraits = append(portraits, r)
				}
				continue
			}
			p, err := build(r)
			if err != nil {
				log.Printf("%s : %v", a.ID, err)
				continue
			}
			return p, nil
		}
		for len(portraits) > 0 { // pas assez de paysages : on complète
			r := portraits[0]
			portraits = portraits[1:]
			p, err := build(r)
			if err != nil {
				log.Printf("%s : %v", r.asset.ID, err)
				continue
			}
			return p, nil
		}
		if lastErr != nil {
			return Photo{}, lastErr
		}
		return Photo{}, fmt.Errorf("aucune image utilisable après %d essais (voir le journal)", tries)
	}
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
	i := 0
	next := func() (Photo, error) {
		k := last[i]
		i++
		img, err := jpeg.Decode(bytes.NewReader(k.JPEG))
		if err != nil {
			return Photo{}, err
		}
		return Photo{Asset: k.Asset, Img: img, Info: k.Info, jpg: k.JPEG}, nil
	}
	path, err := j.applyTo(next, mons, single)
	if err != nil {
		return Result{}, err
	}
	log.Printf("écrans modifiés : fond d'écran recalculé (%d photo(s))", n)
	j.lockScreen(path)
	return Result{Asset: last[0].Asset}, nil
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
	assets, err := j.refreshAlbums()
	if err == nil {
		f.src = j.online(n, assets)
		var p Photo
		if p, err = f.fetch(); err == nil {
			f.head = &p
		}
	}
	if err != nil {
		off := &feeder{j: j, src: j.cached()}
		p, cerr := off.fetch()
		if cerr != nil {
			return Result{}, err
		}
		log.Printf("Immich injoignable (%v) : photo du cache hors ligne", err)
		off.head = &p
		f, res.Offline, res.Cause = off, true, err
	}

	primaryPath, err := j.applyTo(f.next, mons, single)
	if err != nil {
		return Result{}, err
	}
	first := f.shown[0].Asset

	pool := len(assets)
	if res.Offline {
		pool = len(c.cachedIDs())
	}
	c.mu.Lock()
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
