# Immich Wallpaper — réécriture en Rust (à l'essai)

Même appli que la version Go à la racine du dépôt, réécrite en Rust. **Elle n'est pas publiée** :
les releases et la mise à jour automatique restent celles de la version Go tant que cette
version n'a pas été validée à la main.

## Ce qui est identique

- Mêmes fichiers de données (`config.json`, cache hors ligne, `recent.json`, `excluded.json`) :
  une installation existante est reprise telle quelle, clé API chiffrée (DPAPI) comprise.
- Même page de réglages (`../settings.html`), ouverte à la demande et refermée après 10 min
  d'inactivité ; même icône et même manifeste (`../rsrc_windows_*.syso`).
- Mêmes noms d'exécutables et même protocole de mise à jour (`--after-update`, retour arrière).
- Mêmes variantes : classique, installeur (`IW_FLAVOR=setup`), portable (`IW_FLAVOR=portable`,
  ou exe classique avec `portable.txt` à côté, comme dans le zip portable). En portable, les
  mises à jour sont annoncées dans le menu et installées sur clic seulement, et l'icône du
  bureau est décochée par défaut.

## Ce qui change

- L'icône de la barre système, le menu et les événements (déverrouillage, écrans, veille)
  passent par une seule fenêtre cachée, sans bibliothèque tierce.
- L'écran de verrouillage et le raccourci du menu Démarrer sont faits par appels directs à
  Windows, sans PowerShell.
- TLS par Schannel (magasin de certificats de Windows).
- Les images sont manipulées en RVB (3 octets par pixel au lieu de 4).
- Une requête en cours n'est plus annulée quand la configuration change (elle se termine,
  son résultat est ignoré).
- La légende utilise les polices de Windows (Segoe UI, sinon Arial) ; il n'y a plus de police
  de secours embarquée.

## Construire

Depuis Linux, avec [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) dans le `PATH` :

```sh
rustup target add x86_64-pc-windows-gnullvm aarch64-pc-windows-gnullvm
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
export CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER=aarch64-w64-mingw32-clang
cargo build --release --target x86_64-pc-windows-gnullvm     # exe classique
IW_FLAVOR=portable cargo build --release --target aarch64-pc-windows-gnullvm
```

`IW_VERSION` fixe la version affichée (sinon celle de `Cargo.toml`). Sous Windows,
`cargo build --release` suffit pour compiler ; l'intégration de l'icône et du manifeste n'y a
été vérifiée qu'avec les cibles `gnullvm` ci-dessus.

## Tester

```sh
cargo test                                             # partie indépendante de Windows
cargo test --no-run --target aarch64-pc-windows-gnullvm   # puis lancer l'exe de test sous Windows
```

Sous Windows, deux tests supplémentaires sont activés par des variables d'environnement :

- `IW_APP_EXE=<chemin de ImmichWallpaper.exe>` : parcours complet de la vraie appli face à un
  faux serveur Immich (tirage, image suivante, photo précédente, changement d'écran, pause,
  favori, exclusion, préchargement).
- `IW_NET_TESTS=1` : TLS et épinglage de certificat face à un vrai serveur, lecture de la
  dernière release.

Ce parcours utilise le mode de vérification de l'appli, utilisable aussi à la main :

```
ImmichWallpaper.exe --selftest resultat.txt [--selftest-dir dossier]
```

Dans ce mode, rien n'est appliqué au bureau : les images sont rendues dans le dossier de test,
le fond d'écran, l'écran de verrouillage, le démarrage automatique et la configuration réelle
ne sont pas touchés.

| Fichier | Rôle |
|---|---|
| `src/core.rs` | albums, choix des photos, cache hors ligne, préchargement, rendu par écran |
| `src/immich.rs` | client API Immich, TLS et épinglage |
| `src/colorprofile.rs` | lecture des images, profil ICC → sRGB, orientation EXIF |
| `src/render.rs`, `src/caption.rs` | recadrage, carte lieu/date |
| `src/config.rs` | configuration, plage horaire, règles d'albums |
| `src/settings.rs` | page de réglages locale |
| `src/update.rs` | recherche et téléchargement d'une mise à jour |
| `src/win/app.rs` | icône, menu, planification, événements système |
| `src/win/platform.rs` | fond d'écran, registre, DPAPI, écran de verrouillage |
| `src/win/monitors.rs`, `src/win/context.rs` | une image par écran ; batterie, plein écran, connexion |
| `src/win/install.rs`, `src/win/updater.rs` | installation, désinstallation, mise à jour |
