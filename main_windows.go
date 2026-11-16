package main

import (
	_ "embed"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime/debug"
	"sync"
	"sync/atomic"
	"time"

	"fyne.io/systray"
)

//go:embed icon.ico
var iconICO []byte

const (
	mbOK        = 0x0
	mbIconInfo  = 0x40
	mbIconError = 0x10
)

type App struct {
	dir, cfgPath, logPath string
	core                  *Core
	settings              *SettingsServer

	mu       sync.Mutex
	cfg      *Config
	next     time.Time
	paused   bool
	status   string
	failures int           // échecs consécutifs (temporisation croissante)
	hold     string        // pourquoi le changement automatique attend (batterie, plein écran…)
	wake     chan struct{} // réveille la planification quand l'échéance change
	busy     atomic.Bool
	updating atomic.Bool
	pending  *pendingUpdate
	mStatus  *systray.MenuItem
	mOpenPic *systray.MenuItem
	mUpdate  *systray.MenuItem
}

func main() {
	debug.SetMemoryLimit(256 << 20) // GC plus agressif au-delà : limite le pic de RAM
	for _, arg := range os.Args[1:] {
		// La version portable n'est pas installée : rien à désinstaller (et surtout pas une
		// éventuelle copie installée sur le même PC).
		if arg == "--uninstall" && !isPortable() {
			uninstall(hasArg("--quiet"))
			return
		}
	}
	if maybeInstall() {
		return
	}
	if hasArg("--after-update") {
		if !waitForInstance() {
			return
		}
	} else if !singleInstance() {
		msgBox("Immich Wallpaper", "L'application est déjà lancée (icône près de l'horloge).", mbIconInfo)
		return
	}
	a := &App{dir: dataDir(), wake: make(chan struct{}, 1)}
	dirErr := os.MkdirAll(a.dir, 0o755)
	if dirErr != nil && isPortable() {
		msgBox("Immich Wallpaper", "Version portable : impossible de créer le dossier de données à côté de l'exe.\n"+
			"Place-la dans un dossier où tu peux écrire.\n\n"+dirErr.Error(), mbIconError)
		return
	}
	a.cfgPath = filepath.Join(a.dir, "config.json")
	a.logPath = filepath.Join(a.dir, "journal.txt")
	// Journal borné à 1 Mo, l'ancien est gardé dans journal.1.txt.
	if l, err := openRotatingLog(a.logPath, 1<<20); err == nil {
		log.SetOutput(l)
	}
	kind := ""
	if isPortable() {
		kind = " (portable)"
	}
	log.Printf("--- démarrage v%s%s, dossier %s", appVersion, kind, a.dir)
	if dirErr != nil {
		log.Printf("dossier de données : %v", dirErr)
	}
	writePID()
	cleanupAfterUpdate()

	a.core = newCore(a.dir, setWallpaper)
	a.core.setLockScreen = setLockScreen
	a.core.screen = screenFor
	a.core.monitors = listMonitors
	a.core.setMonitorWallpapers = setMonitorWallpapers
	a.core.afterApply = updateDesktopLink
	var err error
	// La page de réglages n'ouvre son port (127.0.0.1) qu'à la demande.
	a.settings, err = newSettingsServer(a.cfgPath)
	if err != nil {
		msgBox("Immich Wallpaper", "Impossible de préparer la page de réglages : "+err.Error(), mbIconError)
		return
	}
	a.settings.OnSaved = func(c *Config) { a.applyConfig(c, false) }
	a.settings.OnNext = func() (string, error) {
		as, err := a.change()
		if err != nil {
			return "", err
		}
		return "Nouvelle image : " + as.OriginalFileName, nil
	}
	a.settings.Status = a.getStatus
	a.settings.Excluded = a.core.ExcludedCount
	a.settings.OnClearExcluded = a.core.ClearExcluded

	systray.Run(a.onReady, func() {
		a.settings.Close()
		log.Printf("--- arrêt")
	})
	if a.pending != nil {
		a.pending.watch() // surveille le démarrage de la nouvelle version (retour arrière si plantage)
	}
}

func (a *App) setStatus(s string) {
	a.mu.Lock()
	a.status = s
	a.mu.Unlock()
	if a.mStatus != nil {
		a.mStatus.SetTitle(truncate(s, 60))
	}
	systray.SetTooltip(truncate("Immich Wallpaper — "+s, 120))
}

func (a *App) getStatus() string {
	a.mu.Lock()
	defer a.mu.Unlock()
	s := a.status
	if a.paused {
		s += " · en pause"
	} else if a.hold != "" {
		s += " · en attente (" + a.hold + ")"
	} else if !a.next.IsZero() && a.cfg != nil {
		s += " · prochaine à " + a.next.Format("15:04")
	}
	return s
}

func truncate(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n-1]) + "…"
}

// applyConfig est appelé au démarrage et à chaque enregistrement depuis la page.
func (a *App) applyConfig(cfg *Config, startup bool) {
	a.core.SetConfig(cfg)
	a.mu.Lock()
	a.cfg = cfg
	if startup && !cfg.ChangeOnStart {
		a.next = time.Now().Add(time.Duration(cfg.IntervalMinutes) * time.Minute)
	} else {
		a.next = time.Now() // nouvelle config → on applique tout de suite
	}
	a.failures = 0
	a.mu.Unlock()
	a.poke()
	log.Printf("config chargée : %s, %d album(s), toutes les %d min", cfg.ServerURL, len(cfg.Albums), cfg.IntervalMinutes)
}

// poke demande à la planification de recalculer sa prochaine échéance.
func (a *App) poke() {
	select {
	case a.wake <- struct{}{}:
	default:
	}
}

// retryDelay : délai avant un nouvel essai après n échecs consécutifs (2, 4, 8… 30 min).
func retryDelay(n int) time.Duration {
	d := 2 * time.Minute
	for i := 1; i < n && d < 30*time.Minute; i++ {
		d *= 2
	}
	return min(d, 30*time.Minute)
}

// recovered journalise un panic au lieu de laisser l'appli (sans console) disparaître.
func recovered(what string) {
	if r := recover(); r != nil {
		log.Printf("panic (%s) : %v\n%s", what, r, debug.Stack())
	}
}

// blocked indique pourquoi un changement automatique doit attendre ("" = rien ne s'y oppose).
// hours=false ignore la plage horaire (préchargement).
func blocked(cfg *Config, hours bool) string {
	switch {
	case hours && !cfg.activeAt(time.Now()):
		return "hors plage horaire"
	case cfg.PauseOnBattery && onBattery():
		return "sur batterie"
	case cfg.PauseOnFullscreen && fullscreenBusy():
		return "application en plein écran"
	case cfg.PauseOnMetered && meteredConnection():
		return "connexion limitée"
	}
	return ""
}

// preload prépare la photo suivante en arrière-plan, sauf si le contexte s'y oppose.
func (a *App) preload() {
	defer recovered("préchargement")
	a.mu.Lock()
	cfg := a.cfg
	a.mu.Unlock()
	if cfg == nil || !cfg.Preload || blocked(cfg, false) != "" {
		return
	}
	if err := a.core.Preload(); err != nil {
		log.Printf("préchargement : %v", err)
	}
}

// previous réaffiche le fond d'écran précédent et repousse le prochain changement.
func (a *App) previous() {
	defer recovered("photo précédente")
	if !a.busy.CompareAndSwap(false, true) {
		return
	}
	defer a.busy.Store(false)
	res, err := a.core.Previous()
	if err != nil {
		if !errors.Is(err, errNoPrevious) {
			log.Printf("photo précédente : %v", err)
		}
		a.setStatus("Photo précédente : " + err.Error())
		return
	}
	a.mu.Lock()
	if a.cfg != nil {
		a.next = time.Now().Add(time.Duration(a.cfg.IntervalMinutes) * time.Minute)
	}
	a.mu.Unlock()
	a.poke()
	a.setStatus("Photo précédente : " + res.Asset.OriginalFileName)
}

// favorite ajoute la photo affichée aux favoris Immich.
func (a *App) favorite() {
	defer recovered("favori")
	as, err := a.core.Favorite()
	if err != nil {
		log.Printf("ajout aux favoris : %v", err)
		a.setStatus("Favori impossible : " + err.Error())
		return
	}
	log.Printf("ajoutée aux favoris Immich : %s (%s)", as.OriginalFileName, as.ID)
	a.setStatus("Ajoutée aux favoris Immich : " + as.OriginalFileName)
}

// hide masque définitivement la photo affichée (liste locale) et passe à la suivante.
func (a *App) hide() {
	defer recovered("photo masquée")
	if _, err := a.core.ExcludeCurrent(); err != nil {
		log.Printf("photo masquée : %v", err)
		a.setStatus("Impossible de masquer la photo : " + err.Error())
		return
	}
	a.change()
}

func (a *App) change() (as Asset, err error) {
	if !a.busy.CompareAndSwap(false, true) {
		return Asset{}, errors.New("un changement est déjà en cours")
	}
	defer a.busy.Store(false)
	defer func() {
		if r := recover(); r != nil {
			log.Printf("panic (changement d'image) : %v\n%s", r, debug.Stack())
			err = fmt.Errorf("erreur interne : %v", r)
		}
	}()
	a.setStatus("Téléchargement…")
	res, err := a.core.Next()
	as = res.Asset

	a.mu.Lock()
	interval := 60 * time.Minute
	if a.cfg != nil {
		interval = time.Duration(a.cfg.IntervalMinutes) * time.Minute
	}
	switch {
	case err == nil:
		a.failures = 0
		a.hold = ""
		a.next = time.Now().Add(interval)
	case isAuthErr(err):
		a.next = time.Now().Add(30 * time.Minute)
	default: // réseau pas prêt (démarrage, VPN…) : nouvel essai bientôt, puis de plus en plus espacé
		a.failures++
		a.next = time.Now().Add(retryDelay(a.failures))
	}
	a.mu.Unlock()
	a.poke()

	if err != nil {
		log.Printf("erreur : %v", err)
		a.setStatus("Erreur : " + err.Error())
		return as, err
	}
	st := as.OriginalFileName
	if res.Offline {
		st = "Hors ligne (photo du cache) : " + st
		if isAuthErr(res.Cause) {
			st = "Token refusé, photo du cache : " + as.OriginalFileName
		}
	}
	if a.core.LockError() != nil {
		st += " (écran de verrouillage : échec, voir le journal)"
	}
	a.setStatus(st)
	a.mOpenPic.Enable()
	if !res.Offline {
		go a.preload() // la photo suivante est prête avant le prochain changement
	}
	return as, nil
}

func (a *App) onReady() {
	systray.SetIcon(iconICO)
	systray.SetTitle("Immich Wallpaper")

	a.mStatus = systray.AddMenuItem("Démarrage…", "")
	a.mStatus.Disable()
	systray.AddSeparator()
	mNext := systray.AddMenuItem("Image suivante", "Changer le fond d'écran maintenant")
	mPrev := systray.AddMenuItem("Photo précédente", "Revenir au fond d'écran précédent")
	a.mOpenPic = systray.AddMenuItem("Voir la photo dans Immich", "")
	a.mOpenPic.Disable()
	mFav := systray.AddMenuItem("Ajouter aux favoris Immich", "Marquer la photo affichée comme favorite")
	mHide := systray.AddMenuItem("Ne plus afficher cette photo", "La masquer (réversible dans les paramètres) et passer à la suivante")
	systray.AddSeparator()
	mSettings := systray.AddMenuItem("Paramètres…", "Ouvrir la page de réglages")
	mPause := systray.AddMenuItemCheckbox("Pause", "Ne plus changer automatiquement", false)
	mAuto := systray.AddMenuItemCheckbox("Démarrer avec Windows", "", autostartEnabled())
	mLog := systray.AddMenuItem("Ouvrir le journal", "")
	mUpdate := systray.AddMenuItem("Rechercher une mise à jour", "Version actuelle : "+appVersion)
	a.mUpdate = mUpdate
	mUninstall := systray.AddMenuItem("Désinstaller…", "Tout supprimer (programme, réglages, démarrage auto)")
	if isPortable() {
		mUninstall.Hide() // rien d'installé : il suffit de supprimer le dossier
	}
	systray.AddSeparator()
	mQuit := systray.AddMenuItem("Quitter", "")

	// Clic gauche sur l'icône : ouvrir la photo affichée dans Immich (clic droit : menu).
	systray.SetOnTapped(func() {
		if _, u, ok := a.core.Last(); ok {
			shellOpen(u)
		}
	})

	if _, err := os.Stat(a.cfgPath); errors.Is(err, os.ErrNotExist) {
		if err := writeDefaultConfig(a.cfgPath); err != nil {
			log.Printf("création de la configuration par défaut : %v", err)
		}
	}
	if keyStoredPlain(a.cfgPath) {
		// Clé API en clair (ancienne version, édition à la main) : on la réécrit chiffrée.
		if cfg, err := readConfig(a.cfgPath); err == nil {
			if err := saveConfig(a.cfgPath, cfg); err != nil {
				log.Printf("chiffrement de la clé API : %v", err)
			} else if !keyStoredPlain(a.cfgPath) {
				log.Printf("clé API chiffrée dans config.json (DPAPI)")
			}
		}
	}
	if cfg, err := loadConfig(a.cfgPath); err == nil {
		a.applyConfig(cfg, true)
		a.setStatus("Prêt")
	} else {
		log.Printf("config incomplète : %v", err)
		a.setStatus("Configuration à compléter")
		a.openSettings()
	}

	go a.updateLoop()
	if hasArg("--after-update") {
		go reportHealthy()
	}
	go a.handleSystemEvents()

	go func() {
		// Un minuteur réarmé sur la prochaine échéance, au lieu d'un réveil toutes les 10 s.
		// Plafonné à 1 min : l'horloge des minuteurs peut ne pas avancer pendant la veille du PC,
		// on recompare donc régulièrement à l'heure réelle.
		t := time.NewTimer(time.Hour)
		check := func() {
			a.mu.Lock()
			cfg := a.cfg
			active := cfg != nil && !a.paused
			wait := time.Until(a.next)
			a.mu.Unlock()
			hold := ""
			switch {
			case !active:
				wait = time.Minute
			case wait <= 0:
				// Échéance atteinte : on attend si le contexte s'y oppose (réévalué chaque minute).
				if hold = blocked(cfg, true); hold != "" {
					wait = time.Minute
				} else {
					go a.change()
					wait = 10 * time.Second // change() réveille la boucle dès qu'il a fixé la suite
				}
			}
			a.mu.Lock()
			a.hold = hold
			a.mu.Unlock()
			t.Reset(min(wait, time.Minute))
		}
		check()
		for {
			select {
			case <-t.C:
				check()
			case <-a.wake:
				check()
			case <-mNext.ClickedCh:
				go a.change()
			case <-mPrev.ClickedCh:
				go a.previous()
			case <-mFav.ClickedCh:
				go a.favorite()
			case <-mHide.ClickedCh:
				go a.hide()
			case <-a.mOpenPic.ClickedCh:
				if _, u, ok := a.core.Last(); ok {
					shellOpen(u)
				}
			case <-mSettings.ClickedCh:
				a.openSettings()
			case <-mPause.ClickedCh:
				a.mu.Lock()
				a.paused = !a.paused
				p := a.paused
				a.mu.Unlock()
				a.poke()
				if p {
					mPause.Check()
				} else {
					mPause.Uncheck()
				}
			case <-mAuto.ClickedCh:
				on := !mAuto.Checked()
				if err := setAutostart(on); err != nil {
					msgBox("Immich Wallpaper", fmt.Sprintf("Impossible de modifier le démarrage auto : %v", err), mbIconError)
				} else if on {
					mAuto.Check()
				} else {
					mAuto.Uncheck()
				}
			case <-mUpdate.ClickedCh:
				go a.checkAndUpdate(true)
			case <-mUninstall.ClickedCh:
				// Lancé dans un processus séparé : il nous arrête puis supprime nos fichiers.
				if self, err := os.Executable(); err == nil {
					if err := hiddenCmd(self, "--uninstall").Start(); err != nil {
						log.Printf("lancement de la désinstallation : %v", err)
					}
				}
			case <-mLog.ClickedCh:
				shellOpen(a.logPath)
			case <-mQuit.ClickedCh:
				systray.Quit()
				return
			}
		}
	}()
}

// openSettings ouvre la page de réglages dans le navigateur (et son port s'il était fermé).
func (a *App) openSettings() {
	u, err := a.settings.Open()
	if err != nil {
		msgBox("Immich Wallpaper", "Impossible d'ouvrir la page de réglages : "+err.Error(), mbIconError)
		return
	}
	shellOpen(u)
}

func hasArg(name string) bool {
	for _, a := range os.Args[1:] {
		if a == name {
			return true
		}
	}
	return false
}

// handleSystemEvents réagit au déverrouillage et aux changements d'écran. Ces événements
// arrivent souvent en rafale (plusieurs WM_DISPLAYCHANGE, déverrouillage + écran rallumé) :
// on attend 3 s de calme, et un déverrouillage l'emporte sur un simple changement d'écran.
func (a *App) handleSystemEvents() {
	const (
		evDisplay = 1
		evUnlock  = 2
	)
	ch := make(chan int, 16)
	send := func(e int) func() {
		return func() {
			select {
			case ch <- e:
			default:
			}
		}
	}
	go watchSystemEvents(&sysEvents{onDisplay: send(evDisplay), onUnlock: send(evUnlock)})

	pending := 0
	var timer <-chan time.Time
	for {
		select {
		case e := <-ch:
			pending = max(pending, e)
			timer = time.After(3 * time.Second)
		case <-timer:
			a.mu.Lock()
			cfg, paused := a.cfg, a.paused
			a.mu.Unlock()
			ev := pending
			pending, timer = 0, nil
			if cfg == nil {
				continue
			}
			if ev == evUnlock && cfg.ChangeOnUnlock && !paused && blocked(cfg, true) == "" {
				log.Printf("session déverrouillée : nouvelle image")
				go a.change()
			} else {
				go a.reapply()
			}
			a.poke() // sortie de veille possible : l'échéance est peut-être dépassée
		}
	}
}

// reapply recalcule le fond d'écran (mêmes photos) pour la nouvelle configuration d'écrans.
// S'il faut plus de photos que celles mémorisées (écran ajouté, aucune photo encore affichée),
// un nouveau tirage passe par change() — statut et échéance à jour — sauf si l'appli est en pause.
func (a *App) reapply() {
	defer recovered("recalcul après changement d'écran")
	if !a.busy.CompareAndSwap(false, true) {
		return
	}
	_, err := a.core.Reapply()
	a.busy.Store(false)
	switch {
	case err == nil:
	case errors.Is(err, errNeedDraw):
		a.mu.Lock()
		paused := a.paused
		a.mu.Unlock()
		if paused {
			log.Printf("écrans modifiés : nouveau tirage nécessaire, ignoré (en pause)")
			return
		}
		log.Printf("écrans modifiés : nouveau tirage nécessaire")
		a.change()
	default:
		log.Printf("recalcul après changement d'écran : %v", err)
	}
}
