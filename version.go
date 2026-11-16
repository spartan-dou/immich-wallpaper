package main

import (
	"os"
	"path/filepath"
	"sync"
)

// appVersion est remplacée au build par la pipeline (-X main.appVersion=1.5.0).
var appVersion = "1.10.0"

// flavor distingue les exécutables publiés (-X main.flavor=…) :
//   - ""         : exe classique, celui que la mise à jour automatique des copies installées télécharge ;
//     lancé hors du dossier d'installation, il propose de s'installer (« Non » = lancer sans installer) ;
//   - "setup"    : installeur — installe (ou met à jour) puis lance la copie installée, rien d'autre ;
//   - "portable" : ne s'installe jamais, garde ses données dans un dossier à côté de l'exe.
var flavor = ""

// portableMarker : fichier qui, posé à côté de l'exe, le rend portable, quelle que soit sa
// variante (c'est ce que contient le zip portable : l'exe classique et ce fichier). Le
// supprimer rend l'appli « normale ».
const portableMarker = "portable.txt"

// hasPortableMarker indique si le dossier contient le fichier témoin de la version portable.
func hasPortableMarker(dir string) bool {
	st, err := os.Stat(filepath.Join(dir, portableMarker))
	return err == nil && st.Mode().IsRegular()
}

// portableByMarker : évalué une fois, l'exe ne change pas de dossier en cours de route.
var portableByMarker = sync.OnceValue(func() bool {
	exe, err := os.Executable()
	return err == nil && hasPortableMarker(filepath.Dir(exe))
})

func isPortable() bool { return flavor == "portable" || portableByMarker() }
func isSetup() bool    { return flavor == "setup" }
