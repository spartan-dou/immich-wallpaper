# Immich Wallpaper

Petite appli Windows (barre système) qui met en fond d'écran — et, en option, d'écran de verrouillage — des photos tirées d'un ou plusieurs albums [Immich](https://immich.app), avec le lieu et la date incrustés dans une carte façon verre dépoli. Un peu comme « Windows à la une », mais avec tes photos.

**Téléchargement** : onglet [Releases](../../releases) → `ImmichWallpaper-x64.exe` (PC Intel/AMD) ou `ImmichWallpaper-arm64.exe` (Windows sur ARM).

Petite appli Windows (barre système) qui met en fond d'écran des photos tirées d'un ou plusieurs albums Immich.

## Installation

Deux versions natives : `ImmichWallpaper-x64.exe` (PC Intel/AMD) et `ImmichWallpaper-arm64.exe` (PC Snapdragon / Windows sur ARM). Dans le doute : *Paramètres → Système → Informations système → Type du système*.

1. Lance `ImmichWallpaper.exe` (SmartScreen : *Informations complémentaires → Exécuter quand même*, exe non signé).
2. Il propose de s'installer : copie dans `%LOCALAPPDATA%\Programs\ImmichWallpaper\`, raccourci dans le menu Démarrer, lancement avec Windows, entrée dans *Paramètres → Applications installées*. « Non » = mode portable.
3. La page de réglages s'ouvre : URL + token → **Tester et charger les albums** → coche les albums → **Enregistrer**.

**Mise à jour** : lance simplement le nouvel exe téléchargé, il propose de remplacer la version installée (il arrête l'ancienne lui-même, la config est conservée).

## Mises à jour automatiques

L'appli vérifie les [releases](../../releases) de ce dépôt 2 min après le démarrage puis une fois par jour. Si une version plus récente existe, elle télécharge l'exe de son architecture, vérifie son empreinte avec `SHA256SUMS.txt`, se remplace (l'exe en cours est renommé en `.old`, Windows interdisant de l'écraser) et redémarre. Désactivable dans les réglages ; menu de l'icône → **Rechercher une mise à jour** pour forcer la vérification.

## Désinstallation

*Paramètres → Applications → Applications installées → Immich Wallpaper → Désinstaller*, ou menu de l'icône → **Désinstaller…**.
Supprime : le programme, le démarrage auto, le raccourci, la config (dont le token) et le journal. Le fond d'écran actuel reste en place.

## Clé API Immich

Immich → avatar → Paramètres du compte → Clés API → Nouvelle clé.
Permissions : `album.read`, `asset.read`, `asset.view`, `asset.download`.
Sans `asset.download`, l'appli fonctionne mais se rabat sur l'aperçu (1440 px) quand Immich redirige la pleine résolution vers l'original.

## Menu de l'icône

- **Image suivante** : change tout de suite.
- **Voir la photo dans Immich** : ouvre la photo affichée.
- **Paramètres…** : rouvre la page de réglages (servie localement sur 127.0.0.1, protégée par un jeton aléatoire, le token Immich n'y est jamais réaffiché).
- **Pause**, **Démarrer avec Windows**, **Ouvrir le journal**, **Quitter**.

## Légende et écran de verrouillage

- **Lieu / date** : carte façon verre dépoli (fond flouté, coins arrondis) incrustée sur l'image (ville, région, pays déduits du GPS par Immich ; rien si la photo n'est pas géolocalisée). L'image est alors recadrée par l'appli à la résolution de l'écran principal pour que le texte ne soit jamais rogné.
- **Écran de verrouillage** : option à cocher. Dans Windows, *Personnalisation → Écran de verrouillage → Personnaliser* doit être sur « Image ». En cas d'échec, le motif est dans le journal.
- L'appli repasse automatiquement l'arrière-plan du bureau en mode « Image » si « Windows à la une » ou un diaporama était actif.

## Ouvrir la photo dans Immich

Le fond d'écran n'est pas cliquable sous Windows. À la place : **clic gauche sur l'icône** près de l'horloge, ou l'icône **« Voir cette photo dans Immich »** déposée sur le bureau (désactivable dans les réglages).

## Couleurs

Immich génère par défaut ses aperçus en Display P3 (profil ICC intégré). Windows ne gère pas les profils pour le fond d'écran : l'appli convertit donc l'image en sRGB (et applique l'orientation EXIF des originaux) avant de l'appliquer.

## Fichiers

Tout est dans `%APPDATA%\ImmichWallpaper\` : `config.json` (éditable à la main aussi), `journal.txt`, et les deux images `wallpaper_a/b.jpg`.

## Comportement

- Tirage aléatoire, sans repasser par les photos affichées récemment ; liste des albums rafraîchie toutes les 6 h ou à chaque enregistrement.
- Vidéos et éléments en corbeille ignorés. Option « paysage uniquement » (si aucune paysage n'est trouvée, une portrait est prise).
- Qualité « Pleine résolution » = JPEG généré par Immich (gère HEIC/RAW) ; repli automatique sur l'aperçu si ton Immich ne génère pas de fullsize.
- Serveur injoignable (démarrage, hors VPN…) : l'image actuelle reste, nouvel essai toutes les 2 min.
- Même image sur tous les écrans (utilise « Étendre » pour une image à cheval sur plusieurs écrans).

## Développement

Go pur (pas de cgo), cross-compilable depuis n'importe quel OS :

```sh
GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go build -trimpath -ldflags "-H windowsgui -s -w" -o ImmichWallpaper-x64.exe .
GOOS=windows GOARCH=arm64 CGO_ENABLED=0 go build -trimpath -ldflags "-H windowsgui -s -w" -o ImmichWallpaper-arm64.exe .
go test ./...
```

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
| `win_windows.go`, `install_windows.go` | API Windows, installation/désinstallation |
| `update.go`, `update_windows.go` | mise à jour depuis les releases GitHub |
| `version.go` | version (injectée par la pipeline à partir du tag) |

### Publier une version

```sh
git tag v1.4.0 && git push origin v1.4.0
```
La pipeline compile les deux exe et crée la release GitHub correspondante ; les applis installées se mettent à jour d'elles-mêmes dans les 24 h. Chaque push sur `main` produit aussi les exe en artefacts de build.
