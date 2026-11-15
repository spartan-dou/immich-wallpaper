# Immich Wallpaper

Petite appli Windows (barre système) qui met en fond d'écran — et, en option, d'écran de verrouillage — des photos tirées d'un ou plusieurs albums [Immich](https://immich.app), avec le lieu et la date incrustés dans une carte façon verre dépoli. Un peu comme « Windows à la une », mais avec tes photos.

**Téléchargement** : onglet [Releases](../../releases), en deux versions, chacune pour PC Intel/AMD (`x64`) et Windows sur ARM (`arm64`) :

| Version | Fichier | Pour qui |
|---|---|---|
| **Installeur** | `ImmichWallpaper-setup-x64.exe` / `-setup-arm64.exe` | Usage normal : s'installe, se lance avec Windows, se désinstalle depuis les paramètres de Windows. |
| **Portable** | `ImmichWallpaper-portable-x64.exe` / `-portable-arm64.exe` | Sans installation : l'exe et ses données restent dans le dossier où tu le poses (clé USB, dossier personnel…). |

Les fichiers `ImmichWallpaper-x64.exe` / `-arm64.exe` sont l'exe classique : c'est lui que la mise à jour automatique des copies installées télécharge. Lancé à la main, il propose de s'installer ou de tourner sans installation.

## Installation

Architecture : `x64` pour un PC Intel/AMD, `arm64` pour un PC Snapdragon / Windows sur ARM. Dans le doute : *Paramètres → Système → Informations système → Type du système*.

### Avec l'installeur

1. Lance `ImmichWallpaper-setup-x64.exe` (SmartScreen : *Informations complémentaires → Exécuter quand même*, exe non signé).
2. Confirme l'installation : copie dans `%LOCALAPPDATA%\Programs\ImmichWallpaper\`, raccourci dans le menu Démarrer, lancement avec Windows, entrée dans *Paramètres → Applications installées*. Aucun droit administrateur n'est nécessaire. « Non » annule.
3. La page de réglages s'ouvre : URL + token → **Tester et charger les albums** → coche les albums → **Enregistrer**.

Installation sans question (déploiement automatisé) : `ImmichWallpaper-setup-x64.exe --quiet`.

**Mise à jour manuelle** : relance simplement un installeur plus récent, il remplace la version installée (il arrête l'ancienne lui-même, la config est conservée). Sinon, l'appli se met à jour seule (voir plus bas).

### Version portable

1. Pose `ImmichWallpaper-portable-x64.exe` dans un dossier où tu peux écrire, puis lance-le.
2. La page de réglages s'ouvre, comme ci-dessus.

Ce qui change par rapport à la version installée :

- Les réglages, le journal, le cache et les images sont dans le dossier `ImmichWallpaper-data` à côté de l'exe ; rien n'est écrit dans `%APPDATA%`.
- Rien n'est installé : ni raccourci, ni entrée dans les applications de Windows. Pour la retirer, quitte l'appli et supprime le dossier.
- « Démarrer avec Windows » reste possible depuis le menu de l'icône (décoché par défaut) ; pense à le décocher avant de supprimer ou déplacer le dossier.
- La mise à jour automatique remplace l'exe portable par la nouvelle version portable.
- La clé API est chiffrée pour ton compte Windows sur ce PC : sur un autre PC ou un autre compte, il faut la ressaisir.
- Une seule instance tourne à la fois : la version portable ne se lance pas si la version installée est déjà en cours d'exécution.

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
Permissions : `album.read`, `asset.read`, `asset.view`, `asset.download` ; `asset.update` en plus si tu veux utiliser « Ajouter aux favoris Immich ».
Sans `asset.download`, l'appli fonctionne mais se rabat sur l'aperçu (1440 px) quand Immich redirige la pleine résolution vers l'original.

La clé est chiffrée dans `config.json` avec DPAPI (`api_key_protected`, lisible uniquement par ton compte Windows sur ce PC). Une clé écrite à la main dans `api_key` est acceptée et chiffrée au démarrage suivant. Elle n'est envoyée qu'au serveur pour lequel elle a été saisie : si tu changes l'URL du serveur, il faut la ressaisir.

**Certificat auto-signé** : l'option n'accepte plus n'importe quel certificat. Celui présenté lors de « Tester et charger les albums » est épinglé (`cert_fingerprint`, empreinte SHA-256) ; s'il change, la connexion est refusée jusqu'à un nouveau test suivi d'un enregistrement.

## Menu de l'icône

- **Image suivante** : change tout de suite.
- **Photo précédente** : réaffiche le fond d'écran précédent (jusqu'à 5 retours en arrière, depuis le lancement de l'appli) et repousse le prochain changement.
- **Voir la photo dans Immich** : ouvre la photo affichée.
- **Ajouter aux favoris Immich** : marque la photo affichée comme favorite (la clé API doit avoir la permission `asset.update`).
- **Ne plus afficher cette photo** : la masque et passe à la suivante. La liste est locale (`excluded.json`), rien n'est modifié dans Immich ; la page de réglages indique le nombre de photos masquées et permet de les réafficher.

Avec plusieurs photos affichées (plusieurs écrans, écran très large), ces actions portent sur celle de l'écran principal.
- **Paramètres…** : rouvre la page de réglages (servie localement sur 127.0.0.1, protégée par un jeton aléatoire, le token Immich n'y est jamais réaffiché).
- **Pause**, **Démarrer avec Windows**, **Ouvrir le journal**.
- **Rechercher une mise à jour** : vérifie tout de suite et propose l'installation.
- **Désinstaller…** : supprime le programme, ses réglages et le démarrage automatique.
- **Quitter**.

## Planification

Réglages → **Planification**. Ces réglages ne concernent que les changements automatiques (intervalle, déverrouillage) : **Image suivante** fonctionne toujours.

- **Plage horaire** : les changements n'ont lieu qu'entre deux heures (une plage comme 22:00 → 07:00 passe par minuit).
- **Pas de changement** sur batterie, sur une connexion limitée (partage de connexion, forfait de données) ou pendant une application en plein écran (jeu, vidéo, présentation). Le changement en attente se fait dès que la condition disparaît ; l'état l'indique (« en attente (sur batterie) »).
- **Albums selon la saison ou l'heure** : chaque règle associe des albums à une saison (printemps mars-mai, été juin-août, automne sept.-nov., hiver déc.-fév.) et/ou à une plage horaire. Quand une règle s'applique, ses albums remplacent les albums habituels ; plusieurs règles simultanées sont réunies.
- **Photo suivante téléchargée à l'avance** (activé par défaut) : après chaque changement, la photo suivante est téléchargée dans `%APPDATA%\ImmichWallpaper\preload`, avec son lieu et sa date. Le changement suivant est alors immédiat et ne dépend plus du réseau, en particulier au déverrouillage du PC. Rien n'est préchargé sur batterie, connexion limitée ou en plein écran si ces pauses sont activées.

## Légende et écran de verrouillage

- **Lieu / date** : carte façon verre dépoli (fond flouté, coins arrondis) incrustée sur l'image (ville, région, pays déduits du GPS par Immich ; rien si la photo n'est pas géolocalisée). L'image est alors recadrée par l'appli à la résolution de l'écran principal pour que le texte ne soit jamais rogné.
- **Écran de verrouillage** : option à cocher. Dans Windows, *Personnalisation → Écran de verrouillage → Personnaliser* doit être sur « Image ». En cas d'échec, le motif est dans le journal.
- L'appli repasse automatiquement l'arrière-plan du bureau en mode « Image » si « Windows à la une » ou un diaporama était actif.

## Ouvrir la photo dans Immich

Le fond d'écran n'est pas cliquable sous Windows. À la place : **clic gauche sur l'icône** près de l'horloge, ou l'icône **« Voir cette photo dans Immich »** déposée sur le bureau (désactivable dans les réglages).

## Couleurs

Immich génère par défaut ses aperçus en Display P3 (profil ICC intégré). Windows ne gère pas les profils pour le fond d'écran : l'appli convertit donc l'image en sRGB (et applique l'orientation EXIF des originaux) avant de l'appliquer.

## Fichiers

Tout est dans `%APPDATA%\ImmichWallpaper\` (version portable : dans `ImmichWallpaper-data` à côté de l'exe) : `config.json` (éditable à la main aussi), `journal.txt` (1 Mo max, le précédent est gardé dans `journal.1.txt`), `recent.json` (photos affichées récemment, pour ne pas les revoir après un redémarrage), `excluded.json` (photos masquées), les dossiers `cache` (hors ligne) et `preload` (photo suivante) et les deux images `wallpaper_a/b.jpg`.

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
# installeur ou version portable : ajouter -X main.flavor=setup (ou portable) aux ldflags
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
| `schedule.go` | plage horaire, règles d'albums par saison/heure |
| `context_windows.go` | batterie, plein écran, connexion limitée |
| `win_windows.go`, `install_windows.go` | API Windows, installation/désinstallation |
| `monitors_windows.go` | une image par écran (COM `IDesktopWallpaper`) |
| `events_windows.go` | déverrouillage, changements d'écran, sortie de veille |
| `update.go`, `update_windows.go` | mise à jour depuis les releases GitHub |
| `version.go` | version (injectée par la pipeline à partir du tag) |

### Publier une version

Change le numéro dans `version.go` (ex. `var appVersion = "1.5.0"`) et pousse sur `main` : la pipeline voit qu'aucune release `v1.5.0` n'existe, compile les exe (classique, installeur et portable, en x64 et ARM64), crée le tag et la release. Les applis installées se mettent à jour d'elles-mêmes dans les 24 h.

(Pousser un tag `vX.Y.Z` fonctionne aussi.) Les autres push sur `main` produisent seulement des exe de dev en artefacts. Une release existante n'est jamais écrasée ; chaque exe publié est accompagné d'une attestation de provenance (`gh attestation verify ImmichWallpaper-x64.exe --repo spartan-dou/immich-wallpaper`).
