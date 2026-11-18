//! Tests du cœur, face à un faux serveur Immich (repris des versions précédentes, écrites en Go).

use crate::config::{AlbumRule, Config};
use crate::core::{Core, Hooks, Monitor, HISTORY_DEPTH};
use crate::immich::Client;
use crate::render::{jpeg_dims, Screen};
use crate::testutil::*;
use crate::util::{Error, Local};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Ce que la « plateforme » de test a reçu, et ce qu'elle répond.
#[derive(Default)]
struct Rec {
    applied: Mutex<Vec<(PathBuf, String)>>,
    multi: Mutex<Vec<(String, PathBuf)>>,
    lock: Mutex<Option<PathBuf>>,
    screen: Mutex<Screen>,
    monitors: Mutex<Vec<Monitor>>,
    now: Mutex<Local>,
    panic: AtomicBool,
}

impl Rec {
    fn last_applied(&self) -> (PathBuf, String) {
        self.applied.lock().unwrap().last().cloned().expect("aucun fond d'écran appliqué")
    }
    fn set_screen(&self, w: i32, h: i32, inset: i32) {
        *self.screen.lock().unwrap() = Screen { w, h, bottom_inset: inset };
    }
}

fn dims(path: &Path) -> (u32, u32) {
    jpeg_dims(&std::fs::read(path).unwrap()).expect("fichier écrit non JPEG")
}

/// Cœur de test. `screen` : la plateforme connaît la taille de l'écran ; `multi` : elle sait
/// appliquer une image par écran ; `lock` : elle gère l'écran de verrouillage.
fn core_in(dir: &Path, rec: &Arc<Rec>, screen: bool, multi: bool, lock: bool) -> Core {
    let r = rec.clone();
    let mut h = Hooks::new(move |p, s| {
        if r.panic.load(Ordering::SeqCst) {
            panic!("boum");
        }
        r.applied.lock().unwrap().push((p.to_path_buf(), s.to_string()));
        Ok(())
    });
    if screen {
        let r = rec.clone();
        h.screen = Some(Box::new(move |_| *r.screen.lock().unwrap()));
    }
    if multi {
        let r = rec.clone();
        h.monitors = Some(Box::new(move |_| r.monitors.lock().unwrap().clone()));
        let r = rec.clone();
        h.set_monitor_wallpapers = Some(Box::new(move |paths, _| {
            *r.multi.lock().unwrap() = paths.to_vec();
            Ok(())
        }));
    }
    if lock {
        let r = rec.clone();
        h.set_lock_screen = Some(Box::new(move |p| {
            *r.lock.lock().unwrap() = Some(p.to_path_buf());
            Ok(())
        }));
    }
    let r = rec.clone();
    h.now = Box::new(move |_| *r.now.lock().unwrap());
    Core::new(dir, h)
}

fn new_core(rec: &Arc<Rec>, screen: bool, multi: bool, lock: bool) -> Core {
    core_in(&temp_dir("core"), rec, screen, multi, lock)
}

fn test_cfg(url: &str, albums: &[&str]) -> Config {
    let mut c = Config {
        server_url: url.into(),
        api_key: KEY.into(),
        albums: albums.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    c.show_place = false;
    c.normalize().unwrap();
    c
}

#[test]
fn core_next() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    let mut cfg = Config {
        server_url: format!("{}/api/", fake.url),
        api_key: KEY.into(),
        albums: vec![ALB1.into(), ALB2.into()],
        ..Default::default()
    };
    cfg.show_place = false;
    cfg.offline_cache = 0;
    cfg.normalize().unwrap();
    core.set_config(cfg.clone());
    let mut seen = std::collections::HashSet::new();
    for _ in 0..6 {
        let r = core.next().unwrap();
        assert_ne!(r.asset.id, P1, "portrait choisi alors que landscape_only");
        seen.insert(r.asset.id);
    }
    assert!(seen.contains(L1) && seen.contains(L2), "{seen:?}");
    assert_eq!(core.assets_count(), 3);
    dims(&rec.last_applied().0);
    assert!(core.last().unwrap().1.ends_with(&format!("/photos/{}", core.last().unwrap().0.id)));
    // Mauvais token.
    cfg.api_key = "nope".into();
    core.set_config(cfg);
    assert_eq!(core.next().unwrap_err(), Error::Unauthorized);
}

#[test]
fn portrait_fallback() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    core.set_config(test_cfg(&fake.url, &[ALB1]));
    assert_eq!(core.next().unwrap().asset.id, P1);
}

#[test]
fn caption_pipeline() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    rec.set_screen(1920, 1080, 48);
    let core = new_core(&rec, true, false, true);
    let mut cfg = test_cfg(&fake.url, &[ALB2]);
    cfg.style = "fit".into();
    cfg.show_place = true;
    cfg.show_date = true;
    cfg.lock_screen = true;
    core.set_config(cfg);
    core.next().unwrap();
    let (path, style) = rec.last_applied();
    assert_eq!(dims(&path), (1920, 1080));
    assert_eq!(style, "fill");
    assert_eq!(rec.lock.lock().unwrap().as_deref(), Some(path.as_path()));
    assert!(core.lock_error().is_none());
    // Les informations de la photo (lieu, date) ont été lues malgré les champs `null`.
    assert_eq!(core.last_photos()[0].info.exif.city, "Rovaniemi");
}

#[test]
fn incremental_albums() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    core.set_config(test_cfg(&fake.url, &[ALB1, ALB2]));
    core.next().unwrap();
    let (a1, a2) = (format!("/api/albums/{ALB1}"), format!("/api/albums/{ALB2}"));
    assert_eq!((fake.fetches(&a1), fake.fetches(&a2)), (1, 1), "chargement initial");
    // Vérification suivante sans changement : aucun album rechargé.
    core.force_album_check();
    core.next().unwrap();
    assert_eq!((fake.fetches(&a1), fake.fetches(&a2)), (1, 1), "album rechargé alors qu'inchangé");
    // Album 2 modifié : lui seul est rechargé.
    fake.state.count2.store(3, Ordering::SeqCst);
    core.force_album_check();
    core.next().unwrap();
    assert_eq!((fake.fetches(&a1), fake.fetches(&a2)), (1, 2), "rechargement incrémental incorrect");
}

#[test]
fn offline_cache() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB2]);
    cfg.offline_cache = 1;
    core.set_config(cfg);
    for _ in 0..3 {
        core.next().unwrap();
    }
    assert_eq!(core.cached_count(), 1, "le cache doit être limité à 1");
    fake.set_down(true);
    core.force_album_check();
    let before = rec.applied.lock().unwrap().len();
    let r = core.next().unwrap();
    assert!(r.offline && r.cause.is_some(), "bascule hors ligne attendue : {r:?}");
    assert_eq!(rec.applied.lock().unwrap().len(), before + 1);
    // Cache vide : l'erreur remonte.
    std::fs::remove_dir_all(core.cache_dir()).unwrap();
    assert!(core.next().is_err(), "erreur attendue sans cache");
}

#[test]
fn multi_monitor() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    *rec.monitors.lock().unwrap() = vec![
        Monitor {
            id: "ecranA".into(),
            screen: Screen {
                w: 1920,
                h: 1080,
                bottom_inset: 48,
            },
        },
        Monitor {
            id: "ecranB".into(),
            screen: Screen {
                w: 2560,
                h: 1440,
                bottom_inset: 0,
            },
        },
    ];
    let core = new_core(&rec, false, true, true);
    let mut cfg = test_cfg(&fake.url, &[ALB2]);
    cfg.multi_monitor = "different".into();
    cfg.lock_screen = true;
    core.set_config(cfg);
    core.next().unwrap();
    assert!(rec.applied.lock().unwrap().is_empty(), "mode écran unique utilisé");
    let got = rec.multi.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert_eq!(rec.lock.lock().unwrap().as_ref(), Some(&got[0].1));
    for ((id, path), want) in got.iter().zip([("ecranA", (1920, 1080)), ("ecranB", (2560, 1440))]) {
        assert_eq!(id, want.0);
        assert_eq!(dims(path), want.1, "{id}");
    }
    let recent = core.recent();
    assert!(
        recent.len() == 2 && recent[0] != recent[1],
        "deux photos différentes attendues : {recent:?}"
    );
}

#[test]
fn split_wide() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    rec.set_screen(5120, 1440, 48);
    let core = new_core(&rec, true, true, false);
    let mut cfg = test_cfg(&fake.url, &[ALB1, ALB2]);
    cfg.landscape_only = false;
    core.set_config(cfg.clone());
    core.next().unwrap();
    let (path, style) = rec.last_applied();
    assert_eq!(dims(&path), (5120, 1440));
    assert_eq!(style, "fill");
    let recent = core.recent();
    assert!(
        recent.len() == 2 && recent[0] != recent[1],
        "deux photos différentes attendues sur l'écran 32:9 : {recent:?}"
    );
    // 21:9 : pas de découpe.
    rec.set_screen(3440, 1440, 0);
    core.clear_recent();
    core.next().unwrap();
    assert_eq!(core.recent().len(), 1, "un 21:9 ne doit pas être découpé");
    // Multi-écrans : 16:9 + 32:9 → 3 photos.
    *rec.monitors.lock().unwrap() = vec![
        Monitor {
            id: "a".into(),
            screen: Screen {
                w: 1920,
                h: 1080,
                bottom_inset: 0,
            },
        },
        Monitor {
            id: "b".into(),
            screen: Screen {
                w: 5120,
                h: 1440,
                bottom_inset: 0,
            },
        },
    ];
    cfg.multi_monitor = "different".into();
    core.set_config(cfg);
    core.clear_recent();
    core.next().unwrap();
    assert_eq!(core.recent().len(), 3, "3 photos attendues (1 + 2)");
    assert_eq!(dims(&rec.multi.lock().unwrap()[1].1), (5120, 1440));
}

#[test]
fn reapply() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    rec.set_screen(1920, 1080, 0);
    let core = new_core(&rec, true, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB1, ALB2]);
    cfg.landscape_only = false;
    cfg.show_date = true; // rendu à la taille de l'écran (sinon Windows ajuste lui-même)
    core.set_config(cfg);
    core.next().unwrap();
    let first = core.last_photos()[0].asset.id.clone();
    let downloads = || fake.count("/api/assets", "thumbnail");
    let before = downloads();

    // Nouvelle résolution : même photo, recalculée, aucun téléchargement.
    rec.set_screen(2560, 1440, 0);
    core.reapply().unwrap();
    assert_eq!(dims(&rec.last_applied().0).0, 2560);
    assert_eq!(core.last_photos()[0].asset.id, first);
    assert_eq!(downloads(), before);

    // Écran 32:9 branché : il faut 2 photos → Reapply le signale sans tirer lui-même
    // (c'est l'appli qui décide, pour respecter la pause).
    rec.set_screen(5120, 1440, 0);
    assert_eq!(core.reapply().unwrap_err(), Error::NeedDraw);
    assert_eq!(downloads(), before);
    core.next().unwrap();
    assert!(core.last_photos().len() == 2 && downloads() > before, "nouveau tirage attendu");
}

/// Les portraits écartés (landscape_only) ne doivent ni entrer dans le cache hors ligne,
/// ni coûter un appel d'informations.
#[test]
fn portraits_not_cached() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB1, ALB2]);
    cfg.show_place = true;
    core.set_config(cfg);
    for _ in 0..6 {
        core.next().unwrap();
    }
    assert!(
        !core.cache_dir().join(format!("{P1}.jpg")).exists(),
        "portrait écarté présent dans le cache hors ligne"
    );
    assert_eq!(
        fake.fetches(&format!("/api/assets/{P1}")),
        0,
        "informations demandées pour un portrait écarté"
    );
    // Identifiant non UUID renvoyé par le serveur (« ..\config ») : jamais utilisé.
    assert_eq!(core.assets_count(), 3);
    assert!(!core.cache_dir().parent().unwrap().join("config.jpg").exists());
}

/// Hors ligne, les paysages du cache passent avant les portraits.
#[test]
fn offline_prefers_landscape() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    core.set_config(test_cfg(&fake.url, &[ALB1])); // un seul portrait : affiché en secours, donc mis en cache
    assert_eq!(core.next().unwrap().asset.id, P1);
    core.set_config(test_cfg(&fake.url, &[ALB1, ALB2]));
    for _ in 0..4 {
        core.next().unwrap();
    }
    fake.set_down(true);
    core.force_album_check();
    for _ in 0..5 {
        let r = core.next().unwrap();
        assert!(r.offline);
        assert_ne!(r.asset.id, P1, "portrait tiré du cache alors que des paysages y sont disponibles");
    }
}

/// « Étendre » reste appliqué en « Étendre » quand l'appli rend elle-même l'image.
#[test]
fn span_is_kept() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    rec.set_screen(3840, 1080, 0); // bureau virtuel de deux écrans
    let core = new_core(&rec, true, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB2]);
    cfg.style = "span".into();
    cfg.show_date = true;
    cfg.split_wide = false;
    core.set_config(cfg);
    core.next().unwrap();
    let (path, style) = rec.last_applied();
    assert_eq!(style, "span");
    assert_eq!(dims(&path), (3840, 1080));
}

#[test]
fn panic_becomes_error() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    core.set_config(test_cfg(&fake.url, &[ALB2]));
    rec.panic.store(true, Ordering::SeqCst);
    let e = core.next().unwrap_err();
    assert!(e.to_string().contains("boum"), "panic non converti en erreur : {e}");
    // Le verrou est bien relâché : l'appel suivant ne bloque pas.
    rec.panic.store(false, Ordering::SeqCst);
    core.next().unwrap();
}

fn at(month: u32) -> Local {
    Local {
        year: 2026,
        month,
        day: 15,
        hour: 12,
        minute: 0,
        second: 0,
    }
}

/// Les albums d'une règle remplacent les albums habituels pendant sa saison.
#[test]
fn album_rules_draw() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    *rec.now.lock().unwrap() = at(7);
    let core = new_core(&rec, false, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB1]); // habituel : un seul portrait (P1)
    cfg.album_rules = vec![AlbumRule {
        albums: vec![ALB2.into()],
        season: "summer".into(),
        ..Default::default()
    }];
    core.set_config(cfg);
    for _ in 0..4 {
        assert_ne!(core.next().unwrap().asset.id, P1, "en été, seul l'album de la règle doit servir");
    }
    assert_eq!(core.assets_count(), 3, "tous les albums référencés doivent être chargés");
    *rec.now.lock().unwrap() = at(1);
    assert_eq!(core.next().unwrap().asset.id, P1, "hors saison, retour aux albums habituels");
}

/// Après un préchargement, le changement suivant n'a besoin d'aucune requête.
#[test]
fn preload() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    let mut cfg = test_cfg(&fake.url, &[ALB2]);
    cfg.show_place = true;
    core.set_config(cfg.clone());
    let first = core.next().unwrap();
    core.preload().unwrap();
    let pre = core.preloaded_ids();
    assert_eq!(pre.len(), 1);
    assert_ne!(pre[0], first.asset.id, "la photo préchargée doit différer de la photo affichée");

    fake.set_down(true); // plus de réseau
    let total = fake.total();
    let r = core.next().unwrap();
    assert!(
        !r.offline && r.asset.id == pre[0],
        "photo préchargée attendue ({}), obtenu {r:?}",
        pre[0]
    );
    assert_eq!(fake.total(), total, "requêtes pendant un changement préchargé");
    assert!(core.preloaded_ids().is_empty());
    assert_eq!(
        std::fs::read_dir(core.preload_dir()).unwrap().count(),
        0,
        "fichiers de préchargement non nettoyés"
    );
    // La légende a été préparée à l'avance elle aussi.
    assert_eq!(core.last_photos()[0].info.exif.city, "Rovaniemi");

    // Un changement de configuration périme le préchargement ; l'option peut être désactivée.
    fake.set_down(false);
    core.preload().unwrap();
    assert_eq!(core.preloaded_ids().len(), 1);
    cfg.preload = false;
    core.set_config(cfg);
    assert!(
        core.preloaded_ids().is_empty(),
        "préchargement conservé après un changement de configuration"
    );
    let before = fake.count("/api/assets", "");
    core.preload().unwrap();
    assert!(
        core.preloaded_ids().is_empty() && fake.count("/api/assets", "") == before,
        "préchargement effectué alors que l'option est désactivée"
    );
}

#[test]
fn previous() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    core.set_config(test_cfg(&fake.url, &[ALB2]));
    assert_eq!(core.previous().unwrap_err(), Error::NoPrevious);
    let a = core.next().unwrap();
    let b = core.next().unwrap();
    assert_ne!(a.asset.id, b.asset.id);
    let (before, path) = (fake.count("/api/assets", ""), rec.last_applied().0);
    let r = core.previous().unwrap();
    assert_eq!(r.asset.id, a.asset.id);
    assert_eq!(
        fake.count("/api/assets", ""),
        before,
        "la photo précédente ne doit rien télécharger"
    );
    assert_ne!(rec.last_applied().0, path);
    assert_eq!(core.last().unwrap().0.id, a.asset.id);
    assert_eq!(core.last_photos()[0].asset.id, a.asset.id);
    assert_eq!(core.previous().unwrap_err(), Error::NoPrevious, "historique épuisé");
    // L'historique est borné.
    for _ in 0..HISTORY_DEPTH + 3 {
        core.next().unwrap();
    }
    assert_eq!(core.history_len(), HISTORY_DEPTH);
}

#[test]
fn exclude() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let dir = temp_dir("exclude");
    let core = core_in(&dir, &rec, false, false, false);
    assert!(core.exclude_current().is_err(), "rien à masquer avant le premier affichage");
    core.set_config(test_cfg(&fake.url, &[ALB2]));
    core.next().unwrap();
    let first = core.next().unwrap();
    let hidden = core.exclude_current().unwrap();
    assert_eq!(hidden.id, first.asset.id);
    assert_eq!(core.excluded_count(), 1);
    assert!(
        !core.cache_dir().join(format!("{}.jpg", hidden.id)).exists(),
        "photo masquée encore dans le cache hors ligne"
    );
    for _ in 0..6 {
        assert_ne!(core.next().unwrap().asset.id, hidden.id, "photo masquée réaffichée");
    }
    assert!(!core.history_contains(&hidden.id), "photo masquée encore dans l'historique");
    // La liste survit à un redémarrage, puis peut être vidée.
    let again = core_in(&dir, &rec, false, false, false);
    assert_eq!(again.excluded_count(), 1, "liste d'exclusion non relue");
    again.clear_excluded().unwrap();
    assert_eq!(again.excluded_count(), 0);
    assert_eq!(
        core_in(&dir, &rec, false, false, false).excluded_count(),
        0,
        "liste d'exclusion non vidée sur disque"
    );
    // Les photos récentes survivent aussi à un redémarrage.
    assert!(!core_in(&dir, &rec, false, false, false).recent().is_empty());
}

#[test]
fn favorite() {
    let fake = FakeImmich::start();
    let rec = Arc::new(Rec::default());
    let core = new_core(&rec, false, false, false);
    assert!(core.favorite().is_err(), "aucune photo affichée : erreur attendue");
    core.set_config(test_cfg(&fake.url, &[ALB2]));
    let shown = core.next().unwrap();
    assert_eq!(core.favorite().unwrap().id, shown.asset.id);
    let log = fake.state.log.lock().unwrap().clone();
    assert!(
        log.contains(&("PUT".to_string(), format!("/api/assets/{}", shown.asset.id))),
        "{log:?}"
    );
}

/// 401 = clé refusée (erreur globale) ; 403 = ressource interdite (on passe à la suivante).
#[test]
fn auth_errors() {
    let fake = FakeImmich::start();
    let cfg = test_cfg(&fake.url, &[ALB2]);
    let c = Client::new(&cfg);
    // L2 en « original » : 403 (pas de permission asset.download).
    assert_eq!(c.download(L2, "original").unwrap_err(), Error::Forbidden);
    // fullsize → redirigé vers l'original → 403 → repli sur l'aperçu.
    assert!(!c.download(L2, "fullsize").unwrap().is_empty());
    // fullsize absent (404) → repli sur l'aperçu.
    assert!(!c.download(L1, "fullsize").unwrap().is_empty());
    let mut bad = cfg.clone();
    bad.api_key = "nope".into();
    let b = Client::new(&bad);
    assert_eq!(b.download(L1, "preview").unwrap_err(), Error::Unauthorized);
    assert_eq!(b.albums().unwrap_err(), Error::Unauthorized);
    // Client fermé : plus aucune requête ne part.
    c.close();
    let before = fake.total();
    let e = c.download(L1, "preview").unwrap_err();
    assert!(!e.is_auth() && fake.total() == before, "requête après close : {e}");
    assert_eq!(c.fingerprint(), "", "aucun certificat épinglé sans l'option");
}
