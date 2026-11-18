# Immich Wallpaper

Petite appli Windows (barre système) qui met en fond d'écran — et, en option, d'écran de verrouillage — des photos tirées d'un ou plusieurs albums [Immich](https://immich.app), avec le lieu et la date incrustés dans une carte façon verre dépoli. Un peu comme « Windows à la une », mais avec tes photos.

Écrite en Rust, sans runtime à installer : un seul exe, des réglages dans une fenêtre Windows classique, aucun port ouvert.

**Téléchargement** : onglet [Releases](../../releases), en deux versions, chacune pour PC Intel/AMD (`x64`) et Windows sur ARM (`arm64`) :

| Version | Fichier | Pour qui |
|---|---|---|
| **Installeur** | `ImmichWallpaper-setup-x64.exe` / `-setup-arm64.exe` | Usage normal : s'installe, se lance avec Windows, se désinstalle depuis les paramètres de Windows. |
| **Portable** | `ImmichWallpaper-portable-x64.zip` / `-portable-arm64.zip` | Sans installation, PC professionnel compris : à dézipper où tu veux, tout reste dans ce dossier (voir [PC professionnel](#pc-professionnel)). |

Les fichiers `ImmichWallpaper-x64.exe` / `-arm64.exe` sont l'exe classique : c'est lui que la mise à jour automatique des copies installées télécharge. Lancé à la main, il propose de s'installer ou de tourner sans installation. `ImmichWallpaper-portable-x64.exe` / `-arm64.exe` est l'ancienne version portable en un seul exe ; elle reste publiée pour les copies existantes et se comporte comme le zip.

## Installation

Architecture : `x64` pour un PC Intel/AMD, `arm64` pour un PC Snapdragon / Windows sur ARM. Dans le doute : *Paramètres → Système → Informations système → Type du système*.

### Avec l'installeur

1. Lance `ImmichWallpaper-setup-x64.exe` (SmartScreen : *Informations complémentaires → Exécuter quand même*, exe non signé).
2. Confirme l'installation : copie dans `%LOCALAPPDATA%\Programs\ImmichWallpaper\`, raccourci dans le menu Démarrer, lancement avec Windows, entrée dans *Paramètres → Applications installées*. Aucun droit administrateur n'est nécessaire. « Non » annule.
3. La fenêtre de réglages s'ouvre : onglet **Serveur** (URL + token) → **Tester et charger les albums** → onglet **Albums** : coche les albums → **Enregistrer**.

Installation sans question (déploiement automatisé) : `ImmichWallpaper-setup-x64.exe --quiet`.

**Mise à jour manuelle** : relance simplement un installeur plus récent, il remplace la version installée (il arrête l'ancienne lui-même, la config est conservée). Sinon, l'appli se met à jour seule (voir plus bas).

### Version portable

1. Dézippe `ImmichWallpaper-portable-x64.zip` dans un dossier où tu peux écrire (Documents, clé USB…), puis lance `ImmichWallpaper.exe`.
2. La fenêtre de réglages s'ouvre, comme ci-dessus.

Le fichier `portable.txt` à côté de l'exe indique à l'appli de ne rien installer (comme dans [omnictl](https://github.com/spartan-dou/omnictl)) ; il décrit aussi tout ce qu'elle fait sur le PC. Le supprimer rend l'appli « normale » : elle propose alors de s'installer.

Ce qui change par rapport à la version installée :

- Les réglages, le journal, le cache et les images sont dans le dossier `ImmichWallpaper-data` à côté de l'exe ; rien n'est écrit dans `%APPDATA%`.
- Rien n'est installé : ni raccourci, ni entrée dans les applications de Windows. Pour la retirer, quitte l'appli et supprime le dossier.
- Rien n'est posé sur le bureau par défaut : l'icône « Voir cette photo dans Immich » est décochée (à cocher dans les réglages si tu la veux).
- « Démarrer avec Windows » reste possible depuis le menu de l'icône (décoché par défaut) ; pense à le décocher avant de supprimer ou déplacer le dossier.
- **Mises à jour sur clic seulement** : une nouvelle version est annoncée dans le menu de l'icône (« Installer la version… ») ; rien n'est téléchargé ni remplacé sans ce clic et une confirmation.
- La clé API est chiffrée pour ton compte Windows sur ce PC : sur un autre PC ou un autre compte, il faut la ressaisir.
- Une seule instance tourne à la fois : la version portable ne se lance pas si la version installée est déjà en cours d'exécution.

### PC professionnel

La version portable est faite pour tourner sur un PC géré par un service informatique sans rien y installer ni déclencher l'antivirus. Ce qu'elle fait, et ne fait pas (détail dans `portable.txt`, à transmettre au besoin) :

| | |
|---|---|
| Droits | ceux de l'utilisateur, jamais d'administrateur ; ni service, ni pilote, ni tâche planifiée |
| Programmes lancés | aucun : ni PowerShell, ni cmd, ni script. L'écran de verrouillage passe par l'API de Windows (WinRT), dans le processus de l'appli |
| Fichiers | uniquement dans son dossier ; rien sur le bureau par défaut |
| Registre | `HKCU` seulement : réglages du fond d'écran (comme *Paramètres → Personnalisation*) ; entrée `Run` uniquement si « Démarrer avec Windows » est coché |
| Réseau | en sortie, HTTPS vérifié par Windows (Schannel, inspection TLS de l'entreprise comprise) : ton serveur Immich, et `api.github.com` une fois par jour pour annoncer une nouvelle version (désactivable) |
| Port local | aucun : les réglages sont une fenêtre de l'appli, rien n'écoute sur le réseau, pas même sur `127.0.0.1` |
| Mises à jour | jamais automatiques : sur clic, fichier vérifié par son empreinte SHA-256 |

À savoir :

- **L'exe n'est pas signé.** Au premier lancement, SmartScreen affiche « Windows a protégé votre ordinateur » : *Informations complémentaires → Exécuter quand même*. Si le PC n'autorise que les programmes approuvés (AppLocker, WDAC, Smart App Control), aucune version ne pourra s'y lancer sans l'accord du service informatique. Chaque fichier publié a une attestation de provenance vérifiable : `gh attestation verify ImmichWallpaper.exe --repo spartan-dou/immich-wallpaper`.
- **Le serveur Immich doit être joignable depuis le PC.** L'appli ne passe par aucun proxy (ni celui de Windows, ni les variables `HTTPS_PROXY`) : derrière un proxy obligatoire, elle reste sur les photos du cache hors ligne.
- **Écran de verrouillage** : souvent imposé par l'entreprise sur un PC géré. Dans ce cas l'option échoue proprement (motif dans le journal) et seul le bureau change.

## Fenêtre de réglages

Menu de l'icône → **Paramètres…** (elle s'ouvre aussi au premier lancement). Une fenêtre aux couleurs d'Immich, en thème clair ou sombre selon Windows (*Paramètres → Personnalisation → Couleurs*) : barre latérale, cartes arrondies, interrupteurs, et tes albums affichés avec leur photo de couverture, comme dans Immich.

| Page | Contenu |
|---|---|
| **Serveur** | URL, token (jamais réaffiché : laisser vide garde celui enregistré), certificat auto-signé, test de connexion |
| **Albums** | tes albums en grille avec leur couverture, à cocher ; recherche par nom |
| **Affichage** | intervalle, ajustement, qualité, plusieurs écrans, cache hors ligne, paysage uniquement, photos masquées à réafficher |
| **Planification** | plage horaire, pauses (batterie, connexion limitée, plein écran), préchargement |
| **Saisons et heures** | règles d'albums par saison et/ou plage horaire |
| **Légende** | lieu, date, position du texte, icône du bureau, écran de verrouillage |
| **Mises à jour** | mises à jour automatiques, version |

En bas : **Image suivante**, **Fermer** (touche Échap), qui propose d'enregistrer s'il reste des modifications, et **Enregistrer** (touche Entrée). L'état de l'appli (« prochaine à 14:05 », « en pause »…) est rappelé en bas de la barre latérale. Tout se fait aussi au clavier : Tab, Espace pour cocher, flèches dans la grille d'albums, Ctrl+Tab pour changer de page. Une erreur (URL invalide, heure mal écrite…) est expliquée et rien n'est enregistré.

Les vignettes de couverture sont téléchargées depuis ton serveur après le test de connexion (même token, même certificat épinglé), gardées en mémoire le temps de la session, jamais écrites sur le disque. La fenêtre est dessinée par l'appli elle-même (GDI+ de Windows), sans navigateur ni port ouvert, et reste nette à toutes les échelles d'affichage.

## Plusieurs écrans et mode hors ligne

- **Une image différente par écran** (Affichage → Plusieurs écrans) : chaque photo est rendue à la résolution exacte de son écran, avec sa propre légende (interface Windows `IDesktopWallpaper`). L'écran de verrouillage reprend la photo de l'écran principal.
- **Écran très large** (32:9, « deux écrans en un ») : deux photos côte à côte, chacune dans un panneau 16:9 avec sa légende vers le bord extérieur, au lieu d'une seule photo très rognée. Les 21:9 ne sont pas découpés. Désactivable dans les réglages.
- **Déverrouillage et changements d'écran** : nouvelle photo à chaque déverrouillage du PC (désactivable) ; quand un écran est branché/débranché, change de résolution ou se rallume, le fond d'écran est recalculé avec les photos actuelles (nouveau tirage seulement s'il en faut davantage, ex. écran 32:9 ajouté — et jamais pendant une pause). Événements reçus par une fenêtre cachée (`WM_WTSSESSION_CHANGE`, `WM_DISPLAYCHANGE`, état d'affichage de la console), regroupés sur 3 s.
- **Hors ligne** : les dernières photos téléchargées (40 par défaut) sont gardées dans `%APPDATA%\ImmichWallpaper\cache`. Si Immich est injoignable, la rotation continue avec elles ; le menu de l'icône l'indique.
- **Albums** : l'appli vérifie toutes les 30 min la liste des albums (une seule requête) et ne recharge le contenu que des albums modifiés (nombre de photos ou date de modification changés). Un album supprimé ou dont le partage a été retiré sort du tirage.

## Mises à jour automatiques

L'appli vérifie les [releases](../../releases) de ce dépôt 2 min après le démarrage puis une fois par jour. Si une version plus récente existe, elle télécharge l'exe de son architecture, vérifie son empreinte avec `SHA256SUMS.txt`, se remplace (le nouvel exe est écrit à côté puis mis en place par renommage ; l'exe en cours devient `.old`, Windows interdisant de l'écraser) et redémarre. Seuls les fichiers des releases de ce dépôt sont acceptés (adresse et redirections contrôlées). L'empreinte protège contre un téléchargement corrompu, pas contre une compromission du dépôt : les releases ne sont pas signées. L'ancienne version surveille le démarrage de la nouvelle : si celle-ci s'arrête dans les premières secondes, elle se remet en place et cette version est ignorée par les mises à jour automatiques. Désactivable dans les réglages ; menu de l'icône → **Rechercher une mise à jour** pour forcer la vérification.

**Version portable** : rien n'est installé sans toi. La vérification quotidienne annonce seulement la nouvelle version dans le menu de l'icône (« Installer la version… ») ; le téléchargement et le remplacement n'ont lieu qu'après ce clic et une confirmation. Le zip portable se met à jour avec l'exe classique, l'ancien exe portable avec l'exe portable.

Les versions jusqu'à la 1.10 étaient écrites en Go. Le format de `config.json`, les dossiers, les noms des fichiers publiés et le protocole de mise à jour n'ont pas changé : une copie existante passe à la version Rust par sa mise à jour habituelle, réglages et clé API compris.

## Désinstallation

*Paramètres → Applications → Applications installées → Immich Wallpaper → Désinstaller*, ou menu de l'icône → **Désinstaller…**.
Supprime : le programme, le démarrage auto, le raccourci, la config (dont le token) et le journal. Le fond d'écran actuel reste en place.

## Clé API Immich

Immich → avatar → Paramètres du compte → Clés API → Nouvelle clé.
Permissions : `album.read`, `asset.read`, `asset.view`, `asset.download` ; `asset.update` en plus si tu veux utiliser « Ajouter aux favoris Immich ».
Sans `asset.download`, l'appli fonctionne mais se rabat sur l'aperçu (1440 px) quand Immich redirige la pleine résolution vers l'original.

La clé est chiffrée dans `config.json` avec DPAPI (`api_key_protected`, lisible uniquement par ton compte Windows sur ce PC). Une clé écrite à la main dans `api_key` est acceptée et chiffrée au démarrage suivant. Elle n'est envoyée qu'au serveur pour lequel elle a été saisie : si tu changes l'URL du serveur, il faut la ressaisir.

**Certificat auto-signé** : l'option n'accepte pas n'importe quel certificat. Celui présenté lors de « Tester et charger les albums » est épinglé (`cert_fingerprint`, empreinte SHA-256) ; s'il change, la connexion est refusée jusqu'à un nouveau test suivi d'un enregistrement.

## Menu de l'icône

- **Image suivante** : change tout de suite.
- **Photo précédente** : réaffiche le fond d'écran précédent (jusqu'à 5 retours en arrière, depuis le lancement de l'appli) et repousse le prochain changement.
- **Voir la photo dans Immich** : ouvre la photo affichée.
- **Ajouter aux favoris Immich** : marque la photo affichée comme favorite (la clé API doit avoir la permission `asset.update`).
- **Ne plus afficher cette photo** : la masque et passe à la suivante. La liste est locale (`excluded.json`), rien n'est modifié dans Immich ; l'onglet Affichage des réglages indique le nombre de photos masquées et permet de les réafficher.

Avec plusieurs photos affichées (plusieurs écrans, écran très large), ces actions portent sur celle de l'écran principal.
- **Paramètres…** : ouvre la fenêtre de réglages (ou la ramène au premier plan).
- **Pause**, **Démarrer avec Windows**, **Ouvrir le journal**.
- **Rechercher une mise à jour** : vérifie tout de suite et propose l'installation (version portable : « Installer la version… » quand une nouvelle version a été trouvée).
- **Désinstaller…** : supprime le programme, ses réglages et le démarrage automatique (pas dans la version portable).
- **Quitter**.

## Planification

Réglages → **Planification** et **Saisons et heures**. Ces réglages ne concernent que les changements automatiques (intervalle, déverrouillage) : **Image suivante** fonctionne toujours.

- **Plage horaire** : les changements n'ont lieu qu'entre deux heures (une plage comme 22:00 → 07:00 passe par minuit). Décocher les deux heures = toute la journée.
- **Pas de changement** sur batterie, sur une connexion limitée (partage de connexion, forfait de données) ou pendant une application en plein écran (jeu, vidéo, présentation). Le changement en attente se fait dès que la condition disparaît ; l'état l'indique (« en attente (sur batterie) »).
- **Albums selon la saison ou l'heure** (onglet Saisons et heures) : chaque règle associe des albums à une saison (printemps mars-mai, été juin-août, automne sept.-nov., hiver déc.-fév.) et/ou à une plage horaire. Quand une règle s'applique, ses albums remplacent les albums habituels ; plusieurs règles simultanées sont réunies.
- **Photo suivante téléchargée à l'avance** (activé par défaut) : après chaque changement, la photo suivante est téléchargée dans `%APPDATA%\ImmichWallpaper\preload`, avec son lieu et sa date. Le changement suivant est alors immédiat et ne dépend plus du réseau, en particulier au déverrouillage du PC. Rien n'est préchargé sur batterie, connexion limitée ou en plein écran si ces pauses sont activées.

## Légende et écran de verrouillage

- **Lieu / date** : carte façon verre dépoli (fond flouté, coins arrondis) incrustée sur l'image (ville, région, pays déduits du GPS par Immich ; rien si la photo n'est pas géolocalisée), dans la police de Windows (Segoe UI, sinon Arial). L'image est alors recadrée par l'appli à la résolution de l'écran principal pour que le texte ne soit jamais rogné.
- **Écran de verrouillage** : option à cocher. Dans Windows, *Personnalisation → Écran de verrouillage → Personnaliser* doit être sur « Image ». L'image est appliquée par l'API de Windows (`Windows.System.UserProfile.LockScreen`), sans lancer PowerShell. En cas d'échec, le motif est dans le journal (accès refusé : l'écran de verrouillage est probablement imposé par une stratégie).
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

Rust (version fixée par `rust-toolchain.toml`), appels directs à l'API Windows par la crate `windows` : icône de la barre système, menu, fenêtre de réglages et événements système sans bibliothèque d'interface tierce. Depuis Linux, avec [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) dans le `PATH` :

```sh
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER=aarch64-w64-mingw32-clang
cargo build --release --target x86_64-pc-windows-gnullvm      # exe classique
IW_FLAVOR=setup cargo build --release --target x86_64-pc-windows-gnullvm
IW_FLAVOR=portable cargo build --release --target aarch64-pc-windows-gnullvm
```

`IW_VERSION` fixe la version affichée (sinon celle de `Cargo.toml`). Sous Windows, `cargo build --release` suffit.

Tests :

```sh
cargo fmt --check
cargo test                                                # partie indépendante de Windows
cargo test --no-run --target x86_64-pc-windows-gnullvm    # puis lancer l'exe de test sous Windows
```

Sous Windows, la suite comprend la fenêtre de réglages (créée, remplie depuis une configuration, relue à l'identique, grilles d'albums et vignettes, règles, interrupteurs, enregistrement, erreurs, changement d'échelle d'affichage). Deux tests supplémentaires sont activés par des variables d'environnement :

- `IW_APP_EXE=<chemin de ImmichWallpaper.exe>` : parcours complet de la vraie appli face à un faux serveur Immich (icône, menu, fenêtre de réglages, tirage, image suivante, photo précédente, changement d'écran, pause, favori, exclusion, préchargement).
- `IW_NET_TESTS=1` : TLS et épinglage de certificat face à un vrai serveur, lecture de la dernière release.

`IW_SETTINGS_SHOTS=<dossier>` enregistre une capture de chaque page de la fenêtre de réglages, en thème clair et sombre. Le parcours complet utilise le mode de vérification de l'appli, utilisable aussi à la main : `ImmichWallpaper.exe --selftest resultat.txt [--selftest-dir dossier]`. Dans ce mode, rien n'est appliqué au bureau : les images sont rendues dans le dossier de test, le fond d'écran, l'écran de verrouillage, le démarrage automatique et la configuration réelle ne sont pas touchés.

La pipeline exécute `cargo fmt --check` et les tests sous Linux et sous Windows, puis compile les exe avec llvm-mingw (version et empreinte fixées).

**Icône et manifeste** (DPI par écran, contrôles communs v6, droits de l'utilisateur) : `res/app.rc` est compilé une fois pour toutes en objets COFF, passés tels quels à l'éditeur de liens par `build.rs`. Après modification de `res/icon.ico` ou `res/app.manifest`, depuis `res/` :

```sh
x86_64-w64-mingw32-windres app.rc -O coff -o app-x64.o
aarch64-w64-mingw32-windres app.rc -O coff -o app-arm64.o
```

| Fichier | Rôle |
|---|---|
| `src/main.rs` | point d'entrée, version, variante, détection de `portable.txt` |
| `src/core.rs` | albums, choix des photos, cache hors ligne, préchargement, rendu par écran |
| `src/immich.rs` | client API Immich, TLS (Schannel) et épinglage |
| `src/colorprofile.rs` | lecture des images, profil ICC → sRGB, orientation EXIF |
| `src/render.rs`, `src/caption.rs` | recadrage, carte lieu/date |
| `src/config.rs` | configuration, plage horaire, règles d'albums |
| `src/settings.rs` | réglages : lecture, test de connexion, enregistrement (indépendant de l'interface) |
| `src/update.rs` | recherche et téléchargement d'une mise à jour |
| `src/logfile.rs`, `src/util.rs` | journal avec rotation, erreurs, utilitaires |
| `src/win/app.rs` | icône, menu, planification, événements système, mode de vérification |
| `src/win/settings_window.rs` | fenêtre de réglages (dessin, thème, grille d'albums) |
| `src/win/platform.rs` | fond d'écran, registre, DPAPI, écran de verrouillage (WinRT) |
| `src/win/monitors.rs`, `src/win/context.rs` | une image par écran ; batterie, plein écran, connexion |
| `src/win/install.rs`, `src/win/updater.rs` | installation, désinstallation, mise à jour |
| `res/` | icône, manifeste et leurs objets compilés |
| `packaging/portable.txt` | fichier témoin du zip portable (et ce que fait l'appli sur le PC) |

### Publier une version

Change le numéro dans `Cargo.toml` (ex. `version = "1.11.0"`) et pousse sur `main` : la pipeline voit qu'aucune release `v1.11.0` n'existe, compile les exe (classique, installeur et portable, en x64 et ARM64) et les zips portables, crée le tag et la release. Les applis installées se mettent à jour d'elles-mêmes dans les 24 h.

(Pousser un tag `vX.Y.Z` fonctionne aussi.) Les autres push sur `main` produisent seulement des exe de dev en artefacts. Une release existante n'est jamais écrasée ; chaque exe publié est accompagné d'une attestation de provenance (`gh attestation verify ImmichWallpaper-x64.exe --repo spartan-dou/immich-wallpaper`).
