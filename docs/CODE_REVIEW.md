# Revue de code — Immich Wallpaper

- **Périmètre** : commit `45f9d38` (v1.7.1), branche `main`. Tous les fichiers suivis ont été lus : sources Go, `settings.html`, workflow CI, manifeste, `go.mod`/`go.sum`, README, tests.
- **Méthode** : lecture statique. **Go n'est pas installé sur la machine d'analyse** : ni `go vet`, ni `go test`, ni `govulncheck` n'ont été exécutés. Les constats sont donc établis par lecture ; quand un point repose sur un raisonnement non exécuté ou sur le comportement d'un système externe (API Immich, Windows), il est marqué **(hypothèse)**.
- **Dépendances** : versions de `go.mod` interrogées une à une sur la base publique OSV (api.osv.dev) le 2026-11-14.
- **Sévérités** : critique / élevée / moyenne / faible. Aucun constat critique.

## Résumé — les 5 points les plus importants

1. **[Élevée — sécurité] La mise à jour automatique n'est pas authentifiée.** L'empreinte SHA256 est téléchargée depuis la même release que l'exécutable (`update.go:134-165`) : elle détecte une corruption, pas une compromission. Avec `auto_update` actif par défaut (`config.go:49`), une release publiée à chaque push sur `main` qui change `version.go` (`.github/workflows/build.yml:46-50`), des actions CI référencées par tag mutable et un jeton `contents: write` sur tout le workflow (`build.yml:13-14,22,24,71`), toute compromission du dépôt ou de la CI devient une exécution de code sur tous les postes installés en moins de 24 h. → Signer les releases (clé publique embarquée) et durcir la CI.
2. **[Élevée — performance/RAM] Les portraits rejetés coûtent le pipeline complet et restent en mémoire.** Avec `landscape_only` (défaut), chaque portrait tiré est téléchargé, entièrement décodé, converti, enrichi par un appel `AssetInfo`, réencodé dans le cache puis conservé décodé dans une tranche (`core.go:240-262`), jusqu'à 15 essais par photo demandée (`core.go:227`). C'est à l'opposé de l'objectif de la v1.7.1 (réduction de RAM) et cela pollue le cache hors ligne avec des portraits.
3. **[Moyenne — sécurité] Dépendances datées avec avis publiés, sur un chemin qui décode des images non fiables, sans `recover`.** OSV liste 11 avis pour `golang.org/x/image v0.24.0` dont deux dans le décodeur WebP utilisé par l'appli (`core.go:24`). Aucune goroutine n'a de `recover()` : un panic de décodage tue silencieusement l'appli (binaire `windowsgui`, pas de sortie d'erreur).
4. **[Moyenne — sécurité] La clé API peut être envoyée à un serveur arbitraire et est stockée en clair.** `mergeKey` réinjecte la clé enregistrée quel que soit le `server_url` reçu (`settings.go:89,107,148-156`), ce qui contourne la garantie « la clé n'est jamais renvoyée à la page » (`settings.go:81`). La clé est en clair dans `config.json` (`config.go:16,96`), et `insecure_skip_verify` désactive toute vérification TLS (`immich.go:33-34`).
5. **[Moyenne — design/robustesse] Tout le tirage (réseau compris) s'exécute sous le mutex de `Core`, et plusieurs comportements divergent de la configuration.** Le menu de la barre système peut se figer pendant un téléchargement (`core.go:453-454` + `main_windows.go:257`) ; le style « Étendre » est écrasé en « Remplir » dès que la légende est active (`core.go:538-540`) ; un changement d'écran déclenche un nouveau tirage même en pause (`core.go:419-422`).

---

## 1. Performance

### P1 — Portraits : pipeline complet pour des images rejetées — **élevée**
`core.go:240-262`, `core.go:227`, `config.go:43`

`prepareImage` détecte le portrait à partir du seul en-tête (`core.go:626-633`), mais le code poursuit quand même : décodage complet + conversion ICC (`core.go:245`), requête `AssetInfo` (`core.go:252`), `saveToCache` (redimensionnement CatmullRom + JPEG q95 + écriture disque + purge, `core.go:258`), puis stockage de l'image **décodée** dans `portraits` (`core.go:260`). La boucle autorise `15*n` essais : dans un album majoritairement en portrait, jusqu'à 15 images pleine résolution décodées coexistent en mémoire (une image de 24 Mpx en RGBA pèse environ 96 Mo), alors que la limite souple est de 256 Mo (`main_windows.go:46`).

Effet de bord fonctionnel : ces portraits entrent dans le cache hors ligne, et `cachedPhotos` (`core.go:345-372`) ne filtre pas l'orientation. En mode hors ligne, `landscape_only` n'est donc plus respecté.

**Recommandation** :
- Sur `errNotLandscape`, ne rien décoder : garder au plus **un** portrait de secours sous forme d'octets compressés, sans `AssetInfo` ni mise en cache, et ne le décoder que si aucun paysage n'a été trouvé.
- Mieux : filtrer avant téléchargement à partir des dimensions fournies par Immich (**hypothèse** : les réponses d'assets exposent largeur/hauteur/orientation via `exifInfo` ; à vérifier sur les versions d'Immich ciblées), et mémoriser l'orientation par asset pour ne jamais retélécharger un portrait connu.

### P2 — Toutes les photos d'un tirage sont décodées simultanément — **moyenne**
`core.go:219-275`, `core.go:459-477`, `main_windows.go:46`

`fetchPhotos(n)` renvoie `n` images décodées, puis `applyTo` les rend. En mode « une image par écran » avec écran 32:9, `n` vaut 3 ou plus : les sources décodées, les `image.RGBA` de rendu (33 Mo par écran 4K) et les tampons JPEG coexistent. Au-delà de 256 Mo de tas vivant, la limite souple fait tourner le GC en continu au lieu de borner la mémoire.

**Recommandation** : traiter écran par écran (télécharger → décoder → rendre → écrire → libérer) et ne conserver entre deux étapes que les octets compressés.

### P3 — Chaque photo est redimensionnée et réencodée deux fois à l'identique — **moyenne**
`core.go:293`, `core.go:481`, `core.go:306-312`

`shrinkJPEG` (redimensionnement CatmullRom à 3840 px + JPEG q95) est appelé une première fois dans `saveToCache`, puis une seconde fois sur la même image pour remplir `lastPhotos`. En mode hors ligne, on réencode même un JPEG qui vient d'être lu sur disque (`core.go:362` puis `core.go:481`). CatmullRom est le noyau le plus coûteux de `x/image/draw` et tout est mono-thread en Go pur.

**Recommandation** : calculer le JPEG réduit une seule fois par photo et le partager entre le cache et `lastPhotos` (ou faire pointer `lastPhotos` vers le fichier du cache). Pour la réduction, `ApproxBiLinear` ou `BiLinear` suffit.

### P4 — Entrées/sorties réseau et PowerShell sous le mutex de `Core` — **moyenne**
`core.go:453-454`, `core.go:121-128`, `core.go:94-104`, `core.go:503`, `main_windows.go:216,257`, `win_windows.go:157-172`

`next()` garde `c.mu` pendant les téléchargements (timeout de 2 min par requête, `immich.go:39`, jusqu'à `15*n` essais) et pendant l'appel PowerShell de l'écran de verrouillage (timeout d'une minute). Or `Core.Last()` prend le même verrou et est appelé de façon synchrone dans la boucle d'événements du menu (`main_windows.go:257`) : un clic sur « Voir la photo dans Immich » pendant un tirage bloque toute la boucle (Pause, Quitter, Paramètres compris) jusqu'à la fin du tirage. `SetConfig` (appelé par `api/save`) bloque de la même façon la requête HTTP d'enregistrement.

**Recommandation** : ne tenir le verrou que pour lire/écrire l'état (instantané de config, `recent`, `last`) ; faire le réseau, le rendu et PowerShell hors verrou. `last` peut être un `atomic.Pointer`.

### P5 — `orient()` passe par les interfaces pixel par pixel — **faible**
`colorprofile.go:343-364`

`dst.Set(..., src.At(...))` alloue et convertit pour chaque pixel ; sur un original de 24 à 50 Mpx cela représente plusieurs secondes. Ne concerne en pratique que `image_size=original` (**hypothèse** : les miniatures générées par Immich sont déjà redressées).

**Recommandation** : convertir une fois en `*image.RGBA` (`draw.Draw`) puis permuter directement dans `Pix`. `toSRGB` (`colorprofile.go:316-325`) peut au passage être parallélisé par bandes de lignes.

### P6 — Chargement des albums : JSON complet et double requête périodique — **faible**
`immich.go:88`, `immich.go:196-199`, `core.go:134-185`

`GET /api/albums/{id}` rapatrie tous les assets avec tous leurs champs alors que 4 champs sont lus (`immich.go:18-23`). `Albums()` fait deux requêtes (possédés puis partagés) toutes les 30 min. Si un album configuré n'apparaît plus dans les signatures (supprimé, partage retiré), `refreshAlbums` le recharge intégralement à chaque vérification (`core.go:148-152`) et, en cas d'échec, conserve indéfiniment son ancien contenu (`core.go:157-158`).

**Recommandation** : demander l'album sans ses assets et toujours paginer via la recherche (**hypothèse** : paramètre `withoutAssets` de l'API Immich, à vérifier) ; retirer du tirage un album devenu inaccessible et le signaler dans le statut.

### P7 — Réveils périodiques et absence d'annulation — **faible**
`main_windows.go:240`, `immich.go:49`, `settings.html:269`

Un ticker de 10 s tourne en permanence pour comparer l'heure à `a.next` ; les requêtes HTTP n'ont pas de `context` (impossible d'annuler un téléchargement à la fermeture ou au changement de config) et le timeout global de 2 min inclut la lecture du corps, ce qui peut couper un original volumineux sur une liaison lente.

**Recommandation** : `time.Timer` réarmé sur `a.next` ; `http.NewRequestWithContext` avec un contexte annulé à l'arrêt ; timeouts distincts (connexion/en-têtes vs corps).

---

## 2. Sécurité

### S1 — Mise à jour automatique sans authentification de l'éditeur — **élevée**
`update.go:134-165`, `update.go:119-126`, `update_windows.go:127-191`, `config.go:49`, `README.md:13`

`SHA256SUMS.txt` et l'exécutable proviennent de la même release : qui peut remplacer l'un peut remplacer l'autre. La vérification ne protège donc que contre un téléchargement corrompu. L'exécutable n'est pas signé (le README le dit, ligne 13), `auto_update` est actif par défaut et l'installation automatique est silencieuse. Points positifs vérifiés : pas de retour à une version antérieure possible (`update.go:52-64`), TLS standard, contrôle de l'en-tête `MZ`, retour arrière si la nouvelle version plante.

**Recommandation** :
- Signer `SHA256SUMS.txt` (minisign/Ed25519) avec une clé privée hors du dépôt, embarquer la clé publique dans le binaire et refuser toute mise à jour sans signature valide. À terme, signature Authenticode.
- Vérifier que `browser_download_url` pointe bien vers `github.com`/`objects.githubusercontent.com` avant téléchargement.
- Envisager « notifier » plutôt qu'« installer » comme comportement par défaut tant que les releases ne sont pas signées.

### S2 — Chaîne de build : permissions larges, actions non épinglées, release sans validation — **moyenne**
`.github/workflows/build.yml:13-14`, `build.yml:22,24,71`, `build.yml:26`, `build.yml:46-50`, `build.yml:82`

- `permissions: contents: write` s'applique à tout le workflow, y compris aux exécutions `pull_request` (pour une PR issue d'une branche du dépôt, le jeton a les droits d'écriture ; GitHub ne le réduit que pour les forks).
- Les actions sont référencées par tag mutable (`actions/checkout@v4`, `actions/setup-go@v5`, `actions/upload-artifact@v4`).
- `go-version: stable` : la chaîne de compilation change sans trace dans le dépôt, les binaires ne sont pas reproductibles.
- Un push sur `main` qui modifie `version.go` publie immédiatement une release que les clients installent seuls ; `gh release upload --clobber` permet de remplacer les binaires d'une release existante.

Combiné à S1, c'est le chemin d'attaque le plus rentable du projet.

**Recommandation** : séparer un job `build` (`contents: read`, tous événements) d'un job `release` (`contents: write`, uniquement push sur `main`/tag, idéalement derrière un *environment* avec approbation) ; épingler les actions par SHA de commit ; fixer la version de Go (`go-version-file: go.mod` + directive `toolchain`) ; retirer `--clobber` ; publier une attestation de provenance (`actions/attest-build-provenance`).

### S3 — Dépendances avec avis de sécurité publiés — **moyenne**
`go.mod:4-13`, `core.go:21-24`, `caption.go:27`

Résultat OSV pour les versions exactes de `go.mod` :

| Module | Avis | Concerne le code ? |
|---|---|---|
| `golang.org/x/image v0.24.0` | 11 avis, dont GO-2026-5061 (panic WebP, canal alpha VP8) et GO-2026-6222 (allocation excessive, décodage VP8L) | **Oui** : `x/image/webp` est importé (`core.go:24`) et décode des données venant du serveur |
| idem | GO-2026-4962 (allocation excessive, SFNT) | Faible : seules des polices locales ou embarquées sont lues (`caption.go:24-34`) |
| idem | avis TIFF/BMP (GO-2026-4815, -5031, -5032, -5062, -5066), GO-2026-4961 (WebP 32 bits) | Non : paquets non importés, cibles 64 bits uniquement |
| `golang.org/x/sys v0.30.0` | GO-2026-5024 (`NewNTUnicodeString`) | Non : fonction non appelée |
| `golang.org/x/text v0.22.0` (indirecte) | GO-2026-5970, GO-2026-6629 | Non vérifié (dépendance indirecte) |
| `fyne.io/systray v1.12.2`, `godbus/dbus v5.1.0` | aucun | — |

L'atteignabilité n'a pas été confirmée par `govulncheck` (Go absent). Aucune configuration Dependabot/Renovate n'existe (`.github/` ne contient que `workflows/build.yml`) et `go.mod` déclare `go 1.22`.

**Recommandation** : `go get -u golang.org/x/image golang.org/x/sys golang.org/x/text && go mod tidy` ; ajouter `govulncheck ./...` à la CI ; ajouter `.github/dependabot.yml` (écosystèmes `gomod` et `github-actions`).

### S4 — Aucune récupération de panic ni borne sur les dimensions d'image — **moyenne**
`main_windows.go:150-192,246,255,344,353`, `core.go:626`, `colorprofile.go:306-314`, `immich.go:152`

- Aucun `recover()` dans le dépôt. `change()` et `reapply()` tournent dans des goroutines : un panic dans un décodeur (voir S3) ou dans les analyseurs binaires maison (`colorprofile.go`) termine le processus sans message (binaire `-H windowsgui`).
- `prepareImage` lit les dimensions mais ne les borne pas : une image de 30 000 × 30 000 px déclenche une allocation de plusieurs Go ; la limite de 200 Mo porte sur le fichier compressé (`immich.go:152`), pas sur les pixels.
- **(hypothèse, non exécutée)** `toSRGB` : un profil ICC paramétrique avec gamma négatif donne `+Inf` pour la valeur 0 (`colorprofile.go:198`), puis `NaN` après produit matriciel ; `q()` ne filtre pas `NaN` et indexe `enc[int(NaN)]` (`colorprofile.go:313`), ce qui provoque un panic sur amd64. Atteignable seulement avec `image_size=original` et un fichier malveillant dans un album (y compris partagé par un tiers).

**Recommandation** : `defer` + `recover` journalisé autour de `change`/`reapply` ; refuser au-delà d'un plafond de pixels (par ex. 120 Mpx) juste après `DecodeConfig` ; traiter `NaN` dans `q()` ; ajouter des tests de fuzzing (`go test -fuzz`) sur `jpegMeta`, `exifOrientation`, `webpICC`, `parseICC`, `parseCurve`, `decodeNormalized`.

### S5 — La clé API enregistrée est réutilisée vers n'importe quel `server_url` — **moyenne**
`settings.go:89`, `settings.go:107`, `settings.go:148-156`, `settings.go:81`

`api/albums` et `api/save` complètent une clé vide par la clé enregistrée sans vérifier que le serveur demandé est celui pour lequel elle a été saisie. Quiconque connaît l'URL à jeton de la page (historique du navigateur, extension, autre processus de la session) peut envoyer `{"server_url":"https://attaquant", "api_key":""}` à `api/albums` : la clé part dans l'en-tête `x-api-key` vers ce serveur (`immich.go:53`). La page elle-même ne reçoit jamais la clé, mais cette primitive permet de l'exfiltrer.

**Recommandation** : ne fusionner la clé que si `server_url` normalisé est identique à celui enregistré ; sinon exiger une nouvelle saisie.

### S6 — Clé API en clair sur disque — **moyenne**
`config.go:16`, `config.go:59`, `config.go:96`

`config.json` contient la clé en clair. Le mode `0o600` n'a pas d'effet d'ACL sous Windows (Go ne gère que l'attribut lecture seule) ; la protection repose uniquement sur les ACL héritées de `%APPDATA%`. Tout processus de la session, toute sauvegarde ou synchronisation du profil y a accès.

**Recommandation** : chiffrer la clé avec DPAPI (`windows.CryptProtectData`, déjà disponible dans `golang.org/x/sys/windows`) ou la placer dans le Gestionnaire d'identifiants Windows ; migrer automatiquement une clé en clair existante. Conserver la recommandation du README sur les permissions minimales de la clé.

### S7 — TLS : `insecure_skip_verify` global, HTTP accepté sans avertissement — **moyenne**
`immich.go:33-34`, `config.go:117`, `settings.html:64`

L'option « Accepter un certificat auto-signé » désactive toute vérification : n'importe quel intermédiaire réseau obtient la clé API (envoyée à chaque requête) et peut servir des images arbitraires au décodeur. `http://` est accepté sans distinction ni alerte.

**Recommandation** : remplacer l'option par un épinglage (empreinte SHA-256 du certificat relevée lors de « Tester », vérifiée via `VerifyPeerCertificate`) ou par un fichier d'autorité de certification ; afficher un avertissement dans la page pour `http://` hors réseau local.

### S8 — Identifiants venant du serveur utilisés comme noms de fichier — **moyenne (défense en profondeur)**
`core.go:298-300`, `immich.go:18-23`, `win_windows.go:194`

`Asset.ID` est repris tel quel du JSON du serveur et sert à construire `cache/<id>.jpg` et `cache/<id>.json` sans validation. Un serveur compromis (ou un intermédiaire, voir S7) qui renvoie `..\config` comme identifiant fait écraser `config.json` par les métadonnées du cache ; d'autres chemins relatifs permettent d'écrire des `.jpg`/`.json` hors du dossier. Le même identifiant est inséré dans le raccourci `.url` du bureau, et `server_url` n'est pas analysé par `url.Parse` (seul le préfixe est testé, `config.go:117`), ce qui laisse passer des retours à la ligne dans ce fichier INI. Point positif vérifié : `shellOpen` ne reçoit que des URL préfixées `http(s)://`.

**Recommandation** : n'accepter que les identifiants correspondant intégralement à un UUID (`uuidRe` ancré, `config.go:102`) dès `AlbumImages`/`searchAlbum` ; valider `server_url` avec `url.Parse` (hôte non vide, pas de caractères de contrôle, ni requête ni fragment).

### S9 — Serveur de réglages local : durcissements manquants — **faible**
`settings.go:31`, `settings.go:38`, `settings.go:66-71`, `settings.go:85,103`, `settings.go:125`

Vérifié et correct : écoute sur `127.0.0.1` uniquement, jeton aléatoire de 128 bits dans le chemin, clé jamais renvoyée, noms d'albums injectés via `textContent` (`settings.html:200`, pas de XSS), aucune ressource externe chargée. Restent : l'erreur de `rand.Read` ignorée ; contrôle d'`Origin` seulement pour POST et seulement si l'en-tête est présent ; pas de contrôle de `Host` ; `api/next` accepte GET ; corps de requête non borné ; `http.Serve` sans délais ; pas d'en-têtes `Cache-Control: no-store`, `Referrer-Policy`, `Content-Security-Policy`, `X-Frame-Options` ; jeton stable pendant toute la vie du processus et conservé dans l'historique du navigateur.

**Recommandation** : vérifier `Host == s.addr`, exiger POST + `Content-Type: application/json` sur les routes d'action, `http.MaxBytesReader`, `http.Server` avec délais, en-têtes ci-dessus ; idéalement échanger le jeton d'URL contre un cookie `HttpOnly; SameSite=Strict` puis rediriger vers une URL sans jeton.

### S10 — Programmes système lancés par nom, arrêt de processus par motif — **faible**
`win_windows.go:160`, `install_windows.go:82`, `install_windows.go:175`, `install_windows.go:249-254`

`powershell.exe`, `taskkill` et `cmd.exe` sont résolus via `PATH`. `taskkill /F /IM ImmichWallpaper*` termine tout processus dont le nom commence ainsi. La ligne `rd /s /q "%s"` est construite à partir de `LOCALAPPDATA`. Rien d'exploitable sans disposer déjà de la session utilisateur. Point positif vérifié : les paramètres sont transmis à PowerShell par variables d'environnement et non par interpolation (`install_windows.go:174-176`, `win_windows.go:151,161`).

**Recommandation** : chemins absolus sous `%SystemRoot%\System32` ; remplacer `taskkill` par la terminaison via le fichier PID déjà en place.

---

## 3. Design / architecture

### D1 — Le style choisi n'est pas respecté dès qu'une légende est rendue — **moyenne**
`core.go:538-540`, `render.go:33`, `win_windows.go:130-132`, `settings.html:87-94`

Quand la légende est active (défaut : `show_place`, `config.go:46`) ou en mode deux panneaux, `applySingle` force `style = "fill"` et `compose` ramène `center`, `tile` et `span` à `fill`. Cas le plus visible : avec « Étendre (multi-écrans) », `screenFor("span")` renvoie bien la taille du bureau virtuel, l'image est rendue à cette taille, puis appliquée en « Remplir » : chaque écran affiche le centre rogné de l'image au lieu d'une image à cheval sur les écrans. La page propose pourtant ces six styles sans avertissement.

**Recommandation** : conserver `span` lors de l'application si `cfg.Style == "span"` ; implémenter `center`/`tile` dans `compose` ou les désactiver dans la page quand une légende est demandée. Ajouter un test par style.

### D2 — Un événement d'affichage peut déclencher un tirage malgré la pause — **moyenne**
`core.go:419-422`, `main_windows.go:342-347`, `main_windows.go:353-361`

Si `lastPhotos` contient moins de photos que nécessaire (démarrage avec `change_on_start=false`, erreur précédente, écran ajouté), `Reapply` appelle `Next()` : nouveau tirage et nouveau fond d'écran, même si l'utilisateur a mis l'appli en pause, et à chaque sortie de veille tant qu'aucune photo n'est mémorisée. Ce chemin contourne en outre `App.change` : ni le statut, ni `a.next`, ni l'élément « Voir la photo » ne sont mis à jour (`reapply` ignore le `Result`).

**Recommandation** : `Reapply` renvoie un indicateur « tirage nécessaire » ; c'est `App` qui décide (respect de la pause) et qui passe par `change()`.

### D3 — Remplacement de l'exécutable non atomique — **moyenne**
`update_windows.go:24-30`

`replaceSelf` renomme l'exe courant en `.old` puis écrit le nouveau directement à l'emplacement final. Une coupure pendant l'écriture laisse un exécutable tronqué à l'emplacement lancé au démarrage de Windows, sans mécanisme de reprise (le `.old` n'est restauré que par le processus en cours).

**Recommandation** : écrire dans `ImmichWallpaper.exe.new`, `Sync`, puis deux renommages (`self` → `.old`, `.new` → `self`) ; au démarrage, restaurer `.old` si l'exe principal est absent ou invalide.

### D4 — Paquet unique, état et responsabilités mêlés — **moyenne**
`main_windows.go:28-43`, `core.go:59-83`, `settings.go:59-145`

Tout est dans `package main`. `App` cumule interface, planification, mise à jour et état ; `Core` cumule synchronisation des albums, sélection, cache, rendu et application. Le découplage de la plateforme par champs de fonctions (`core.go:77-82`) est une bonne base et rend `Core` testable sous Linux. En revanche l'horloge n'est pas injectable (planification non testée), le routage HTTP est un `switch` unique, et les textes français sont codés en dur partout (par ex. `caption.go:43`).

**Recommandation** : extraire progressivement des paquets internes (`immich`, `imaging`, `cache`, `scheduler`, `platform/windows`) ; injecter une horloge ; isoler les textes pour préparer une traduction.

### D5 — Couverture de tests : rien sur la partie Windows ni sur les analyseurs binaires — **moyenne**
`.github/workflows/build.yml:33-38`, `app_test.go`, `core_test.go`, `update_test.go`

Les 14 tests couvrent bien `Core`, le serveur de réglages et la logique de mise à jour grâce à un faux serveur Immich. Mais la CI ne fait que `go vet` pour Windows ; les tests tournent sous Linux, donc `main_windows.go`, `install_windows.go`, `update_windows.go`, `monitors_windows.go` et `events_windows.go` (appels COM par indice de vtable, `monitors_windows.go:25-32`) ne sont jamais exécutés. Aucun test avec entrées malformées pour `colorprofile.go`. Ni `-race`, ni `staticcheck`, ni `govulncheck`.

**Recommandation** : job `windows-latest` exécutant `go test -race ./...` ; tests de fuzzing (voir S4) ; `staticcheck` et `govulncheck` en CI ; tests de la planification une fois l'horloge injectée.

### D6 — Erreurs ignorées en silence — **faible**
`main_windows.go:65`, `main_windows.go:222`, `core.go:292-301`, `win_windows.go:38-39`, `install_windows.go:161-170`, `install_windows.go:177`, `monitors_windows.go:94`, `monitors_windows.go:149`, `immich.go:46`

Création du dossier de données et de la config par défaut, écritures dans le registre, création du raccourci du menu Démarrer, échecs du cache hors ligne, `SetPosition` COM, énumération des écrans : aucune de ces erreurs n'est journalisée. En cas d'échec, l'utilisateur voit un comportement dégradé sans trace dans `journal.txt`.

**Recommandation** : au minimum `log.Printf` sur chacune ; remonter au statut celles qui dégradent une fonction choisie par l'utilisateur (cache, multi-écrans).

### D7 — Journal : suppression brutale, pas de rotation à chaud — **faible**
`main_windows.go:70-75`

Au-delà de 1 Mo, le journal est supprimé au démarrage (l'historique disparaît au moment où l'on en a besoin) et n'est jamais borné pendant l'exécution, alors que l'appli tourne des semaines. `io.MultiWriter(f)` avec un seul flux est sans effet.

**Recommandation** : rotation `journal.txt` → `journal.1.txt`, vérifiée aussi périodiquement.

### D8 — Planification : nouvelle tentative fixe, 403 traité comme erreur globale — **faible**
`main_windows.go:164-171`, `immich.go:61-64`, `core.go:233-235`

Hors cache, un serveur injoignable est réinterrogé toutes les 2 min indéfiniment, sans temporisation croissante. Un 403 sur **un** asset est assimilé à un refus global de la clé et interrompt tout le tirage.

**Recommandation** : temporisation exponentielle plafonnée (2 → 30 min) ; distinguer 401 (clé invalide) de 403 sur une ressource (passer à l'asset suivant).

### D9 — Documentation en décalage avec le code — **faible**
`README.md:7`, `README.md:69`, `README.md:73`, `core.go:28`

Paragraphe d'introduction dupliqué (ligne 7) ; « liste des albums rafraîchie toutes les 6 h » alors que le code vérifie toutes les 30 min ; « Même image sur tous les écrans » contredit la section multi-écrans ; la liste du menu omet « Rechercher une mise à jour » et « Désinstaller… ».

**Recommandation** : corriger ces quatre points.

### D10 — Divers lisibilité / dette — **faible**
- `core.go:209` : variable locale `max` qui masque la fonction native `max` utilisée sur la même ligne.
- `core.go:240-245` : `prepareImage` puis `decodeNormalized` analysent deux fois les métadonnées JPEG ; `prepareImage` renvoie `data` que personne n'utilise.
- `caption.go:38-39` : chemin `C:\Windows\Fonts` codé en dur (utiliser `%WINDIR%` ou le dossier connu des polices).
- `settings.html:213-230` et `config.go:36-55` : valeurs par défaut dupliquées entre la page et Go.
- `core.go:66` : l'historique `recent` n'est pas persistant ; après un redémarrage, les mêmes photos peuvent revenir.
- `rsrc_windows_*.syso` : binaires générés et versionnés ; les produire en CI garantirait la cohérence avec `app.manifest` et permettrait d'y inscrire la version (**hypothèse** : l'exe n'a pas de ressource VERSIONINFO, la commande documentée dans le README n'en génère pas).
- `go.mod:3` : `go 1.22` alors que la CI compile avec `stable` ; aligner les deux.

---

## 4. Idées de fonctionnalités

Classées par rapport valeur/effort estimé.

1. **Sources au-delà des albums** : favoris, personnes, tags, « ce jour-là » (souvenirs), recherche par texte (« montagne », « plage »). Le client sait déjà interroger `/api/search/metadata` (`immich.go:108-127`) ; il suffit d'élargir les filtres (**hypothèse** sur les filtres disponibles selon la version d'Immich).
2. **Actions sur la photo affichée depuis l'icône** : « Photo précédente », « Ne plus afficher cette photo » (liste d'exclusion locale), « Ajouter aux favoris Immich ». L'état nécessaire existe déjà (`lastPhotos`, `recent`).
3. **Portraits intelligents** : au lieu d'écarter les portraits, en composer deux côte à côte sur un écran 16:9 (le rendu en panneaux existe, `core.go:582-609`) ou les afficher sur fond flouté. Règle P1 au passage.
4. **Recadrage tenant compte des visages** : utiliser les zones de visages connues d'Immich pour décaler le recadrage « Remplir » (`render.go:33-37`) et éviter de couper les têtes (**hypothèse** : zones exposées par l'API des assets).
5. **Légende personnalisable** : nom de l'album, description, personnes, date complète, modèle de texte ; choix de la langue (les mois sont en français en dur, `caption.go:43`).
6. **Historique persistant et rotation équitable** : sauvegarder `recent` sur disque, mode « parcourir tout l'album avant de répéter », pondération par album.
7. **Planification contextuelle** : plages horaires, albums selon l'heure ou la saison, pause sur batterie ou connexion limitée, pas de changement pendant une application en plein écran.
8. **Sécurité visible par l'utilisateur** : clé protégée par DPAPI, épinglage du certificat à la place de « accepter un certificat auto-signé », mises à jour signées avec affichage des notes de version et mode « me prévenir seulement ».
9. **Préchargement** : télécharger et préparer la photo suivante en avance pour un changement instantané au déverrouillage, sans dépendre du réseau à cet instant.
10. **Distribution** : manifeste winget/scoop, ressource de version dans l'exécutable, signature Authenticode (supprime l'avertissement SmartScreen mentionné dans le README).
11. **Diagnostic** : bouton « Copier le diagnostic » (version, configuration sans la clé, dernières lignes du journal) dans la page de réglages.

---

## Annexe — points vérifiés et jugés corrects

- Aucun secret dans le dépôt ni dans l'historique git (recherche de motifs de clés/jetons sur tous les commits).
- `go.sum` versionné et `go mod verify` exécuté en CI (`build.yml:28-31`).
- Clé API jamais renvoyée à la page (`settings.go:81`) ; test dédié (`app_test.go`, `TestSettingsServer`).
- Écriture atomique de la configuration (fichier temporaire + renommage, `config.go:90-100`).
- Pas d'injection de commande PowerShell (paramètres par variables d'environnement).
- Mise à jour : refus des versions antérieures ou égales, des brouillons et préversions (`update.go:115`), retour arrière automatique (`update_windows.go:84-113`).
- Instance unique par mutex nommé `Local\` (`win_windows.go:99-110`), vérification du nom d'image avant de terminer un PID (`install_windows.go:102-104`).
- Analyseurs `jpegMeta`, `exifOrientation`, `webpICC`, `parseICC` : contrôles de bornes présents sur tous les accès relus (cibles 64 bits) ; seul le cas `NaN` de S4 a été identifié.

---

## Suivi — correctifs appliqués

État après application des recommandations (tests : `go test ./...` sous Linux et Windows ARM64, `go vet` Windows x64/ARM64, `staticcheck`, `govulncheck`, fuzzing court ; `go test -race` n'a pas pu être lancé en local — pas de compilateur C — et tourne en CI).

| Constat | État | Détail |
|---|---|---|
| P1 | Appliqué | Portraits écartés ni décodés, ni enrichis, ni mis en cache ; au plus *n* gardés compressés en secours. Hors ligne, les paysages du cache passent d'abord. Le filtrage avant téléchargement (hypothèse `exifInfo`) n'est pas fait. |
| P2 | Appliqué | Traitement écran par écran ; seules les versions JPEG réduites sont conservées entre deux écrans. |
| P3 | Appliqué | Une seule réduction/encodage par photo (cache + photos mémorisées), en `BiLinear` ; les photos du cache ne sont plus réencodées. |
| P4 | Appliqué | Verrou d'état court, distinct du verrou des opérations longues ; réseau, rendu et PowerShell hors verrou. |
| P5 | Appliqué | `orient()` par accès direct aux octets ; `toSRGB` par bandes parallèles. |
| P6 | Partiel | Album inaccessible retiré du tirage et journalisé. `withoutAssets` non appliqué (hypothèse non vérifiée, compatibilité avec les anciennes versions d'Immich). |
| P7 | Appliqué | Minuteur réarmé sur l'échéance (plafonné à 1 min pour rester correct après une veille) ; requêtes avec `context`, annulées au changement de configuration ; délais distincts (en-têtes 2 min, téléchargement 15 min). |
| S1 | Partiel | Adresse des fichiers et redirections contrôlées (releases GitHub du dépôt uniquement). **Signature des releases non faite** : elle demande une clé privée détenue hors du dépôt. Le défaut « installer » est inchangé. |
| S2 | Appliqué | Jobs séparés (`contents: read` partout sauf `release`), actions épinglées par SHA, Go fixé par `go.mod`, plus de `--clobber`, attestation de provenance. L'approbation manuelle (*environment*) reste à activer dans les réglages du dépôt. |
| S3 | Appliqué | `x/image` 0.48, `x/sys` 0.50, `x/text` 0.44 ; `govulncheck` en CI ; Dependabot (`gomod`, `github-actions`). |
| S4 | Appliqué | `recover` autour du décodage, de `Next`/`Reapply` et des goroutines de l'appli ; plafond de 120 Mpx ; `NaN` traité ; tests de fuzzing. |
| S5 | Appliqué | La clé enregistrée n'est réutilisée que pour le serveur enregistré. |
| S6 | Appliqué | Clé chiffrée par DPAPI (`api_key_protected`), migration automatique d'une clé en clair. |
| S7 | Appliqué | Certificat épinglé (empreinte SHA-256 relevée au test) à la place de l'acceptation aveugle ; avertissement `http://` hors réseau local. |
| S8 | Appliqué | Identifiants d'assets limités aux UUID ; `server_url` validé par `url.Parse`. |
| S9 | Partiel | Contrôle de `Host`, POST + JSON exigés, corps borné, délais, en-têtes de sécurité, erreur de `rand.Read` traitée. L'échange du jeton d'URL contre un cookie n'est pas fait. |
| S10 | Appliqué | Chemins absolus `System32` ; `taskkill` limité aux trois noms d'exécutable distribués (conservé pour les anciennes versions sans fichier PID). |
| D1 | Appliqué | « Étendre » conservé à l'application ; « Centrer » et « Mosaïque » rendus par `compose` ; un test par style. |
| D2 | Appliqué | `Reapply` renvoie `errNeedDraw` ; l'appli décide (pause respectée) et passe par `change()`. |
| D3 | Partiel | Écriture dans `.new` + `Sync` + renommages. Pas de restauration au démarrage (si l'exe est absent, rien ne se lance). |
| D4 | Non fait | Découpage en paquets et horloge injectable : refonte à part. |
| D5 | Appliqué | CI : tests sous Windows, `-race`, `staticcheck`, `govulncheck`, fuzzing. Tests de planification non faits (dépend de D4). |
| D6 | Appliqué | Erreurs journalisées (dossier de données, registre, raccourcis, cache, COM). |
| D7 | Appliqué | Rotation `journal.txt` → `journal.1.txt`, vérifiée à chaque écriture. |
| D8 | Appliqué | Temporisation 2 → 30 min ; 401 (clé) distingué de 403 (ressource ignorée). |
| D9 | Appliqué | README corrigé. |
| D10 | Partiel | `max` masqué, double lecture des métadonnées, chemin des polices, historique `recent` persistant, `go.mod` aligné. Non faits : valeurs par défaut dupliquées dans la page, génération des `.syso` en CI. |

### Idées de fonctionnalités (section 4)

| Idée | État | Détail |
|---|---|---|
| 2 — Actions depuis l'icône | Fait | « Photo précédente » (5 retours en arrière, en mémoire), « Ajouter aux favoris Immich » (permission `asset.update`), « Ne plus afficher cette photo » (liste locale `excluded.json`, réversible depuis la page de réglages). |
| 6 — Historique persistant | Partiel | Photos récentes conservées d'un lancement à l'autre (`recent.json`). |
| 7 — Planification contextuelle | Fait | Plage horaire, albums selon la saison ou l'heure, pause sur batterie, connexion limitée ou plein écran. |
| 8 — Sécurité visible | Partiel | Clé protégée par DPAPI et certificat épinglé ; mises à jour signées non faites. |
| 9 — Préchargement | Fait | Photo suivante (et sa légende) téléchargée à l'avance sur disque ; le changement suivant ne fait aucune requête. |
| 1, 3, 4, 5, 10, 11 | Non fait | — |
