package main

import (
	"log"
	"os"
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

// cleanupAfterUpdate supprime l'ancien exe (.old) une fois l'ancienne instance terminée,
// et met à jour la version affichée dans « Applications installées ».
func cleanupAfterUpdate() {
	self, err := os.Executable()
	if err != nil {
		return
	}
	go func() {
		for i := 0; i < 30; i++ {
			if err := os.Remove(self + ".old"); err == nil || os.IsNotExist(err) {
				return
			}
			time.Sleep(time.Second)
		}
	}()
	if samePath(self, installedExe()) {
		if k, err := registry.OpenKey(registry.CURRENT_USER, uninstallKey, registry.SET_VALUE); err == nil {
			k.SetStringValue("DisplayVersion", appVersion)
			k.Close()
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
	releaseInstance()
	if err := hiddenCmd(self, "--after-update").Start(); err != nil {
		log.Printf("redémarrage impossible : %v", err)
		return
	}
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
