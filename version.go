package main

// appVersion est remplacée au build par la pipeline (-X main.appVersion=1.5.0).
var appVersion = "1.8.0"

// flavor distingue les exécutables publiés (-X main.flavor=…) :
//   - ""         : exe classique, celui que la mise à jour automatique des copies installées télécharge ;
//     lancé hors du dossier d'installation, il propose de s'installer (« Non » = lancer sans installer) ;
//   - "setup"    : installeur — installe (ou met à jour) puis lance la copie installée, rien d'autre ;
//   - "portable" : ne s'installe jamais, garde ses données dans un dossier à côté de l'exe.
var flavor = ""

func isPortable() bool { return flavor == "portable" }
func isSetup() bool    { return flavor == "setup" }
