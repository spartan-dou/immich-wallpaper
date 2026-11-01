package main

import (
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"fyne.io/systray"
	"golang.org/x/sys/windows/registry"
)

// replaceSelf remplace l'exe en cours d'exécution. Windows interdit d'écraser un exe lancé,
// mais autorise à le renommer : on le renomme en .old, on écrit le nouveau à sa place.
func replaceSelf(bin []byte) (string, error) {
	self, err := os.Executable()
	if err != nil {
		return "", err
	}
	old := self + ".old"
	os.Remove(old)
	if err := os.Rename(self, old); err != nil {
		return "", err
	}
	if err := os.WriteFile(self, bin, 0o755); err != nil {
		os.Rename(old, self) // retour arrière
		return "", err
	}
	return self, nil
}

// cleanupAfterUpdate : ménage au démarrage et version affichée dans « Applications installées ».
// Après une mise à jour, l'ancien exe (.old) n'est supprimé qu'une fois la nouvelle version
// déclarée en bonne santé (voir reportHealthy), pour permettre un retour arrière.
func cleanupAfterUpdate() {
	self, err := os.Executable()
	if err != nil {
		return
	}
	os.Remove(self + ".bad")
	if !hasArg("--after-update") {
		os.Remove(self + ".old")
	}
	if samePath(self, installedExe()) {
		if k, err := registry.OpenKey(registry.CURRENT_USER, uninstallKey, registry.SET_VALUE); err == nil {
			k.SetStringValue("DisplayVersion", appVersion)
			k.Close()
		}
	}
}

func healthFile() string { return filepath.Join(dataDir(), "update_ok") }
func skipFile() string   { return filepath.Join(dataDir(), "skip_version") }

// reportHealthy : la nouvelle version tourne depuis 20 s → elle le signale à l'ancienne,
// qui peut alors se terminer ; on supprime ensuite l'ancien exe.
func reportHealthy() {
	time.Sleep(20 * time.Second)
	os.WriteFile(healthFile(), []byte(appVersion), 0o644)
	log.Printf("mise à jour vers %s validée", appVersion)
	self, err := os.Executable()
	if err != nil {
		return
	}
	for i := 0; i < 120; i++ {
		if err := os.Remove(self + ".old"); err == nil || os.IsNotExist(err) {
			return
		}
		time.Sleep(time.Second)
	}
}

// pendingUpdate : nouvelle version lancée, à surveiller avant de quitter.
type pendingUpdate struct {
	self, version string
	cmd           *exec.Cmd
}

// watch attend le signal de bonne santé de la nouvelle version. Si elle s'arrête avant
// (plantage au démarrage), on remet l'ancien exe en place, on le relance et on ignore
// cette version pour les mises à jour automatiques.
func (p *pendingUpdate) watch() {
	exited := make(chan struct{})
	go func() { p.cmd.Wait(); close(exited) }()
	deadline := time.After(90 * time.Second)
	for {
		if b, err := os.ReadFile(healthFile()); err == nil && string(b) == p.version {
			os.Remove(healthFile())
			return
		}
		select {
		case <-exited:
			if _, err := os.Stat(healthFile()); err == nil {
				return
			}
			log.Printf("la version %s s'est arrêtée au démarrage : retour à %s", p.version, appVersion)
			bad := p.self + ".bad"
			os.Remove(bad)
			if os.Rename(p.self, bad) != nil || os.Rename(p.self+".old", p.self) != nil {
				log.Printf("retour arrière impossible")
				return
			}
			os.WriteFile(skipFile(), []byte(p.version), 0o644)
			hiddenCmd(p.self, "--after-update").Start()
			return
		case <-deadline:
			return // toujours en vie sans signal : on la laisse tourner
		case <-time.After(time.Second):
		}
	}
}

// waitForInstance attend que l'instance précédente libère le verrou (après une mise à jour).
func waitForInstance() bool {
	for i := 0; i < 40; i++ {
		if singleInstance() {
			return true
		}
		time.Sleep(500 * time.Millisecond)
	}
	return false
}

// checkAndUpdate cherche une nouvelle version ; manual = déclenché depuis le menu.
func (a *App) checkAndUpdate(manual bool) {
	if !a.updating.CompareAndSwap(false, true) {
		return
	}
	defer a.updating.Store(false)

	a.mu.Lock()
	auto := a.cfg == nil || a.cfg.AutoUpdate
	a.mu.Unlock()
	if !manual && !auto {
		return
	}
	u, err := checkUpdate(appVersion)
	if err != nil {
		log.Printf("mise à jour : %v", err)
		if manual {
			msgBox("Immich Wallpaper", "Impossible de vérifier les mises à jour :\n"+err.Error(), mbIconError)
		}
		return
	}
	if u != nil && !manual {
		if b, err := os.ReadFile(skipFile()); err == nil && strings.TrimSpace(string(b)) == u.Version {
			log.Printf("version %s ignorée (échec au démarrage précédemment)", u.Version)
			return
		}
	}
	if u == nil {
		if manual {
			msgBox("Immich Wallpaper", "Immich Wallpaper est à jour (version "+appVersion+").", mbIconInfo)
		}
		return
	}
	if manual && !confirm("Version "+u.Version+" disponible (actuelle : "+appVersion+").\n\nInstaller maintenant ? L'appli redémarrera.") {
		return
	}
	log.Printf("mise à jour %s → %s : téléchargement", appVersion, u.Version)
	a.setStatus("Mise à jour vers " + u.Version + "…")
	bin, err := u.Download()
	self := ""
	if err == nil {
		// Chemin récupéré avant le renommage : après, le processus courant pointe vers le .old.
		self, err = replaceSelf(bin)
	}
	if err != nil {
		log.Printf("mise à jour : %v", err)
		a.setStatus("Échec de la mise à jour (voir le journal)")
		if manual {
			msgBox("Immich Wallpaper", "Échec de la mise à jour :\n"+err.Error(), mbIconError)
		}
		return
	}
	log.Printf("mise à jour installée, redémarrage")
	os.Remove(healthFile())
	releaseInstance()
	cmd := hiddenCmd(self, "--after-update")
	if err := cmd.Start(); err != nil {
		log.Printf("redémarrage impossible : %v", err)
		os.Rename(self, self+".bad")
		os.Rename(self+".old", self)
		singleInstance()
		return
	}
	a.pending = &pendingUpdate{self: self, version: u.Version, cmd: cmd}
	systray.Quit()
}

// updateLoop : première vérification 2 min après le démarrage, puis une fois par jour.
func (a *App) updateLoop() {
	time.Sleep(2 * time.Minute)
	last := time.Time{}
	for {
		if time.Since(last) >= 24*time.Hour {
			a.checkAndUpdate(false)
			last = time.Now()
		}
		time.Sleep(time.Hour)
	}
}
