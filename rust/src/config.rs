//! Configuration (config.json), validation, plage horaire et règles d'albums.
//! Le format du fichier est celui de la version Go : les réglages existants sont repris tels quels.

use crate::util::{err, Error, Local, Res};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

/// Accepte `null` là où une valeur est attendue (comme le faisait la version Go).
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Albums à utiliser à la place des albums habituels pendant une saison et/ou une plage horaire.
/// Sans saison ni horaires, la règle s'applique tout le temps.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct AlbumRule {
    #[serde(deserialize_with = "null_default")]
    pub albums: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub season: String, // "" | spring | summer | autumn | winter
    #[serde(deserialize_with = "null_default")]
    pub from: String, // "HH:MM" ; vide avec `to` = toute la journée
    #[serde(deserialize_with = "null_default")]
    pub to: String,
}

/// Lue depuis %APPDATA%\ImmichWallpaper\config.json.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    #[serde(deserialize_with = "null_default")]
    pub server_url: String,
    /// Clé en clair, en mémoire. Sur disque elle est chiffrée dans `api_key_protected`
    /// (DPAPI, liée au compte Windows) ; une clé saisie en clair à la main reste acceptée.
    #[serde(deserialize_with = "null_default")]
    pub api_key: String,
    #[serde(deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub api_key_protected: String,
    /// Empreinte SHA-256 du certificat accepté quand insecure_skip_verify est actif.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "String::is_empty")]
    pub cert_fingerprint: String,
    #[serde(deserialize_with = "null_default")]
    pub albums: Vec<String>,
    pub interval_minutes: i64,
    #[serde(deserialize_with = "null_default")]
    pub image_size: String, // fullsize | preview | original
    pub landscape_only: bool,
    #[serde(deserialize_with = "null_default")]
    pub style: String, // fill | fit | stretch | center | tile | span
    pub change_on_start: bool,
    pub insecure_skip_verify: bool,
    pub lock_screen: bool,
    pub show_place: bool,
    pub show_date: bool,
    #[serde(deserialize_with = "null_default")]
    pub caption_position: String, // bottom-right | bottom-left | top-right | top-left
    pub desktop_link: bool,
    pub auto_update: bool,
    #[serde(deserialize_with = "null_default")]
    pub multi_monitor: String, // same | different
    pub offline_cache: i64, // nombre de photos gardées pour le mode hors ligne (0 = désactivé)
    pub split_wide: bool,
    pub change_on_unlock: bool,

    // Planification contextuelle (changements automatiques uniquement).
    #[serde(deserialize_with = "null_default")]
    pub active_from: String, // "HH:MM" (vide = toute la journée)
    #[serde(deserialize_with = "null_default")]
    pub active_to: String,
    pub pause_on_battery: bool,
    pub pause_on_metered: bool,
    pub pause_on_fullscreen: bool,
    #[serde(deserialize_with = "null_default")]
    pub album_rules: Vec<AlbumRule>,
    pub preload: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: "https://immich.exemple.fr".into(),
            api_key: String::new(),
            api_key_protected: String::new(),
            cert_fingerprint: String::new(),
            albums: vec![],
            interval_minutes: 60,
            image_size: "fullsize".into(),
            landscape_only: true,
            style: "fill".into(),
            change_on_start: true,
            insecure_skip_verify: false,
            lock_screen: false,
            show_place: true,
            show_date: false,
            caption_position: "bottom-right".into(),
            // Portable : rien n'est déposé hors de son dossier sans le demander.
            desktop_link: !crate::is_portable(),
            auto_update: true,
            multi_monitor: "same".into(),
            offline_cache: 40,
            split_wide: true,
            change_on_unlock: true,
            active_from: String::new(),
            active_to: String::new(),
            pause_on_battery: false,
            pause_on_metered: false,
            pause_on_fullscreen: false,
            album_rules: vec![],
            preload: true,
        }
    }
}

// ---------- chiffrement de la clé (DPAPI sous Windows) ----------

#[cfg(windows)]
pub use crate::win::platform::{protect_secret, unprotect_secret};

#[cfg(not(windows))]
pub fn protect_secret(_s: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn unprotect_secret(_enc: &str) -> Result<String, String> {
    Err("clé chiffrée par Windows, illisible sur ce système".into())
}

// ---------- lecture / écriture ----------

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b) // Notepad peut ajouter un BOM UTF-8
}

pub fn write_default_config(path: &Path) -> std::io::Result<()> {
    let b = serde_json::to_vec_pretty(&Config::default()).unwrap_or_default();
    std::fs::write(path, b)
}

/// Lit le fichier sans valider (pour la page de réglages).
pub fn read_config(path: &Path) -> Res<Config> {
    let b = std::fs::read(path)?;
    let mut cfg: Config = serde_json::from_slice(strip_bom(&b)).map_err(|e| Error::Msg(format!("JSON invalide : {e}")))?;
    if cfg.api_key.is_empty() && !cfg.api_key_protected.is_empty() {
        match unprotect_secret(&cfg.api_key_protected) {
            Ok(k) => cfg.api_key = k,
            // Fichier copié depuis un autre PC ou un autre compte : la clé doit être ressaisie.
            Err(e) => crate::logf!("clé API illisible (chiffrée pour un autre compte Windows ?) : {e}"),
        }
    }
    cfg.api_key_protected.clear();
    Ok(cfg)
}

/// Lit le fichier ; s'il est absent ou illisible, renvoie les valeurs par défaut.
pub fn read_config_or_default(path: &Path) -> Config {
    read_config(path).unwrap_or_default()
}

/// Indique si le fichier contient encore la clé API en clair.
pub fn key_stored_plain(path: &Path) -> bool {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default, deserialize_with = "null_default")]
        api_key: String,
    }
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Raw>(strip_bom(&b)).ok())
        .map(|r| !r.api_key.is_empty())
        .unwrap_or(false)
}

/// Lit et valide la config.
pub fn load_config(path: &Path) -> Res<Config> {
    let mut cfg = read_config(path)?;
    cfg.normalize()?;
    Ok(cfg)
}

/// Écriture atomique (fichier temporaire puis renommage) ; la clé est chiffrée si possible.
pub fn save_config(path: &Path, cfg: &Config) -> Res<()> {
    let mut out = cfg.clone();
    out.api_key_protected.clear();
    if let Some(enc) = protect_secret(&out.api_key) {
        out.api_key.clear();
        out.api_key_protected = enc;
    }
    let b = serde_json::to_vec_pretty(&out).map_err(|e| Error::Msg(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------- validation ----------

fn uuid_at(b: &[u8]) -> bool {
    if b.len() < 36 {
        return false;
    }
    b[..36].iter().enumerate().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => *c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

/// La chaîne est exactement un UUID (rien avant, rien après).
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36 && uuid_at(s.as_bytes())
}

/// Accepte un UUID brut ou une URL du type https://immich/albums/<uuid> (dernier UUID trouvé).
pub fn parse_album_id(s: &str) -> Res<String> {
    let b = s.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i + 36 <= b.len() {
        if uuid_at(&b[i..]) {
            found = Some(i);
            i += 36;
        } else {
            i += 1;
        }
    }
    match found {
        Some(i) => Ok(s[i..i + 36].to_ascii_lowercase()),
        None => err(format!("album {s:?} : aucun identifiant trouvé")),
    }
}

/// Nettoie et valide l'adresse du serveur Immich.
pub fn normalize_server_url(s: &str) -> Res<String> {
    let u = s.trim().trim_end_matches('/');
    let u = u.strip_suffix("/api").unwrap_or(u);
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return err("server_url doit commencer par http:// ou https://");
    }
    // Caractères de contrôle (retours à la ligne…) : jamais dans une URL, et dangereux dans le
    // raccourci du bureau (fichier INI).
    if u.chars().any(|c| c.is_control() || c == ' ') {
        return err("server_url invalide");
    }
    let p = match url::Url::parse(u) {
        Ok(p) if p.host_str().map(|h| !h.is_empty()).unwrap_or(false) => p,
        _ => return err("server_url invalide"),
    };
    if !p.username().is_empty() || p.password().is_some() || p.query().is_some() || p.fragment().is_some() {
        return err("server_url ne doit contenir ni identifiants, ni « ? », ni « # »");
    }
    Ok(u.to_string())
}

/// Les deux adresses désignent le même serveur une fois normalisées.
pub fn same_server(a: &str, b: &str) -> bool {
    match (normalize_server_url(a), normalize_server_url(b)) {
        (Ok(x), Ok(y)) => x.eq_ignore_ascii_case(&y),
        _ => false,
    }
}

pub fn normalize_fingerprint(s: &str) -> String {
    s.trim().replace([':', ' '], "").to_lowercase()
}

/// WallpaperStyle / TileWallpaper pour le registre Windows.
pub fn style_values(style: &str) -> Option<(&'static str, &'static str)> {
    Some(match style {
        "fill" => ("10", "0"),
        "fit" => ("6", "0"),
        "stretch" => ("2", "0"),
        "center" => ("0", "0"),
        "tile" => ("0", "1"),
        "span" => ("22", "0"),
        _ => return None,
    })
}

fn unique_ids<'a>(lists: impl IntoIterator<Item = &'a Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for l in lists {
        for id in l {
            if !out.contains(id) {
                out.push(id.clone());
            }
        }
    }
    out
}

impl Config {
    pub fn normalize(&mut self) -> Res<()> {
        self.server_url = normalize_server_url(&self.server_url)?;
        self.cert_fingerprint = normalize_fingerprint(&self.cert_fingerprint);
        if !self.insecure_skip_verify {
            self.cert_fingerprint.clear();
        }

        self.api_key = self.api_key.trim().to_string();
        if self.api_key.is_empty() {
            return err("api_key est vide (Immich → Paramètres du compte → Clés API)");
        }

        let mut ids = vec![];
        for a in &self.albums {
            ids.push(parse_album_id(a)?);
        }
        let ids = unique_ids([&ids]);
        if ids.is_empty() {
            return err("albums est vide");
        }
        self.albums = ids;

        self.normalize_schedule()?;
        if self.interval_minutes < 1 {
            self.interval_minutes = 60;
        }
        self.image_size = self.image_size.trim().to_lowercase();
        match self.image_size.as_str() {
            "" => self.image_size = "fullsize".into(),
            "fullsize" | "preview" | "original" => {}
            other => return err(format!("image_size {other:?} inconnu (fullsize, preview ou original)")),
        }
        self.style = self.style.trim().to_lowercase();
        if self.style.is_empty() {
            self.style = "fill".into();
        }
        if self.multi_monitor != "different" {
            self.multi_monitor = "same".into();
        }
        self.offline_cache = self.offline_cache.clamp(0, 500);
        if !matches!(
            self.caption_position.as_str(),
            "bottom-right" | "bottom-left" | "top-right" | "top-left"
        ) {
            self.caption_position = "bottom-right".into();
        }
        if style_values(&self.style).is_none() {
            return err(format!("style {:?} inconnu (fill, fit, stretch, center, tile, span)", self.style));
        }
        Ok(())
    }

    /// Valide la plage horaire et les règles d'albums.
    fn normalize_schedule(&mut self) -> Res<()> {
        self.active_from = normalize_clock(&self.active_from)?;
        self.active_to = normalize_clock(&self.active_to)?;
        if self.active_from.is_empty() != self.active_to.is_empty() {
            return err("plage horaire incomplète : indique l'heure de début et l'heure de fin, ou aucune");
        }
        let mut rules = vec![];
        for r in &self.album_rules {
            let mut out = AlbumRule {
                season: r.season.trim().to_lowercase(),
                ..Default::default()
            };
            if !matches!(out.season.as_str(), "" | "spring" | "summer" | "autumn" | "winter") {
                return err(format!("saison {:?} inconnue (spring, summer, autumn ou winter)", r.season));
            }
            out.from = normalize_clock(&r.from)?;
            out.to = normalize_clock(&r.to)?;
            if out.from.is_empty() != out.to.is_empty() {
                return err("règle d'albums : plage horaire incomplète");
            }
            let mut ids = vec![];
            for a in &r.albums {
                ids.push(parse_album_id(a)?);
            }
            out.albums = unique_ids([&ids]);
            if out.albums.is_empty() {
                continue; // règle sans album : ignorée
            }
            rules.push(out);
        }
        self.album_rules = rules;
        Ok(())
    }

    /// Les changements automatiques sont autorisés à cette heure.
    pub fn active_at(&self, t: &Local) -> bool {
        in_clock_range(&self.active_from, &self.active_to, t)
    }

    /// Albums à utiliser à l'instant t : ceux des règles qui s'appliquent, sinon les albums habituels.
    pub fn albums_at(&self, t: &Local) -> Vec<String> {
        let ids = unique_ids(self.album_rules.iter().filter(|r| r.matches(t)).map(|r| &r.albums));
        if ids.is_empty() {
            self.albums.clone()
        } else {
            ids
        }
    }

    /// Tous les albums référencés (habituels et règles).
    pub fn all_albums(&self) -> Vec<String> {
        unique_ids(std::iter::once(&self.albums).chain(self.album_rules.iter().map(|r| &r.albums)))
    }

    pub fn want_info(&self) -> bool {
        self.show_place || self.show_date
    }
}

// ---------- plage horaire, saisons ----------

/// Lit « HH:MM » et renvoie le nombre de minutes depuis minuit.
pub fn parse_clock(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h <= 23 && m <= 59).then_some(h * 60 + m)
}

/// Accepte une heure vide ou « H:MM » et la renvoie sous la forme « HH:MM ».
pub fn normalize_clock(s: &str) -> Res<String> {
    if s.trim().is_empty() {
        return Ok(String::new());
    }
    match parse_clock(s) {
        Some(v) => Ok(format!("{:02}:{:02}", v / 60, v % 60)),
        None => err(format!("heure {s:?} invalide (format attendu : HH:MM)")),
    }
}

/// L'heure t est dans la plage [from, to). Une plage dont la fin précède le début passe par
/// minuit (22:00 → 07:00). Plage vide, incomplète ou de durée nulle : toujours vrai.
pub fn in_clock_range(from: &str, to: &str, t: &Local) -> bool {
    let (f, e) = match (parse_clock(from), parse_clock(to)) {
        (Some(f), Some(e)) if f != e => (f, e),
        _ => return true,
    };
    let now = t.hour * 60 + t.minute;
    if f < e {
        now >= f && now < e
    } else {
        now >= f || now < e
    }
}

/// Saison météorologique (hémisphère nord) du mois donné (1..=12).
pub fn season_of(month: u32) -> &'static str {
    match month {
        3..=5 => "spring",
        6..=8 => "summer",
        9..=11 => "autumn",
        _ => "winter",
    }
}

impl AlbumRule {
    pub fn matches(&self, t: &Local) -> bool {
        (self.season.is_empty() || self.season == season_of(t.month)) && in_clock_range(&self.from, &self.to, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ALB1, ALB2, KEY};

    fn at(month: u32, hour: u32, minute: u32) -> Local {
        Local {
            year: 2026,
            month,
            day: 15,
            hour,
            minute,
            second: 0,
        }
    }

    #[test]
    fn album_id() {
        let id = parse_album_id(&format!("https://immich.local/albums/{}?x=1", ALB1.to_uppercase())).unwrap();
        assert_eq!(id, ALB1);
        assert!(parse_album_id("pas-un-album").is_err());
        assert!(!is_uuid(&format!("x{ALB1}")) && !is_uuid(&format!("{ALB1}\n")) && !is_uuid("..\\config") && is_uuid(ALB1));
    }

    #[test]
    fn server_url() {
        for (input, want) in [
            (" https://immich.exemple.fr/api/ ", "https://immich.exemple.fr"),
            ("http://192.168.1.10:2283", "http://192.168.1.10:2283"),
            ("https://exemple.fr/immich", "https://exemple.fr/immich"),
        ] {
            assert_eq!(normalize_server_url(input).unwrap(), want);
        }
        for bad in [
            "immich.local",
            "https://",
            "https://a.fr\r\nIconFile=x",
            "https://a.fr?x=1",
            "https://a.fr#x",
            "https://u:p@a.fr",
            "ftp://a.fr",
        ] {
            assert!(normalize_server_url(bad).is_err(), "{bad:?} accepté");
        }
        assert!(same_server("https://A.fr/api/", "https://a.fr") && !same_server("https://a.fr", "https://b.fr"));
    }

    #[test]
    fn clock_ranges() {
        let cases = [
            ("", "", 3, 0, true),
            ("08:00", "", 3, 0, true),
            ("08:00", "08:00", 3, 0, true),
            ("08:00", "18:30", 8, 0, true),
            ("08:00", "18:30", 18, 29, true),
            ("08:00", "18:30", 18, 30, false),
            ("08:00", "18:30", 7, 59, false),
            ("22:00", "07:00", 23, 0, true),
            ("22:00", "07:00", 6, 59, true),
            ("22:00", "07:00", 7, 0, false),
            ("22:00", "07:00", 12, 0, false),
        ];
        for (from, to, h, m, want) in cases {
            assert_eq!(in_clock_range(from, to, &at(5, h, m)), want, "{from}-{to} à {h}:{m}");
        }
        for (m, want) in [(1, "winter"), (4, "spring"), (7, "summer"), (10, "autumn"), (12, "winter")] {
            assert_eq!(season_of(m), want);
        }
        assert_eq!(normalize_clock("7:05").unwrap(), "07:05");
        for bad in ["25:00", "12", "12:60", "midi"] {
            assert!(normalize_clock(bad).is_err(), "{bad:?} accepté");
        }
    }

    fn base() -> Config {
        Config {
            server_url: "https://immich.exemple.fr".into(),
            api_key: KEY.into(),
            albums: vec![ALB1.into()],
            ..Default::default()
        }
    }

    #[test]
    fn album_rules() {
        let mut c = base();
        c.active_from = "8:00".into();
        c.active_to = "22:00".into();
        c.album_rules = vec![
            AlbumRule {
                albums: vec![format!("https://immich.exemple.fr/albums/{ALB2}"), ALB2.into()],
                season: "Summer".into(),
                ..Default::default()
            },
            AlbumRule {
                albums: vec![],
                season: "winter".into(),
                ..Default::default()
            }, // sans album : ignorée
        ];
        c.normalize().unwrap();
        assert_eq!(c.album_rules.len(), 1);
        assert_eq!(c.album_rules[0].albums, vec![ALB2.to_string()]);
        assert_eq!(c.active_from, "08:00");
        assert_eq!(c.albums_at(&at(7, 12, 0)), vec![ALB2.to_string()]);
        assert_eq!(c.albums_at(&at(1, 12, 0)), vec![ALB1.to_string()]);
        assert_eq!(c.all_albums().len(), 2);
        assert!(!c.active_at(&at(5, 23, 0)) && c.active_at(&at(5, 9, 0)));

        let bad: [fn(&mut Config); 4] = [
            |c| c.active_to.clear(),
            |c| {
                c.album_rules = vec![AlbumRule {
                    albums: vec![ALB2.into()],
                    season: "mousson".into(),
                    ..Default::default()
                }]
            },
            |c| {
                c.album_rules = vec![AlbumRule {
                    albums: vec![ALB2.into()],
                    from: "9h".into(),
                    ..Default::default()
                }]
            },
            |c| {
                c.album_rules = vec![AlbumRule {
                    albums: vec!["pas-un-album".into()],
                    ..Default::default()
                }]
            },
        ];
        for f in bad {
            let mut d = c.clone();
            f(&mut d);
            assert!(d.normalize().is_err(), "configuration invalide acceptée : {d:?}");
        }
    }

    #[test]
    fn normalize_defaults() {
        let mut c = base();
        c.server_url = "https://immich.exemple.fr/api/".into();
        c.style = " FIT ".into();
        c.image_size = String::new();
        c.interval_minutes = 0;
        c.offline_cache = 9999;
        c.caption_position = "milieu".into();
        c.cert_fingerprint = "AB:CD".into();
        c.normalize().unwrap();
        assert_eq!(
            (
                c.server_url.as_str(),
                c.style.as_str(),
                c.image_size.as_str(),
                c.interval_minutes,
                c.offline_cache
            ),
            ("https://immich.exemple.fr", "fit", "fullsize", 60, 500)
        );
        assert_eq!(c.caption_position, "bottom-right");
        assert!(c.cert_fingerprint.is_empty(), "empreinte gardée sans insecure_skip_verify");
        let mut d = base();
        d.style = "zoom".into();
        assert!(d.normalize().is_err());
        let mut e = base();
        e.api_key = "  ".into();
        assert!(e.normalize().is_err());
        let mut f = base();
        f.albums.clear();
        assert!(f.normalize().is_err());
    }

    /// Un fichier écrit par la version Go (champs `null`, champs absents, BOM) doit être relu tel quel.
    #[test]
    fn reads_go_config() {
        let dir = crate::testutil::temp_dir("cfg");
        let path = dir.join("config.json");
        let go = format!(
            "\u{feff}{{\"server_url\":\"https://immich.exemple.fr\",\"api_key\":\"{KEY}\",\"albums\":[\"{ALB1}\"],\
             \"interval_minutes\":15,\"style\":\"fit\",\"album_rules\":null,\"champ_inconnu\":1}}"
        );
        std::fs::write(&path, go).unwrap();
        assert!(key_stored_plain(&path));
        let c = load_config(&path).unwrap();
        assert_eq!((c.interval_minutes, c.style.as_str(), c.api_key.as_str()), (15, "fit", KEY));
        // Champs absents : valeurs par défaut (et non zéro).
        assert!(c.preload && c.change_on_unlock && c.split_wide && c.offline_cache == 40 && c.album_rules.is_empty());

        // La clé ne reste en clair sur disque que si le système ne sait pas la chiffrer.
        save_config(&path, &c).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        if protect_secret("x").is_some() {
            assert!(!raw.contains(KEY) && !key_stored_plain(&path), "clé en clair : {raw}");
        }
        let again = read_config(&path).unwrap();
        assert_eq!(again.api_key, KEY);
        assert!(again.api_key_protected.is_empty());
    }
}
