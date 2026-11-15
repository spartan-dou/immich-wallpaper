# Immich Wallpaper

Petite appli Windows (barre système) qui met en fond d'écran — et, en option, d'écran de verrouillage — des photos tirées d'un ou plusieurs albums [Immich](https://immich.app), avec le lieu et la date incrustés dans une carte façon verre dépoli. Un peu comme « Windows à la une », mais avec tes photos.

**Téléchargement** : onglet [Releases](../../releases) → `ImmichWallpaper-x64.exe` (PC Intel/AMD) ou `ImmichWallpaper-arm64.exe` (Windows sur ARM).

## Installation

Deux versions natives : `ImmichWallpaper-x64.exe` (PC Intel/AMD) et `ImmichWallpaper-arm64.exe` (PC Snapdragon / Windows sur ARM). Dans le doute : *Paramètres → Système → Informations système → Type du système*.

1. Lance `ImmichWallpaper.exe` (SmartScreen : *Informations complémentaires → Exécuter quand même*, exe non signé).
2. Il propose de s'installer : copie dans `%LOCALAPPDATA%\Programs\ImmichWallpaper\`, raccourci dans le menu Démarrer, lancement avec Windows, entrée dans *Paramètres → Applications installées*. « Non » = mode portable.
3. La page de réglages s'ouvre : URL + token → **Tester et charger les albums** → coche les albums → **Enregistrer**.

**Mise à jour** : lance simplement le nouvel exe téléchargé, il propose de remplacer la version installée (il arrête l'ancienne lui-même, la config est conservée).

## Plusieurs écrans et mode hors ligne

- **Une image différente par écran** (réglages → Affichage → Plusieurs écrans) : chaque photo est rendue à la résolution exacte de son écran, avec sa propre légende (interface Windows `IDesktopWallpaper`). L'écran de verrouillage reprend la photo de l'écran principal.
- **Écran très large** (32:9, « deux écrans en un ») : deux photos côte à côte, chacune dans un panneau 16:9 avec sa légende vers le bord extérieur, au lieu d'une seule photo très rognée. Les 21:9 ne sont pas découpés. Désactivable dans les réglages.
- **Déverrouillage et changements d'écran** : nouvelle photo à chaque déverrouillage du PC (désactivable) ; quand un écran est branché/débranché, change de résolution ou se rallume, le fond d'écran est recalculé avec les photos actuelles (nouveau tirage seulement s'il en faut davantage, ex. écran 32:9 ajouté — et jamais pendant une pause). Événements reçus par une fenêtre cachée (`WM_WTSSESSION_CHANGE`, `WM_DISPLAYCHANGE`, état d'affichage de la console), regroupés sur 3 s.
- **Hors ligne** : les dernières photos téléchargées (40 par défaut) sont gardées dans `%APPDATA%\ImmichWallpaper\cache`. Si Immich est injoignable, la rotation continue avec elles ; le menu de l'icône l'indique.
- **Albums** : l'appli vérifie toutes les 30 min la liste des albums (une seule requête) et ne recharge le contenu que des albums modifiés (nombre de photos ou date de modification changés). Un album supprimé ou dont le partage a été retiré sort du tirage.

## Mises à jour automatiques

L'appli vérifie les [releases](../../releases) de ce dépôt 2 min après le démarrage puis une fois par jour. Si une version plus récente existe, elle télécharge l'exe de son architecture, vérifie son empreinte avec `SHA256SUMS.txt`, se remplace (le nouvel exe est écrit à côté puis mis en place par renommage ; l'exe en cours devient `.old`, Windows interdisant de l'écraser) et redémarre. Seuls les fichiers des releases de ce dépôt sont acceptés (adresse et redirections contrôlées). L'empreinte protège contre un téléchargement corrompu, pas contre une compromission du dépôt : les releases ne sont pas signées. L'ancienne version surveille le démarrage de la nouvelle : si celle-ci s'arrête dans les premières secondes, elle se remet en place et cette version est ignorée par les mises à jour automatiques. Désactivable dans les réglages ; menu de l'icône → **Rechercher une mise à jour** pour forcer la vérification.

## Désinstallation

*Paramètres → Applications → Applications installées → Immich Wallpaper → Désinstaller*, ou menu de l'icône → **Désinstaller…**.
Supprime : le programme, le démarrage auto, le raccourci, la config (dont le token) et le journal. Le fond d'écran actuel reste en place.

## Clé API Immich

Immich → avatar → Paramètres du compte → Clés API → Nouvelle clé.
Permissions : `album.read`, `asset.read`, `asset.view`, `asset.download`.
Sans `asset.download`, l'appli fonctionne mais se rabat sur l'aperçu (1440 px) quand Immich redirige la pleine résolution vers l'original.

La clé est chiffrée dans `config.json` avec DPAPI (`api_key_protected`, lisible uniquement par ton compte Windows sur ce PC). Une clé écrite à la main dans `api_key` est acceptée et chiffrée au démarrage suivant. Elle n'est envoyée qu'au serveur pour lequel elle a été saisie : si tu changes l'URL du serveur, il faut la ressaisir.

**Certificat auto-signé** : l'option n'accepte plus n'importe quel certificat. Celui présenté lors de « Tester et charger les albums » est épinglé (`cert_fingerprint`, empreinte SHA-256) ; s'il change, la connexion est refusée jusqu'à un nouveau test suivi d'un enregistrement.

## Menu de l'icône

- **Image suivante** : change tout de suite.
- **Voir la photo dans Immich** : ouvre la photo affichée.
- **Paramètres…** : rouvre la page de réglages (servie localement sur 127.0.0.1, protégée par un jeton aléatoire, le token Immich n'y est jamais réaffiché).
- **Pause**, **Démarrer avec Windows**, **Ouvrir le journal**.
- **Rechercher une mise à jour** : vérifie tout de suite et propose l'installation.
- **Désinstaller…** : supprime le programme, ses réglages et le démarrage automatique.
- **Quitter**.

## Légende et écran de verrouillage

- **Lieu / date** : carte façon verre dépoli (fond flouté, coins arrondis) incrustée sur l'image (ville, région, pays déduits du GPS par Immich ; rien si la photo n'est pas géolocalisée). L'image est alors recadrée par l'appli à la résolution de l'écran principal pour que le texte ne soit jamais rogné.
- **Écran de verrouillage** : option à cocher. Dans Windows, *Personnalisation → Écran de verrouillage → Personnaliser* doit être sur « Image ». En cas d'échec, le motif est dans le journal.
- L'appli repasse automatiquement l'arrière-plan du bureau en mode « Image » si « Windows à la une » ou un diaporama était actif.

## Ouvrir la photo dans Immich

Le fond d'écran n'est pas cliquable sous Windows. À la place : **clic gauche sur l'icône** près de l'horloge, ou l'icône **« Voir cette photo dans Immich »** déposée sur le bureau (désactivable dans les réglages).

## Couleurs

Immich génère par défaut ses aperçus en Display P3 (profil ICC intégré). Windows ne gère pas les profils pour le fond d'écran : l'appli convertit donc l'image en sRGB (et applique l'orientation EXIF des originaux) avant de l'appliquer.

## Fichiers

Tout est dans `%APPDATA%\ImmichWallpaper\` : `config.json` (éditable à la main aussi), `journal.txt` (1 Mo max, le précédent est gardé dans `journal.1.txt`), `recent.json` (photos affichées récemment, pour ne pas les revoir après un redémarrage), le dossier `cache` et les deux images `wallpaper_a/b.jpg`.

## Comportement

- Tirage aléatoire, sans repasser par les photos affichées récemment ; albums vérifiés toutes les 30 min et à chaque enregistrement.
- Vidéos et éléments en corbeille ignorés. Option « paysage uniquement » (si aucune paysage n'est trouvée, une portrait est prise ; les portraits écartés ne sont ni décodés ni gardés dans le cache hors ligne).
- Qualité « Pleine résolution » = JPEG généré par Immich (gère HEIC/RAW) ; repli automatique sur l'aperçu si ton Immich ne génère pas de fullsize.
- Serveur injoignable (démarrage, hors VPN…) : l'image actuelle reste (ou une photo du cache hors ligne), nouvel essai après 2 min, puis de plus en plus espacé (4, 8, 16, 30 min).
- Plusieurs écrans : la même image partout par défaut, « Étendre » pour une image à cheval sur tous les écrans, ou une image différente par écran (voir plus haut).
- Images de plus de 120 Mpx refusées (protection mémoire).

## Développement

Go pur (pas de cgo), cross-compilable depuis n'importe quel OS :

```sh
GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go build -trimpath -ldflags "-H windowsgui -s -w" -o ImmichWallpaper-x64.exe .
GOOS=windows GOARCH=arm64 CGO_ENABLED=0 go build -trimpath -ldflags "-H windowsgui -s -w" -o ImmichWallpaper-arm64.exe .
go test ./...
```

La pipeline exécute en plus `go test -race`, les tests sous Windows, `staticcheck`, `govulncheck` et un court fuzzing des analyseurs binaires (`go test -fuzz FuzzICC`, `-fuzz FuzzImageMeta`).

Les fichiers `rsrc_windows_*.syso` contiennent l'icône et le manifeste (DPI, contrôles communs), générés avec [akavel/rsrc](https://github.com/akavel/rsrc) :
`rsrc -arch amd64 -ico icon.ico -manifest app.manifest -o rsrc_windows_amd64.syso` (idem `arm64`).

| Fichier | Rôle |
|---|---|
| `main_windows.go` | barre système, planification |
| `core.go` | choix des photos, pipeline image |
| `immich.go` | client API Immich |
| `colorprofile.go` | profil ICC → sRGB, orientation EXIF |
| `render.go`, `caption.go` | recadrage, carte lieu/date |
| `settings.go`, `settings.html` | page de réglages locale |
| `logfile.go` | journal avec rotation |
| `win_windows.go`, `install_windows.go` | API Windows, installation/désinstallation |
| `monitors_windows.go` | une image par écran (COM `IDesktopWallpaper`) |
| `events_windows.go` | déverrouillage, changements d'écran, sortie de veille |
| `update.go`, `update_windows.go` | mise à jour depuis les releases GitHub |
| `version.go` | version (injectée par la pipeline à partir du tag) |

### Publier une version

Change le numéro dans `version.go` (ex. `var appVersion = "1.5.0"`) et pousse sur `main` : la pipeline voit qu'aucune release `v1.5.0` n'existe, compile les deux exe, crée le tag et la release. Les applis installées se mettent à jour d'elles-mêmes dans les 24 h.

(Pousser un tag `vX.Y.Z` fonctionne aussi.) Les autres push sur `main` produisent seulement des exe de dev en artefacts. Une release existante n'est jamais écrasée ; chaque exe publié est accompagné d'une attestation de provenance (`gh attestation verify ImmichWallpaper-x64.exe --repo spartan-dou/immich-wallpaper`).
