//! Réglages : ce que la fenêtre de réglages demande à l'appli (lecture et enregistrement de la
//! configuration, test de connexion), indépendamment de l'interface. La fenêtre elle-même est
//! dans `win/settings_window.rs` ; aucun port n'est ouvert, aucun navigateur n'est lancé.

use crate::config::{is_uuid, normalize_fingerprint, read_config, read_config_or_default, same_server, save_config, AlbumRule, Config};
use crate::immich::{AlbumInfo, Client};
use crate::util::{err, lock, Res};
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Ce que la fenêtre peut demander à l'appli.
pub trait SettingsHooks: Send + Sync {
    fn saved(&self, cfg: Config);
    fn next(&self) -> Result<String, String>;
    fn status(&self) -> String;
    /// Nombre de photos masquées.
    fn excluded(&self) -> usize;
    /// Les réautorise toutes.
    fn clear_excluded(&self) -> Result<(), String>;
}

pub struct Settings {
    cfg_path: PathBuf,
    /// Protège les lectures/écritures du fichier de configuration.
    file: Mutex<()>,
    /// Empreinte du certificat vu lors du dernier test réussi, par URL de serveur (option
    /// « certificat auto-signé ») : c'est elle qui est épinglée à l'enregistrement.
    seen_pins: Mutex<HashMap<String, String>>,
    hooks: OnceLock<Arc<dyn SettingsHooks>>,
}

/// Configuration affichée dans la fenêtre. Le token n'y figure jamais : `has_key` dit seulement
/// s'il y en a un d'enregistré.
pub struct UiConfig {
    pub cfg: Config,
    pub has_key: bool,
    pub excluded: usize,
    /// Fichier présent mais illisible (les valeurs par défaut sont alors affichées).
    pub error: Option<String>,
}

impl Settings {
    pub fn new(cfg_path: &Path) -> Arc<Settings> {
        Arc::new(Settings {
            cfg_path: cfg_path.to_path_buf(),
            file: Mutex::new(()),
            seen_pins: Mutex::new(HashMap::new()),
            hooks: OnceLock::new(),
        })
    }

    pub fn set_hooks(&self, h: Arc<dyn SettingsHooks>) {
        let _ = self.hooks.set(h);
    }

    pub fn hooks(&self) -> Option<&Arc<dyn SettingsHooks>> {
        self.hooks.get()
    }

    pub fn load(&self) -> UiConfig {
        let res = {
            let _g = lock(&self.file);
            read_config(&self.cfg_path)
        };
        let (mut cfg, error) = match res {
            Ok(c) => (c, None),
            Err(e) if self.cfg_path.exists() => (Config::default(), Some(e.to_string())),
            Err(_) => (Config::default(), None),
        };
        let has_key = !cfg.api_key.is_empty();
        cfg.api_key.clear();
        cfg.api_key_protected.clear();
        UiConfig {
            cfg,
            has_key,
            excluded: self.hooks().map(|h| h.excluded()).unwrap_or(0),
            error,
        }
    }

    /// Teste la connexion avec les réglages saisis et renvoie les albums, triés par nom.
    pub fn albums(&self, input: Config) -> Res<Vec<AlbumInfo>> {
        let mut input = self.merge_key(input)?;
        input.albums = vec!["00000000-0000-0000-0000-000000000000".into()]; // pour passer la validation
        input.album_rules.clear(); // une planification incomplète n'empêche pas le test
        input.active_from.clear();
        input.active_to.clear();
        input.cert_fingerprint.clear(); // test : on relève le certificat présenté
        input.normalize()?;
        let client = Client::new(&input);
        let mut albums = client.albums()?;
        let fp = client.fingerprint();
        if !fp.is_empty() {
            lock(&self.seen_pins).insert(input.server_url.clone(), fp);
        }
        sort_albums(&mut albums);
        Ok(albums)
    }

    /// Télécharge les vignettes de couverture demandées (identifiants de photos Immich) et les
    /// remet à `found` au fur et à mesure, prêtes à afficher. Mêmes garde-fous que le test de
    /// connexion : le token enregistré ne part que vers son serveur, et un certificat auto-signé
    /// doit être celui relevé au test. Les vignettes déjà chargées viennent du cache.
    pub fn album_covers(&self, input: Config, ids: &[String], mut found: impl FnMut(&str, Arc<Cover>)) -> Res<()> {
        let mut input = self.merge_key(input)?;
        input.albums = vec!["00000000-0000-0000-0000-000000000000".into()];
        input.album_rules.clear();
        input.active_from.clear();
        input.active_to.clear();
        input.cert_fingerprint.clear();
        input.normalize()?;
        if input.insecure_skip_verify {
            match lock(&self.seen_pins).get(&input.server_url) {
                Some(fp) => input.cert_fingerprint = fp.clone(),
                None => return err("certificat auto-signé : teste d'abord la connexion"),
            }
        }
        let client = Client::new(&input);
        for id in ids {
            if !is_uuid(id) {
                continue;
            }
            let key = format!("{}/{id}", input.server_url);
            if let Some(c) = lock(&COVERS).get(&key).cloned() {
                found(id, c);
                continue;
            }
            match client.download(id, "thumbnail").and_then(|b| cover_from_image(&b)) {
                Ok(c) => {
                    let c = Arc::new(c);
                    let mut cache = lock(&COVERS);
                    if cache.len() >= MAX_COVERS {
                        cache.clear();
                    }
                    cache.insert(key, c.clone());
                    drop(cache);
                    found(id, c);
                }
                Err(e) => crate::logf!("vignette d'album {id} : {e}"),
            }
        }
        Ok(())
    }

    /// Valide et enregistre la configuration saisie, puis prévient l'appli.
    pub fn save(&self, input: Config) -> Res<Config> {
        let mut check = self.merge_key(input)?;
        check.cert_fingerprint.clear();
        check.normalize()?;
        {
            let _g = lock(&self.file);
            if check.insecure_skip_verify {
                // Empreinte relevée au dernier test de ce serveur, sinon celle déjà enregistrée pour lui.
                if let Some(fp) = lock(&self.seen_pins).get(&check.server_url) {
                    check.cert_fingerprint = fp.clone();
                } else {
                    let old = read_config_or_default(&self.cfg_path);
                    if same_server(&old.server_url, &check.server_url) {
                        check.cert_fingerprint = normalize_fingerprint(&old.cert_fingerprint);
                    }
                }
            }
            save_config(&self.cfg_path, &check)?;
        }
        if let Some(h) = self.hooks() {
            h.saved(check.clone());
        }
        Ok(check)
    }

    /// Garde le token enregistré si le champ est laissé vide, mais seulement pour le serveur
    /// auquel il a été confié : sinon il pourrait être envoyé ailleurs.
    fn merge_key(&self, mut input: Config) -> Res<Config> {
        if !input.api_key.trim().is_empty() {
            return Ok(input);
        }
        let old = {
            let _g = lock(&self.file);
            read_config_or_default(&self.cfg_path)
        };
        if !old.api_key.is_empty() && !same_server(&old.server_url, &input.server_url) {
            return err("l'URL du serveur a changé : saisis de nouveau le token (clé API)");
        }
        input.api_key = old.api_key;
        Ok(input)
    }
}

// ---------- vignettes de couverture ----------

/// Vignette carrée prête à afficher : pixels BGRA (opaques), ligne par ligne.
#[derive(Debug, PartialEq)]
pub struct Cover {
    pub side: u32,
    pub bgra: Vec<u8>,
}

/// Côté maximal des vignettes gardées (de quoi rester nettes sur un écran à 200 %).
pub const COVER_SIDE: u32 = 320;
const MAX_COVERS: usize = 400;

/// Vignettes déjà téléchargées pendant cette session, par serveur et photo.
static COVERS: std::sync::LazyLock<Mutex<HashMap<String, Arc<Cover>>>> = std::sync::LazyLock::new(Default::default);

/// Vignette déjà téléchargée pour ce serveur (adresse normalisée) et cette photo.
pub fn cached_cover(server_url: &str, id: &str) -> Option<Arc<Cover>> {
    lock(&COVERS).get(&format!("{server_url}/{id}")).cloned()
}

/// Ajoute une vignette au cache (captures d'écran des tests).
#[cfg(test)]
pub fn put_cover(server_url: &str, id: &str, c: Cover) {
    lock(&COVERS).insert(format!("{server_url}/{id}"), Arc::new(c));
}

/// Décode une vignette (convertie en sRGB, redressée), la recadre au carré central et la réduit.
pub fn cover_from_image(data: &[u8]) -> Res<Cover> {
    let img = crate::colorprofile::safe_decode(data)?;
    let (w, h) = img.dimensions();
    let s = w.min(h).max(1);
    let side = s.min(COVER_SIDE);
    let crop = (((w - s) / 2) as f64, ((h - s) / 2) as f64, s as f64, s as f64);
    let sq = crate::render::resize(&img, Some(crop), side, side, fast_image_resize::FilterType::CatmullRom);
    let mut bgra = Vec::with_capacity((side * side * 4) as usize);
    for p in sq.pixels() {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    Ok(Cover { side, bgra })
}

// ---------- aides de présentation (indépendantes de l'interface) ----------

/// Valeur de la configuration et libellé affiché dans une liste déroulante.
pub type Choice = (&'static str, &'static str);

pub const STYLES: &[Choice] = &[
    ("fill", "Remplir"),
    ("fit", "Ajuster"),
    ("stretch", "Étirer"),
    ("center", "Centrer"),
    ("tile", "Mosaïque"),
    ("span", "Étendre (multi-écrans)"),
];
pub const SIZES: &[Choice] = &[
    ("fullsize", "Pleine résolution (JPEG)"),
    ("preview", "Aperçu (1440 px, léger)"),
    ("original", "Fichier original"),
];
pub const MONITORS: &[Choice] = &[("same", "La même image partout"), ("different", "Une image différente par écran")];
pub const CAPTIONS: &[Choice] = &[
    ("bottom-right", "En bas à droite"),
    ("bottom-left", "En bas à gauche"),
    ("top-right", "En haut à droite"),
    ("top-left", "En haut à gauche"),
];
pub const SEASONS: &[Choice] = &[
    ("", "Toute l'année"),
    ("spring", "Printemps (mars-mai)"),
    ("summer", "Été (juin-août)"),
    ("autumn", "Automne (sept.-nov.)"),
    ("winter", "Hiver (déc.-fév.)"),
];

/// Position de la valeur dans la liste (la première si elle est inconnue).
pub fn choice_index(list: &[Choice], v: &str) -> usize {
    list.iter().position(|c| c.0 == v).unwrap_or(0)
}

/// Valeur à la position i (la première si i est hors de la liste).
pub fn choice_value(list: &[Choice], i: i32) -> String {
    let i = usize::try_from(i).ok().filter(|&i| i < list.len()).unwrap_or(0);
    list[i].0.to_string()
}

/// L'adresse est en http:// vers autre chose qu'une machine du réseau local : le token et les
/// photos circuleraient en clair sur Internet.
pub fn plain_http_warning(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else {
        return false;
    };
    if u.scheme() != "http" {
        return false;
    }
    let local_v4 = |ip: Ipv4Addr| {
        let o = ip.octets();
        // 100.64.0.0/10 : Tailscale et réseaux d'opérateur.
        ip.is_loopback() || ip.is_private() || ip.is_link_local() || (o[0] == 100 && (o[1] & 0xC0) == 64)
    };
    let local_v6 = |ip: Ipv6Addr| {
        let s = ip.segments()[0];
        ip.is_loopback() || (s & 0xFE00) == 0xFC00 || (s & 0xFFC0) == 0xFE80 || ip.to_ipv4_mapped().is_some_and(local_v4)
    };
    match u.host() {
        Some(url::Host::Ipv4(ip)) => !local_v4(ip),
        Some(url::Host::Ipv6(ip)) => !local_v6(ip),
        Some(url::Host::Domain(h)) => {
            let h = h.to_ascii_lowercase();
            !(h == "localhost"
                || !h.contains('.')
                || [".local", ".lan", ".home", ".internal", ".home.arpa"]
                    .iter()
                    .any(|s| h.ends_with(s)))
        }
        None => false,
    }
}

/// Clé de tri : minuscules, lettres accentuées ramenées à leur base (tri « à la française »).
pub fn sort_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => out.push('a'),
            'æ' => out.push_str("ae"),
            'ç' => out.push('c'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'î' | 'ï' | 'í' | 'ì' => out.push('i'),
            'ñ' => out.push('n'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'ø' => out.push('o'),
            'œ' => out.push_str("oe"),
            'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
            'ÿ' | 'ý' => out.push('y'),
            c => out.push(c),
        }
    }
    out
}

/// Trie par nom, sans tenir compte de la casse ni des accents.
pub fn sort_albums(albums: &mut [AlbumInfo]) {
    albums.sort_by(|a, b| {
        sort_key(&a.album_name)
            .cmp(&sort_key(&b.album_name))
            .then_with(|| a.album_name.cmp(&b.album_name))
    });
}

/// Album proposé à la sélection.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRow {
    pub id: String,
    pub name: String,
    /// Nombre d'éléments, si l'album figure dans la liste chargée depuis Immich.
    pub count: Option<i64>,
    /// Photo de couverture ("" : aucune).
    pub thumb: String,
}

/// Albums chargés depuis Immich, suivis des albums cochés qui n'y figurent pas (pas encore
/// chargés, supprimés ou plus partagés) : ils restent visibles et décochables.
pub fn album_rows(albums: &[AlbumInfo], checked: &[String]) -> Vec<AlbumRow> {
    let mut rows: Vec<AlbumRow> = albums
        .iter()
        .map(|a| AlbumRow {
            id: a.id.clone(),
            name: a.album_name.clone(),
            count: Some(a.asset_count),
            thumb: a.album_thumbnail_asset_id.clone(),
        })
        .collect();
    for id in checked {
        if !rows.iter().any(|r| &r.id == id) {
            let short: String = id.chars().take(8).collect();
            rows.push(AlbumRow {
                id: id.clone(),
                name: format!("(album {short}…)"),
                count: None,
                thumb: String::new(),
            });
        }
    }
    rows
}

/// Le nom contient le texte du filtre (casse et accents ignorés).
pub fn matches_filter(name: &str, filter: &str) -> bool {
    sort_key(name).contains(&sort_key(filter.trim()))
}

/// Ajoute ou retire id de la liste en gardant l'ordre des autres.
pub fn toggle_id(ids: &mut Vec<String>, id: &str, on: bool) {
    let present = ids.iter().any(|x| x == id);
    if on && !present {
        ids.push(id.to_string());
    } else if !on {
        ids.retain(|x| x != id);
    }
}

/// Titre d'une règle d'albums : sa saison.
pub fn rule_title(r: &AlbumRule) -> &'static str {
    let season = SEASONS[choice_index(SEASONS, &r.season)].1;
    season.split(" (").next().unwrap_or(season)
}

/// Détail d'une règle d'albums : heures et nombre d'albums.
pub fn rule_detail(r: &AlbumRule) -> String {
    let dash = |s: &str| if s.is_empty() { "--:--".to_string() } else { s.to_string() };
    let hours = if r.from.is_empty() && r.to.is_empty() {
        "toute la journée".to_string()
    } else {
        format!("{} → {}", dash(&r.from), dash(&r.to))
    };
    let n = match r.albums.len() {
        0 => "aucun album".to_string(),
        1 => "1 album".to_string(),
        n => format!("{n} albums"),
    };
    format!("{hours} · {n}")
}

/// Résumé d'une règle d'albums.
#[cfg(test)]
pub fn rule_label(r: &AlbumRule) -> String {
    format!("{} · {}", rule_title(r), rule_detail(r))
}

/// « 12 éléments ».
pub fn items_label(n: Option<i64>) -> String {
    match n {
        None => "album introuvable".into(),
        Some(0) => "vide".into(),
        Some(1) => "1 élément".into(),
        Some(n) => format!("{n} éléments"),
    }
}

/// « 3 albums sélectionnés ».
pub fn count_label(n: usize) -> String {
    match n {
        0 => "Aucun album sélectionné.".into(),
        1 => "1 album sélectionné".into(),
        n => format!("{n} albums sélectionnés"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::write_default_config;
    use crate::testutil::{temp_dir, FakeImmich, ALB1, ALB2, KEY, L1, L2};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct TestHooks {
        saved: Mutex<Option<Config>>,
        excluded: AtomicUsize,
    }

    impl SettingsHooks for TestHooks {
        fn saved(&self, cfg: Config) {
            *lock(&self.saved) = Some(cfg);
        }
        fn next(&self) -> Result<String, String> {
            Ok("ok".into())
        }
        fn status(&self) -> String {
            "prêt".into()
        }
        fn excluded(&self) -> usize {
            self.excluded.load(Ordering::SeqCst)
        }
        fn clear_excluded(&self) -> Result<(), String> {
            self.excluded.store(0, Ordering::SeqCst);
            Ok(())
        }
    }

    fn input(url: &str, key: &str, albums: &[&str]) -> Config {
        Config {
            server_url: url.into(),
            api_key: key.into(),
            albums: albums.iter().map(|s| s.to_string()).collect(),
            ..Config::default()
        }
    }

    #[test]
    fn settings_logic() {
        let fake = FakeImmich::start();
        let cfg_path = temp_dir("settings").join("config.json");
        write_default_config(&cfg_path).unwrap();
        let s = Settings::new(&cfg_path);
        let hooks = Arc::new(TestHooks::default());
        hooks.excluded.store(3, Ordering::SeqCst);
        s.set_hooks(hooks.clone());

        // Liste des albums, triée par nom.
        let albums = s.albums(input(&fake.url, KEY, &[])).unwrap();
        assert!(albums.iter().any(|a| a.album_name.contains("Bassin")), "{albums:?}");
        assert!(albums.windows(2).all(|w| sort_key(&w[0].album_name) <= sort_key(&w[1].album_name)));
        let e = s.albums(input(&fake.url, "bad", &[])).unwrap_err();
        assert!(e.to_string().contains("token"), "{e}");
        // Une plage horaire incomplète n'empêche pas de tester la connexion.
        let mut half = input(&fake.url, KEY, &[]);
        half.active_from = "08:00".into();
        assert!(s.albums(half).is_ok());

        // Enregistrement.
        let mut c = input(&fake.url, KEY, &[ALB2]);
        c.interval_minutes = 15;
        c.style = "fit".into();
        s.save(c).unwrap();
        assert_eq!(lock(&hooks.saved).as_ref().unwrap().style, "fit");
        // Nouvel enregistrement sans token → le token est conservé.
        s.save(input(&fake.url, "", &[ALB1, ALB2])).unwrap();
        {
            let saved = lock(&hooks.saved);
            let saved = saved.as_ref().unwrap();
            assert!(saved.api_key == KEY && saved.albums.len() == 2);
        }
        // Le token n'est jamais renvoyé à la fenêtre ; le nombre de photos masquées est indiqué.
        let ui = s.load();
        assert!(ui.cfg.api_key.is_empty() && ui.cfg.api_key_protected.is_empty());
        assert!(ui.has_key && ui.excluded == 3 && ui.cfg.preload && ui.error.is_none());
        assert_eq!(ui.cfg.albums, vec![ALB1.to_string(), ALB2.to_string()]);
        // Album manquant → erreur, rien n'est enregistré.
        assert!(s.save(input(&fake.url, "", &[])).is_err());
        assert_eq!(s.load().cfg.albums.len(), 2);

        // Le token enregistré n'est jamais envoyé à un autre serveur que le sien.
        let leaked = Arc::new(AtomicBool::new(false));
        let other = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let other_url = format!("http://{}", other.server_addr());
        let l = leaked.clone();
        std::thread::spawn(move || {
            for req in other.incoming_requests() {
                if req.headers().iter().any(|h| h.field.equiv("x-api-key") && h.value.as_str() == KEY) {
                    l.store(true, Ordering::SeqCst);
                }
                let _ = req.respond(tiny_http::Response::from_string("[]"));
            }
        });
        assert!(s.albums(input(&other_url, "", &[ALB1])).is_err());
        assert!(s.save(input(&other_url, "", &[ALB1])).is_err());
        assert!(!leaked.load(Ordering::SeqCst), "token réutilisé pour un autre serveur");

        // Vignettes de couverture : téléchargées une fois, recadrées au carré, puis servies par le cache.
        let ids: Vec<String> = albums.iter().map(|a| a.album_thumbnail_asset_id.clone()).collect();
        assert!(ids.contains(&L1.to_string()) && ids.contains(&L2.to_string()), "{ids:?}");
        let mut got = vec![];
        s.album_covers(input(&fake.url, "", &[]), &ids, |id, c| got.push((id.to_string(), c)))
            .unwrap();
        assert_eq!(got.len(), 2);
        assert!(
            got.iter().all(|(_, c)| c.side == 40 && c.bgra.len() == 40 * 40 * 4),
            "image 80x40 → carré 40x40"
        );
        let before = fake.count("/api/assets", "/thumbnail");
        s.album_covers(input(&fake.url, "", &[]), &ids, |_, _| {}).unwrap();
        assert_eq!(
            fake.count("/api/assets", "/thumbnail"),
            before,
            "vignettes retéléchargées malgré le cache"
        );
        // Identifiant invalide ignoré ; jamais de token vers un autre serveur.
        s.album_covers(input(&fake.url, "", &[]), &["../config".to_string()], |_, _| {
            panic!("identifiant invalide accepté")
        })
        .unwrap();
        assert!(s.album_covers(input(&other_url, "", &[]), &ids, |_, _| {}).is_err());
        // Certificat auto-signé : la vignette n'est demandée qu'après un test (empreinte relevée).
        let mut insecure = input("https://autre.exemple.fr", KEY, &[]);
        insecure.insecure_skip_verify = true;
        assert!(s.album_covers(insecure, &ids, |_, _| {}).is_err());

        // Fichier illisible : valeurs par défaut et erreur signalée.
        std::fs::write(&cfg_path, "{ pas du json").unwrap();
        let ui = s.load();
        assert!(ui.error.is_some() && !ui.has_key && ui.cfg.server_url == Config::default().server_url);
    }

    #[test]
    fn choices_are_valid() {
        for (list, field) in [
            (STYLES, "style"),
            (SIZES, "image_size"),
            (MONITORS, "multi_monitor"),
            (CAPTIONS, "caption_position"),
        ] {
            for (i, (v, _)) in list.iter().enumerate() {
                assert_eq!(choice_index(list, v), i);
                assert_eq!(choice_value(list, i as i32), *v);
                let mut c = input("https://immich.exemple.fr", KEY, &[ALB1]);
                match field {
                    "style" => c.style = v.to_string(),
                    "image_size" => c.image_size = v.to_string(),
                    "multi_monitor" => c.multi_monitor = v.to_string(),
                    _ => c.caption_position = v.to_string(),
                }
                c.normalize().unwrap();
                let got = match field {
                    "style" => &c.style,
                    "image_size" => &c.image_size,
                    "multi_monitor" => &c.multi_monitor,
                    _ => &c.caption_position,
                };
                assert_eq!(got, v, "{field} : valeur modifiée par la validation");
            }
        }
        for (v, _) in SEASONS {
            let mut c = input("https://immich.exemple.fr", KEY, &[ALB1]);
            c.album_rules = vec![AlbumRule {
                albums: vec![ALB2.into()],
                season: v.to_string(),
                ..Default::default()
            }];
            c.normalize().unwrap();
            assert_eq!(c.album_rules[0].season, *v);
        }
        assert_eq!(choice_index(STYLES, "inconnu"), 0);
        assert_eq!(choice_value(STYLES, -1), "fill");
        assert_eq!(choice_value(STYLES, 99), "fill");
    }

    #[test]
    fn http_warning() {
        for (url, warn) in [
            ("http://immich.exemple.fr", true),
            ("http://203.0.113.5:2283", true),
            ("http://[2001:db8::1]", true),
            ("https://immich.exemple.fr", false),
            ("http://192.168.1.10:2283", false),
            ("http://10.0.0.2", false),
            ("http://172.20.0.1", false),
            ("http://100.101.102.103", false),
            ("http://127.0.0.1:2283", false),
            ("http://localhost:2283", false),
            ("http://nas:2283", false),
            ("http://nas.local", false),
            ("http://immich.home.arpa", false),
            ("http://[fd00::1]", false),
            ("http://[::ffff:192.168.1.2]", false),
            ("pas une url", false),
        ] {
            assert_eq!(plain_http_warning(url), warn, "{url}");
        }
    }

    #[test]
    fn album_list_helpers() {
        let mk = |id: &str, name: &str, n: i64| AlbumInfo {
            id: id.into(),
            album_name: name.into(),
            asset_count: n,
            ..Default::default()
        };
        let mut albums = vec![mk("3", "Zoo", 1), mk("1", "été 2024", 2), mk("2", "Écosse", 3), mk("4", "avril", 4)];
        sort_albums(&mut albums);
        let names: Vec<&str> = albums.iter().map(|a| a.album_name.as_str()).collect();
        assert_eq!(names, ["avril", "Écosse", "été 2024", "Zoo"]);

        let rows = album_rows(&albums, &["2".into(), "99999999-aaaa".into()]);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[4],
            AlbumRow {
                id: "99999999-aaaa".into(),
                name: "(album 99999999…)".into(),
                count: None,
                thumb: String::new(),
            }
        );
        assert_eq!(rows[1].count, Some(3));
        assert!(matches_filter("Été 2024", "ete"));
        assert!(matches_filter("Écosse", " ÉCO "));
        assert!(!matches_filter("Zoo", "eco"));

        let mut ids = vec!["a".to_string(), "b".into()];
        toggle_id(&mut ids, "c", true);
        toggle_id(&mut ids, "a", true);
        assert_eq!(ids, ["a", "b", "c"]);
        toggle_id(&mut ids, "b", false);
        toggle_id(&mut ids, "z", false);
        assert_eq!(ids, ["a", "c"]);

        assert_eq!(count_label(0), "Aucun album sélectionné.");
        assert_eq!(count_label(1), "1 album sélectionné");
        assert_eq!(count_label(4), "4 albums sélectionnés");
        assert_eq!(rule_label(&AlbumRule::default()), "Toute l'année · toute la journée · aucun album");
        let r = AlbumRule {
            albums: vec!["a".into(), "b".into()],
            season: "summer".into(),
            from: "08:00".into(),
            to: "".into(),
        };
        assert_eq!(rule_label(&r), "Été · 08:00 → --:-- · 2 albums");
        assert_eq!((rule_title(&r), rule_detail(&r).as_str()), ("Été", "08:00 → --:-- · 2 albums"));
        // Vignette : carré central, réduite au plus à COVER_SIDE.
        let c = cover_from_image(&crate::testutil::mk_img(900, 500, "png")).unwrap();
        assert_eq!((c.side, c.bgra.len()), (COVER_SIDE, (COVER_SIDE * COVER_SIDE * 4) as usize));
        assert!(cover_from_image(b"pas une image").is_err());
    }
}
