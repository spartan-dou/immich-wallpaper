package main

import (
	_ "embed"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
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
	busy     atomic.Bool
	updating atomic.Bool
	pending  *pendingUpdate
	mStatus  *systray.MenuItem
	mOpenPic *systray.MenuItem
}

func main() {
	for _, arg := range os.Args[1:] {
		if arg == "--uninstall" {
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
	a := &App{dir: dataDir()}
	os.MkdirAll(a.dir, 0o755)
	writePID()
	cleanupAfterUpdate()
	a.cfgPath = filepath.Join(a.dir, "config.json")
	a.logPath = filepath.Join(a.dir, "journal.txt")
	if st, err := os.Stat(a.logPath); err == nil && st.Size() > 1<<20 {
		os.Remove(a.logPath)
	}
	if f, err := os.OpenFile(a.logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644); err == nil {
		log.SetOutput(io.MultiWriter(f))
	}
	log.Printf("--- démarrage v%s, dossier %s", appVersion, a.dir)

	a.core = newCore(a.dir, setWallpaper)
	a.core.setLockScreen = setLockScreen
	a.core.screen = screenFor
	a.core.monitors = listMonitors
	a.core.setMonitorWallpapers = setMonitorWallpapers
	a.core.afterApply = updateDesktopLink
	var err error
	a.settings, err = startSettingsServer(a.cfgPath)
	if err != nil {
		msgBox("Immich Wallpaper", "Impossible de démarrer la page de réglages : "+err.Error(), mbIconError)
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

	systray.Run(a.onReady, func() { log.Printf("--- arrêt") })
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
	a.mu.Unlock()
	log.Printf("config chargée : %s, %d album(s), toutes les %d min", cfg.ServerURL, len(cfg.Albums), cfg.IntervalMinutes)
}

func (a *App) change() (Asset, error) {
	if !a.busy.CompareAndSwap(false, true) {
		return Asset{}, errors.New("un changement est déjà en cours")
	}
	defer a.busy.Store(false)
	a.setStatus("Téléchargement…")
	res, err := a.core.Next()
	as := res.Asset

	a.mu.Lock()
	interval := 60 * time.Minute
	if a.cfg != nil {
		interval = time.Duration(a.cfg.IntervalMinutes) * time.Minute
	}
	switch {
	case err == nil:
		a.next = time.Now().Add(interval)
	case errors.Is(err, errUnauthorized):
		a.next = time.Now().Add(30 * time.Minute)
	default: // réseau pas prêt (démarrage, VPN…) : on réessaie bientôt
		a.next = time.Now().Add(2 * time.Minute)
	}
	a.mu.Unlock()

	if err != nil {
		log.Printf("erreur : %v", err)
		a.setStatus("Erreur : " + err.Error())
		return as, err
	}
	st := as.OriginalFileName
	if res.Offline {
		st = "Hors ligne (photo du cache) : " + st
		if errors.Is(res.Cause, errUnauthorized) {
			st = "Token refusé, photo du cache : " + as.OriginalFileName
		}
	}
	if a.core.LockError() != nil {
		st += " (écran de verrouillage : échec, voir le journal)"
	}
	a.setStatus(st)
	a.mOpenPic.Enable()
	return as, nil
}

func (a *App) onReady() {
	systray.SetIcon(iconICO)
	systray.SetTitle("Immich Wallpaper")

	a.mStatus = systray.AddMenuItem("Démarrage…", "")
	a.mStatus.Disable()
	systray.AddSeparator()
	mNext := systray.AddMenuItem("Image suivante", "Changer le fond d'écran maintenant")
	a.mOpenPic = systray.AddMenuItem("Voir la photo dans Immich", "")
	a.mOpenPic.Disable()
	systray.AddSeparator()
	mSettings := systray.AddMenuItem("Paramètres…", "Ouvrir la page de réglages")
	mPause := systray.AddMenuItemCheckbox("Pause", "Ne plus changer automatiquement", false)
	mAuto := systray.AddMenuItemCheckbox("Démarrer avec Windows", "", autostartEnabled())
	mLog := systray.AddMenuItem("Ouvrir le journal", "")
	mUpdate := systray.AddMenuItem("Rechercher une mise à jour", "Version actuelle : "+appVersion)
	mUninstall := systray.AddMenuItem("Désinstaller…", "Tout supprimer (programme, réglages, démarrage auto)")
	systray.AddSeparator()
	mQuit := systray.AddMenuItem("Quitter", "")

	// Clic gauche sur l'icône : ouvrir la photo affichée dans Immich (clic droit : menu).
	systray.SetOnTapped(func() {
		if _, u, ok := a.core.Last(); ok {
			shellOpen(u)
		}
	})

	if _, err := os.Stat(a.cfgPath); errors.Is(err, os.ErrNotExist) {
		writeDefaultConfig(a.cfgPath)
	}
	if cfg, err := loadConfig(a.cfgPath); err == nil {
		a.applyConfig(cfg, true)
		a.setStatus("Prêt")
	} else {
		log.Printf("config incomplète : %v", err)
		a.setStatus("Configuration à compléter")
		shellOpen(a.settings.URL())
	}

	go a.updateLoop()
	if hasArg("--after-update") {
		go reportHealthy()
	}
	go a.handleSystemEvents()

	go func() {
		t := time.NewTicker(10 * time.Second)
		check := func() {
			a.mu.Lock()
			due := a.cfg != nil && !a.paused && !time.Now().Before(a.next)
			a.mu.Unlock()
			if due {
				go a.change()
			}
		}
		check()
		for {
			select {
			case <-t.C:
				check()
			case <-mNext.ClickedCh:
				go a.change()
			case <-a.mOpenPic.ClickedCh:
				if _, u, ok := a.core.Last(); ok {
					shellOpen(u)
				}
			case <-mSettings.ClickedCh:
				shellOpen(a.settings.URL())
			case <-mPause.ClickedCh:
				a.mu.Lock()
				a.paused = !a.paused
				p := a.paused
				a.mu.Unlock()
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
					hiddenCmd(self, "--uninstall").Start()
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
			if ev == evUnlock && cfg.ChangeOnUnlock && !paused {
				log.Printf("session déverrouillée : nouvelle image")
				go a.change()
			} else {
				go a.reapply()
			}
		}
	}
}

// reapply recalcule le fond d'écran (mêmes photos) pour la nouvelle configuration d'écrans.
func (a *App) reapply() {
	if !a.busy.CompareAndSwap(false, true) {
		return
	}
	defer a.busy.Store(false)
	if _, err := a.core.Reapply(); err != nil {
		log.Printf("recalcul après changement d'écran : %v", err)
	}
}
