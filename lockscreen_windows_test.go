package main

import (
	"bytes"
	"errors"
	"image"
	"image/jpeg"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// Vérifie les appels WinRT de l'écran de verrouillage jusqu'à l'obtention du fichier et de la
// fabrique LockScreen, sans modifier l'écran de verrouillage d'un PC de développement.
func TestLockScreenWinRT(t *testing.T) {
	path := filepath.Join(t.TempDir(), "écran test.jpg")
	var jpg bytes.Buffer
	if err := jpeg.Encode(&jpg, image.NewRGBA(image.Rect(0, 0, 64, 36)), nil); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, jpg.Bytes(), 0o644); err != nil {
		t.Fatal(err)
	}
	err := withWinRT(func() error {
		file, err := storageFile(path, 30*time.Second)
		if err != nil {
			return err
		}
		file.release()
		statics, err := activationFactory("Windows.System.UserProfile.LockScreen", &iidLockScreenStatics)
		if err != nil {
			return err
		}
		statics.release()
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}

	// Fichier absent : erreur de Windows, pas « API indisponible » (pas de repli PowerShell).
	err = withWinRT(func() error {
		_, err := storageFile(filepath.Join(t.TempDir(), "absent.jpg"), 30*time.Second)
		return err
	})
	if err == nil || errors.Is(err, errWinRTUnavailable) {
		t.Fatalf("fichier absent : %v", err)
	}

	// Appel complet (SetImageFileAsync puis son résultat) : sur la machine jetable de la CI, où
	// Windows peut le refuser (compte de service) — l'important est un retour propre, sans
	// plantage —, ou à la demande avec IW_LOCKSCREEN_TEST=1, où il doit réussir.
	switch {
	case os.Getenv("IW_LOCKSCREEN_TEST") == "1":
		if err := setLockScreenNative(path); err != nil {
			t.Fatal(err)
		}
	case os.Getenv("GITHUB_ACTIONS") == "true":
		t.Logf("écran de verrouillage (CI) : %v", setLockScreenNative(path))
	}
}
