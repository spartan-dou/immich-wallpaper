//go:build !windows

package main

import (
	"errors"
	"fmt"
)

func main() { fmt.Println("Immich Wallpaper ne fonctionne que sous Windows.") }

// Chiffrement de la clé API : disponible uniquement sous Windows (DPAPI).
func protectSecret(string) (string, bool) { return "", false }

func unprotectSecret(string) (string, error) {
	return "", errors.New("clé chiffrée par Windows, illisible sur ce système")
}
