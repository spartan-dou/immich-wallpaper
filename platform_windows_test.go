package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

func TestSecretRoundTrip(t *testing.T) {
	enc, ok := protectSecret("clé-secrète-é")
	if !ok || enc == "" || enc == "clé-secrète-é" {
		t.Fatal("chiffrement DPAPI indisponible", enc)
	}
	got, err := unprotectSecret(enc)
	if err != nil || got != "clé-secrète-é" {
		t.Fatal(got, err)
	}
	if _, err := unprotectSecret("AAAA"); err == nil {
		t.Fatal("valeur invalide déchiffrée")
	}
	if _, ok := protectSecret(""); ok {
		t.Fatal("une clé vide ne doit pas être chiffrée")
	}
}

func TestSystemExe(t *testing.T) {
	for _, p := range []string{powershellExe(), systemExe("", "cmd.exe"), systemExe("", "taskkill.exe"), systemExe("", "ping.exe")} {
		if !filepath.IsAbs(p) {
			t.Errorf("%s : chemin relatif", p)
		}
		if _, err := os.Stat(p); err != nil {
			t.Errorf("%s : %v", p, err)
		}
	}
}

// La ligne de commande de désinstallation supprime bien le dossier, y compris avec des espaces.
func TestUninstallCmdLine(t *testing.T) {
	target := filepath.Join(t.TempDir(), "dossier avec espaces", "ImmichWallpaper")
	if err := os.MkdirAll(filepath.Join(target, "sous"), 0o755); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(target, "sous", "f.txt"), []byte("x"), 0o644)
	cmd := exec.Command(systemExe("", "cmd.exe"))
	cmd.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000,
		CmdLine: uninstallCmdLine(systemExe("", "ping.exe"), target)}
	if err := cmd.Run(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(target); !os.IsNotExist(err) {
		t.Fatal("dossier non supprimé :", err)
	}
}

func TestRetryDelay(t *testing.T) {
	want := []time.Duration{2, 2, 4, 8, 16, 30, 30}
	for n, w := range want {
		if got := retryDelay(n); got != w*time.Minute {
			t.Errorf("après %d échec(s) : %v (attendu %v min)", n, got, w)
		}
	}
}

func TestReplaceFileSync(t *testing.T) {
	p := filepath.Join(t.TempDir(), "x.exe.new")
	if err := writeFileSync(p, []byte("MZ")); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(p); string(b) != "MZ" {
		t.Fatal(string(b))
	}
}

// Les fonctions de contexte doivent répondre sans erreur ni blocage sur un vrai Windows.
func TestSystemContext(t *testing.T) {
	cost, err := connectionCost()
	if err != nil {
		// Service absent sur certaines éditions (Windows Server des machines de CI) :
		// l'appli considère alors la connexion comme non limitée.
		t.Logf("coût de la connexion indisponible : %v", err)
		if meteredConnection() {
			t.Fatal("sans information, la connexion ne doit pas être considérée comme limitée")
		}
	}
	t.Logf("batterie=%v plein écran=%v coût connexion=%#x limitée=%v", onBattery(), fullscreenBusy(), cost, meteredConnection())
	cfg := defaultConfig()
	if blocked(&cfg, true) != "" {
		t.Fatal("rien ne doit bloquer avec la configuration par défaut")
	}
	cfg.ActiveFrom, cfg.ActiveTo = "00:00", "00:01"
	if now := time.Now(); now.Hour() != 0 || now.Minute() != 0 {
		if blocked(&cfg, true) != "hors plage horaire" || blocked(&cfg, false) != "" {
			t.Fatal("plage horaire non respectée")
		}
	}
}

// Version portable : données à côté de l'exe, entrée de démarrage distincte, jamais d'installation.
func TestPortableFlavor(t *testing.T) {
	defer func(f string) { flavor = f }(flavor)
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	flavor = ""
	std := dataDir()
	if filepath.Dir(std) == filepath.Dir(exe) || autostartValue() != runValue {
		t.Fatal("version classique : données attendues dans le profil :", std)
	}
	flavor = "portable"
	if got := dataDir(); got != filepath.Join(filepath.Dir(exe), "ImmichWallpaper-data") || got == std {
		t.Fatal("version portable : données attendues à côté de l'exe :", got)
	}
	if autostartValue() == runValue {
		t.Fatal("la version portable ne doit pas réutiliser l'entrée de démarrage de la version installée")
	}
	if maybeInstall() {
		t.Fatal("la version portable ne doit jamais proposer l'installation")
	}
}
