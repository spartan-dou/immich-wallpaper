//! Logique indépendante de Windows : albums, choix des photos, cache hors ligne,
//! préchargement, rendu par écran et application du fond d'écran.

use crate::caption::{caption_parts, draw_caption, load_fonts, Fonts};
use crate::colorprofile::{probe, safe_decode, Meta};
use crate::config::{is_uuid, Config};
use crate::immich::{Asset, AssetInfo, Client};
use crate::logf;
use crate::render::{blit, compose, decode_jpeg, encode_jpeg, jpeg_dims, shrink_jpeg, Screen, KEPT_MAX_SIDE};
use crate::util::{err, lock, Error, Local, Res};
use image::RgbImage;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fréquence de vérification des albums (une requête légère ; le contenu n'est rechargé que s'il a changé).
const ALBUM_CHECK_EVERY: Duration = Duration::from_secs(30 * 60);

/// Nombre de fonds d'écran précédents gardés pour « Photo précédente ».
pub const HISTORY_DEPTH: usize = 5;

/// Écran « très large » à partir de ce rapport largeur/hauteur (32:9 ≈ 3,56 ; 21:9 ≈ 2,39 n'est pas découpé).
const SPLIT_RATIO: f64 = 2.8;

/// Écran cible (mode « une image par écran »). id = chemin de périphérique Windows.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub id: String,
    pub screen: Screen,
}

/// Image prête à être rendue (déjà redressée et convertie en sRGB).
pub struct Photo {
    pub asset: Asset,
    pub img: RgbImage,
    pub info: AssetInfo,
    /// Version réduite (KEPT_MAX_SIDE) encodée en JPEG.
    jpg: Option<Arc<Vec<u8>>>,
}

/// Photo affichée, gardée en JPEG (≈10× plus léger que l'image décodée).
#[derive(Clone)]
pub struct KeptPhoto {
    pub asset: Asset,
    pub info: AssetInfo,
    pub jpeg: Arc<Vec<u8>>,
}

struct AlbumState {
    sig: String,
    assets: Vec<Asset>,
}

/// Photo du prochain changement, déjà téléchargée et gardée sur disque (pas en RAM).
struct PreItem {
    asset: Asset,
    info: AssetInfo,
    has_info: bool,
    path: PathBuf,
}

/// Décrit le dernier changement.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub asset: Asset,
    /// Photo tirée du cache local.
    pub offline: bool,
    /// Pourquoi on est passé hors ligne.
    pub cause: Option<Error>,
}

type Hook<A, R> = Box<dyn Fn(A) -> R + Send + Sync>;

/// Fonctions fournies par la plateforme (Windows) ; remplacées par des faux dans les tests.
pub struct Hooks {
    pub set_wallpaper: Box<dyn Fn(&Path, &str) -> Res<()> + Send + Sync>,
    pub set_monitor_wallpapers: Option<Box<dyn Fn(&[(String, PathBuf)], &str) -> Res<()> + Send + Sync>>,
    pub monitors: Option<Hook<(), Vec<Monitor>>>,
    pub set_lock_screen: Option<Box<dyn Fn(&Path) -> Res<()> + Send + Sync>>,
    pub screen: Option<Box<dyn Fn(&str) -> Screen + Send + Sync>>,
    pub after_apply: Option<Box<dyn Fn(&Config, &Asset, &str) + Send + Sync>>,
    /// Horloge (remplaçable dans les tests) : règles d'albums par heure/saison.
    pub now: Hook<(), Local>,
}

impl Hooks {
    pub fn new(set_wallpaper: impl Fn(&Path, &str) -> Res<()> + Send + Sync + 'static) -> Hooks {
        Hooks {
            set_wallpaper: Box::new(set_wallpaper),
            set_monitor_wallpapers: None,
            monitors: None,
            set_lock_screen: None,
            screen: None,
            after_apply: None,
            now: Box::new(|_| crate::util::now_local()),
        }
    }
}

/// État utilisé uniquement pendant les opérations longues (sous le verrou `run`).
struct RunState {
    flip: bool,
    fonts: Option<Option<Fonts>>,
}

/// État partagé ; son verrou n'est jamais tenu pendant une entrée/sortie réseau,
/// pour que le menu et la page de réglages ne soient pas bloqués par un tirage.
struct State {
    cfg: Option<Arc<Config>>,
    client: Option<Arc<Client>>,
    gen: u64, // incrémenté à chaque changement de configuration
    albums: HashMap<String, Arc<AlbumState>>,
    albums_checked: Option<Instant>,
    /// Nombre d'images de tous les albums référencés ; None tant que rien n'est chargé.
    assets: Option<usize>,
    recent: Vec<String>,
    last: Option<Asset>,
    lock_err: Option<String>,
    last_photos: Vec<KeptPhoto>,  // photos affichées, réutilisées si les écrans changent
    history: Vec<Vec<KeptPhoto>>, // fonds d'écran précédents, du plus ancien au plus récent
    excluded: HashSet<String>,    // photos que l'utilisateur ne veut plus voir
    preloaded: Vec<PreItem>,      // photos du prochain changement
    pre_gen: u64,                 // configuration pour laquelle elles ont été préparées
    pre_key: String,              // albums pour lesquels elles ont été préparées
}

pub struct Core {
    /// Sérialise les opérations longues (réseau, rendu, écran de verrouillage).
    run: Mutex<RunState>,
    st: Mutex<State>,
    dir: PathBuf,
    hooks: Hooks,
}

/// Opération longue, avec la configuration figée à son démarrage.
struct Job<'a> {
    c: &'a Core,
    cfg: Arc<Config>,
    client: Arc<Client>,
    gen: u64,
}

/// Image téléchargée, vérifiée par son en-tête mais pas encore décodée.
struct RawPhoto {
    asset: Asset,
    data: Vec<u8>,
    info: AssetInfo,
    has_info: bool,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct CacheMeta {
    asset: Asset,
    info: AssetInfo,
}

type Source<'a> = Box<dyn FnMut() -> Res<Photo> + 'a>;

fn read_ids(path: &Path) -> Vec<String> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Transforme un panic (décodeur, rendu) en erreur journalisée : l'appli est un binaire sans
/// console, un panic non récupéré la ferait disparaître sans message.
fn guard<T>(what: &str, f: impl FnOnce() -> Res<T>) -> Res<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = crate::util::panic_message(&*p);
            logf!("panic pendant {what} : {msg}");
            err(format!("erreur interne pendant {what} : {msg} (voir le journal)"))
        }
    }
}

impl Core {
    pub fn new(dir: &Path, hooks: Hooks) -> Core {
        let c = Core {
            run: Mutex::new(RunState { flip: false, fonts: None }),
            st: Mutex::new(State {
                cfg: None,
                client: None,
                gen: 0,
                albums: HashMap::new(),
                albums_checked: None,
                assets: None,
                recent: vec![],
                last: None,
                lock_err: None,
                last_photos: vec![],
                history: vec![],
                excluded: HashSet::new(),
                preloaded: vec![],
                pre_gen: 0,
                pre_key: String::new(),
            }),
            dir: dir.to_path_buf(),
            hooks,
        };
        {
            let mut st = lock(&c.st);
            // Historique des photos récentes et photos masquées : conservés d'un lancement à l'autre.
            let recent = read_ids(&c.recent_path());
            if recent.len() <= 500 {
                st.recent = recent;
            }
            st.excluded = read_ids(&c.excluded_path()).into_iter().collect();
        }
        let _ = std::fs::remove_dir_all(c.preload_dir()); // préchargement d'un lancement précédent : périmé
        c
    }

    fn recent_path(&self) -> PathBuf {
        self.dir.join("recent.json")
    }
    fn excluded_path(&self) -> PathBuf {
        self.dir.join("excluded.json")
    }
    pub fn preload_dir(&self) -> PathBuf {
        self.dir.join("preload")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.dir.join("cache")
    }

    pub fn set_config(&self, cfg: Config) {
        let old = {
            let mut st = lock(&self.st);
            let changed = match &st.cfg {
                Some(c) => c.server_url != cfg.server_url || c.api_key != cfg.api_key,
                None => true,
            };
            if changed {
                st.albums.clear(); // autre serveur ou autre compte : on repart de zéro
            }
            let old = st.client.replace(Arc::new(Client::new(&cfg)));
            st.cfg = Some(Arc::new(cfg));
            st.gen += 1;
            st.preloaded.clear();
            st.assets = None;
            st.albums_checked = None;
            old
        };
        if let Some(c) = old {
            c.close(); // les requêtes de l'ancienne configuration ne partent plus
        }
    }

    /// Dernière erreur de l'écran de verrouillage (None si OK).
    pub fn lock_error(&self) -> Option<String> {
        lock(&self.st).lock_err.clone()
    }

    /// Photo affichée (écran principal) et son adresse dans Immich.
    pub fn last(&self) -> Option<(Asset, String)> {
        let st = lock(&self.st);
        let (a, c) = (st.last.as_ref()?, st.client.as_ref()?);
        Some((a.clone(), c.web_url(&a.id)))
    }

    fn begin(&self) -> Res<Job<'_>> {
        let st = lock(&self.st);
        match (&st.cfg, &st.client) {
            (Some(cfg), Some(client)) => Ok(Job {
                c: self,
                cfg: cfg.clone(),
                client: client.clone(),
                gen: st.gen,
            }),
            _ => err("configuration incomplète : ouvre les paramètres"),
        }
    }

    // ---------- choix ----------

    /// Mélange les assets en repoussant à la fin ceux affichés récemment,
    /// et écarte ceux que l'utilisateur ne veut plus voir.
    fn order(&self, list: &[Asset]) -> Vec<Asset> {
        let st = lock(&self.st);
        let recent: HashSet<&String> = st.recent.iter().collect();
        let (mut fresh, mut old) = (vec![], vec![]);
        for a in list {
            if st.excluded.contains(&a.id) {
                continue;
            }
            if recent.contains(&a.id) {
                old.push(a.clone());
            } else {
                fresh.push(a.clone());
            }
        }
        fastrand::shuffle(&mut fresh);
        fastrand::shuffle(&mut old);
        fresh.extend(old);
        fresh
    }

    fn cached_ids(&self) -> Vec<String> {
        let mut items: Vec<(String, std::time::SystemTime)> = vec![];
        if let Ok(rd) = std::fs::read_dir(self.cache_dir()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let (Some(id), Ok(m)) = (name.strip_suffix(".json"), e.metadata()) {
                    items.push((id.to_string(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
                }
            }
        }
        items.sort_by(|a, b| b.1.cmp(&a.1));
        items.into_iter().map(|i| i.0).collect()
    }

    // ---------- opérations publiques ----------

    /// Choisit et applique de nouvelles images (une par écran si demandé).
    pub fn next(&self) -> Res<Outcome> {
        let mut run = lock(&self.run);
        guard("le changement d'image", || self.begin()?.next(&mut run))
    }

    /// Recalcule le fond d'écran avec les photos actuelles pour la configuration d'écrans
    /// courante (branchement, résolution…). S'il faut plus de photos qu'avant, renvoie NeedDraw :
    /// c'est à l'appelant de décider d'un nouveau tirage (pause, planification).
    pub fn reapply(&self) -> Res<Outcome> {
        let mut run = lock(&self.run);
        guard("le recalcul", || {
            let j = self.begin()?;
            let (mons, single, n) = j.targets();
            let last = lock(&self.st).last_photos.clone();
            if last.len() < n {
                return Err(Error::NeedDraw);
            }
            let (_, path) = j.apply_kept(&mut run, &last, &mons, single)?;
            logf!("écrans modifiés : fond d'écran recalculé ({n} photo(s))");
            j.lock_screen(&path);
            Ok(Outcome {
                asset: last[0].asset.clone(),
                offline: false,
                cause: None,
            })
        })
    }

    /// Réaffiche le fond d'écran précédent (jusqu'à HISTORY_DEPTH retours en arrière).
    pub fn previous(&self) -> Res<Outcome> {
        let mut run = lock(&self.run);
        guard("le retour à la photo précédente", || {
            let j = self.begin()?;
            let set = lock(&self.st).history.last().cloned().unwrap_or_default();
            if set.is_empty() {
                return Err(Error::NoPrevious);
            }
            let (mons, single, _) = j.targets();
            let (shown, path) = j.apply_kept(&mut run, &set, &mons, single)?;
            let first = shown[0].asset.clone();
            {
                let mut st = lock(&self.st);
                st.history.pop();
                st.last_photos = shown;
                st.last = Some(first.clone());
            }
            logf!("photo précédente : {} ({})", first.original_file_name, first.id);
            if let Some(f) = &self.hooks.after_apply {
                f(&j.cfg, &first, &j.client.web_url(&first.id));
            }
            j.lock_screen(&path);
            Ok(Outcome {
                asset: first,
                offline: false,
                cause: None,
            })
        })
    }

    /// Télécharge à l'avance les photos du prochain changement (et leurs informations) :
    /// le changement suivant n'a alors plus besoin du réseau — utile au déverrouillage du PC.
    pub fn preload(&self) -> Res<()> {
        let _run = lock(&self.run);
        guard("le préchargement", || {
            let j = self.begin()?;
            if !j.cfg.preload {
                return Ok(());
            }
            let (_, _, n) = j.targets();
            let key = j.album_key();
            let assets = j.refresh_albums()?;
            let dir = self.preload_dir();
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let mut next = j.raw_source(n, assets);
            let mut items = vec![];
            while items.len() < n {
                let mut r = match next() {
                    Ok(r) => r,
                    Err(e) if items.is_empty() => return Err(e),
                    Err(_) => break,
                };
                j.fetch_info(&mut r);
                let path = dir.join(format!("{}.img", r.asset.id));
                std::fs::write(&path, &r.data)?;
                items.push(PreItem {
                    asset: r.asset,
                    info: r.info,
                    has_info: r.has_info,
                    path,
                });
            }
            let count = items.len();
            let mut st = lock(&self.st);
            if st.gen == j.gen {
                st.preloaded = items;
                st.pre_gen = j.gen;
                st.pre_key = key;
            }
            drop(st);
            logf!("photo suivante préchargée ({count})");
            Ok(())
        })
    }

    /// Ajoute la photo affichée (celle de l'écran principal) à la liste des photos à ne plus
    /// afficher. La liste est locale (excluded.json) : rien n'est modifié dans Immich.
    pub fn exclude_current(&self) -> Res<Asset> {
        let (a, saved) = {
            let mut st = lock(&self.st);
            let a = match &st.last {
                Some(a) => a.clone(),
                None => return err("aucune photo affichée"),
            };
            st.excluded.insert(a.id.clone());
            // Elle ne doit pas revenir par « Photo précédente ».
            for set in st.history.iter_mut() {
                set.retain(|k| k.asset.id != a.id);
            }
            st.history.retain(|set| !set.is_empty());
            let saved = self.save_excluded(&st);
            (a, saved)
        };
        if is_uuid(&a.id) {
            let _ = std::fs::remove_file(self.cache_dir().join(format!("{}.jpg", a.id)));
            let _ = std::fs::remove_file(self.cache_dir().join(format!("{}.json", a.id)));
        }
        logf!("photo masquée : {} ({})", a.original_file_name, a.id);
        saved.map(|_| a)
    }

    fn save_excluded(&self, st: &State) -> Res<()> {
        let mut ids: Vec<&String> = st.excluded.iter().collect();
        ids.sort();
        std::fs::write(self.excluded_path(), serde_json::to_vec(&ids).unwrap_or_default())?;
        Ok(())
    }

    /// Nombre de photos masquées.
    pub fn excluded_count(&self) -> usize {
        lock(&self.st).excluded.len()
    }

    /// Réautorise toutes les photos masquées.
    pub fn clear_excluded(&self) -> Res<()> {
        let mut st = lock(&self.st);
        st.excluded.clear();
        self.save_excluded(&st)
    }

    /// Ajoute la photo affichée (écran principal) aux favoris Immich.
    pub fn favorite(&self) -> Res<Asset> {
        let (a, client) = {
            let st = lock(&self.st);
            match (&st.last, &st.client) {
                (Some(a), Some(c)) => (a.clone(), c.clone()),
                _ => return err("aucune photo affichée"),
            }
        };
        client.set_favorite(&a.id).map(|_| a)
    }
}

/// Réunit sans doublon les images des albums demandés.
fn merge_albums(albums: &HashMap<String, Arc<AlbumState>>, ids: &[String]) -> Vec<Asset> {
    let mut seen = HashSet::new();
    let mut all = vec![];
    for id in ids {
        if let Some(st) = albums.get(id) {
            for a in &st.assets {
                if seen.insert(a.id.clone()) {
                    all.push(a.clone());
                }
            }
        }
    }
    all
}

/// Photos préchargées en attente ; les fichiers non utilisés sont supprimés à la fin du tirage.
struct PreQueue(VecDeque<PreItem>);

impl Drop for PreQueue {
    fn drop(&mut self) {
        for it in &self.0 {
            let _ = std::fs::remove_file(&it.path);
        }
    }
}

impl<'a> Job<'a> {
    fn now(&self) -> Local {
        (self.c.hooks.now)(())
    }

    // ---------- albums (rechargement incrémental) ----------

    /// Interroge la liste des albums (une requête) et ne recharge le contenu que des albums dont
    /// la signature (nombre de photos, dates de modification) a changé.
    /// Renvoie la liste des images à utiliser pour ce tirage.
    fn refresh_albums(&self) -> Res<Vec<Asset>> {
        let c = self.c;
        let (loaded, fresh, mut albums) = {
            let st = lock(&c.st);
            let fresh = st.albums_checked.map(|t| t.elapsed() < ALBUM_CHECK_EVERY).unwrap_or(false);
            (st.assets.is_some(), fresh, st.albums.clone())
        };
        if loaded && fresh {
            return self.pick(&albums);
        }
        let sigs = match self.client.album_signatures() {
            Ok(s) => s,
            Err(e) if e.is_auth() || !loaded => return Err(e),
            Err(e) => {
                logf!("vérification des albums impossible, liste précédente conservée : {e}");
                return self.pick(&albums);
            }
        };
        let all_ids = self.cfg.all_albums();
        for id in &all_ids {
            let sig = sigs.get(id);
            if let (Some(st), Some(sig)) = (albums.get(id), sig) {
                if st.sig == *sig {
                    continue;
                }
            }
            match self.client.album_images(id) {
                Ok((name, assets)) => {
                    logf!("album {name:?} : {} images (rechargé)", assets.len());
                    albums.insert(
                        id.clone(),
                        Arc::new(AlbumState {
                            sig: sig.cloned().unwrap_or_default(),
                            assets,
                        }),
                    );
                }
                Err(Error::Unauthorized) => return Err(Error::Unauthorized),
                Err(e) => {
                    if sig.is_none() && albums.contains_key(id) {
                        // Ni listé ni lisible : album supprimé ou partage retiré.
                        logf!("album {id} inaccessible ({e}) : retiré du tirage");
                        albums.remove(id);
                    } else {
                        logf!("album {id} : {e}");
                    }
                }
            }
        }
        let total = merge_albums(&albums, &all_ids).len();
        {
            let mut st = lock(&c.st);
            if st.gen == self.gen {
                // sinon la configuration a changé entre-temps : résultat non mémorisé
                st.albums = albums.clone();
                st.albums_checked = Some(Instant::now());
                st.assets = (total > 0).then_some(total);
            }
        }
        self.pick(&albums)
    }

    /// Images des albums à utiliser maintenant : ceux des règles (saison, heure) qui
    /// s'appliquent ; à défaut, ou s'ils sont vides, les albums habituels.
    fn pick(&self, albums: &HashMap<String, Arc<AlbumState>>) -> Res<Vec<Asset>> {
        for ids in [self.cfg.albums_at(&self.now()), self.cfg.albums.clone(), self.cfg.all_albums()] {
            let list = merge_albums(albums, &ids);
            if !list.is_empty() {
                return Ok(list);
            }
        }
        err("aucune image trouvée dans les albums configurés")
    }

    /// Identifie les albums utilisés à cet instant (validité du préchargement).
    fn album_key(&self) -> String {
        self.cfg.albums_at(&self.now()).join(",")
    }

    // ---------- sources de photos ----------

    /// Source d'images téléchargées depuis Immich, une par appel. Rien n'est décodé ici : les
    /// portraits écartés (landscape_only) ne sont ni décodés ni mis en cache ; au plus n sont
    /// gardés compressés, en secours s'il manque des paysages.
    fn raw_source(&self, n: usize, assets: Vec<Asset>) -> impl FnMut() -> Res<RawPhoto> + '_ {
        let mut order = self.c.order(&assets).into_iter();
        let mut portraits: VecDeque<RawPhoto> = VecDeque::new();
        let mut last_err: Option<Error> = None;
        let mut tries = 0;
        move || {
            while tries < 15 * n {
                let Some(a) = order.next() else { break };
                tries += 1;
                let data = match self.client.download(&a.id, &self.cfg.image_size) {
                    Ok(d) => d,
                    Err(Error::Unauthorized) => return Err(Error::Unauthorized),
                    Err(e) => {
                        logf!("{e}");
                        last_err = Some(e);
                        continue;
                    }
                };
                let meta: Meta = match probe(&data) {
                    Ok(m) => m,
                    Err(e) => {
                        logf!("{} ({}) : {e}", a.id, a.original_file_name);
                        continue;
                    }
                };
                let r = RawPhoto {
                    asset: a,
                    data,
                    info: AssetInfo::default(),
                    has_info: false,
                };
                if self.cfg.landscape_only && meta.portrait() {
                    if portraits.len() < n {
                        portraits.push_back(r);
                    }
                    continue;
                }
                return Ok(r);
            }
            if let Some(r) = portraits.pop_front() {
                return Ok(r); // pas assez de paysages : on complète
            }
            match last_err.clone() {
                Some(e) => Err(e),
                None => err(format!("aucune image utilisable après {tries} essais (voir le journal)")),
            }
        }
    }

    /// Complète la photo avec le lieu et la date (si la légende est demandée).
    fn fetch_info(&self, r: &mut RawPhoto) {
        if r.has_info || !self.cfg.want_info() {
            return;
        }
        match self.client.asset_info(&r.asset.id) {
            Ok(info) => {
                r.info = info;
                r.has_info = true;
            }
            Err(e) => logf!("infos {} : {e}", r.asset.id),
        }
    }

    /// Décode l'image (sRGB, redressée) et la complète de ses informations.
    fn build(&self, mut r: RawPhoto) -> Res<Photo> {
        let img = safe_decode(&r.data)?;
        r.data = Vec::new(); // les octets compressés ne servent plus
        self.fetch_info(&mut r);
        Ok(Photo {
            asset: r.asset,
            img,
            info: r.info,
            jpg: None,
        })
    }

    /// Source de photos téléchargées depuis Immich et décodées, une par appel :
    /// seule la photo demandée est en mémoire.
    fn online(&self, n: usize, assets: Vec<Asset>) -> impl FnMut() -> Res<Photo> + '_ {
        let mut next = self.raw_source(n, assets);
        move || loop {
            let r = next()?;
            let id = r.asset.id.clone();
            match self.build(r) {
                Ok(p) => return Ok(p),
                Err(e) => logf!("{id} : {e}"),
            }
        }
    }

    /// Photos préchargées si elles valent encore pour ce tirage (même configuration,
    /// mêmes albums, photos non exclues depuis).
    fn take_preloaded(&self) -> PreQueue {
        let key = self.album_key();
        let mut st = lock(&self.c.st);
        let ok = st.pre_gen == self.gen && st.pre_key == key;
        let items = std::mem::take(&mut st.preloaded);
        let mut keep = VecDeque::new();
        for it in items {
            if ok && !st.excluded.contains(&it.asset.id) {
                keep.push_back(it);
            } else {
                let _ = std::fs::remove_file(&it.path);
            }
        }
        PreQueue(keep)
    }

    /// Décode une photo préchargée ; son fichier est supprimé.
    fn from_preload(&self, it: PreItem) -> Res<Photo> {
        let data = std::fs::read(&it.path);
        let _ = std::fs::remove_file(&it.path);
        let data = data?;
        probe(&data)?;
        self.build(RawPhoto {
            asset: it.asset,
            data,
            info: it.info,
            has_info: it.has_info,
        })
    }

    /// Source de photos tirées du cache local (Immich injoignable).
    /// Avec landscape_only, les portraits du cache ne servent qu'en dernier recours.
    fn cached(&self) -> impl FnMut() -> Res<Photo> + '_ {
        let pool: Vec<Asset> = self
            .c
            .cached_ids()
            .into_iter()
            .map(|id| Asset { id, ..Default::default() })
            .collect();
        let mut order = self.c.order(&pool).into_iter();
        let mut portraits: VecDeque<String> = VecDeque::new();
        let dir = self.c.cache_dir();
        let load = move |id: &str, skip_portrait: bool| -> Result<Photo, bool> {
            // Err(true) : portrait mis de côté ; Err(false) : fichier inutilisable.
            let meta = std::fs::read(dir.join(format!("{id}.json")))
                .ok()
                .and_then(|b| serde_json::from_slice::<CacheMeta>(&b).ok());
            let (Some(meta), Ok(jpg)) = (meta, std::fs::read(dir.join(format!("{id}.jpg")))) else {
                return Err(false);
            };
            if skip_portrait {
                if let Some((w, h)) = jpeg_dims(&jpg) {
                    if w <= h {
                        return Err(true);
                    }
                }
            }
            let img = decode_jpeg(&jpg).map_err(|_| false)?;
            Ok(Photo {
                asset: meta.asset,
                img,
                info: meta.info,
                jpg: Some(Arc::new(jpg)),
            })
        };
        let landscape_only = self.cfg.landscape_only;
        move || {
            for a in order.by_ref() {
                match load(&a.id, landscape_only) {
                    Ok(p) => return Ok(p),
                    Err(true) => portraits.push_back(a.id),
                    Err(false) => {}
                }
            }
            while let Some(id) = portraits.pop_front() {
                if let Ok(p) = load(&id, false) {
                    return Ok(p);
                }
            }
            err("cache hors ligne vide")
        }
    }

    // ---------- cache hors ligne ----------

    /// Garde une copie (redimensionnée à 3840 px max) pour l'affichage hors ligne.
    fn save_to_cache(&self, k: &KeptPhoto) {
        if self.cfg.offline_cache <= 0 || !is_uuid(&k.asset.id) {
            return;
        }
        let dir = self.c.cache_dir();
        let meta = serde_json::to_vec(&CacheMeta {
            asset: k.asset.clone(),
            info: k.info.clone(),
        })
        .unwrap_or_default();
        let res = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(dir.join(format!("{}.jpg", k.asset.id)), &*k.jpeg))
            .and_then(|_| std::fs::write(dir.join(format!("{}.json", k.asset.id)), meta));
        if let Err(e) = res {
            logf!("cache hors ligne : {e}");
        }
        for id in self.c.cached_ids().into_iter().skip(self.cfg.offline_cache as usize) {
            let _ = std::fs::remove_file(dir.join(format!("{id}.jpg")));
            let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        }
    }

    // ---------- écrans ----------

    /// Nombre de photos à afficher sur cet écran (2 si très large et option active).
    fn panels(&self, sc: Screen) -> usize {
        if self.cfg.split_wide && sc.h > 0 && sc.w as f64 / sc.h as f64 >= SPLIT_RATIO {
            2
        } else {
            1
        }
    }

    /// Écrans à couvrir et nombre de photos nécessaires (une par écran, deux pour un écran
    /// très large découpé en deux panneaux).
    fn targets(&self) -> (Option<Vec<Monitor>>, Screen, usize) {
        let h = &self.c.hooks;
        if self.cfg.multi_monitor == "different" && h.set_monitor_wallpapers.is_some() {
            if let Some(list) = h.monitors.as_ref().map(|f| f(())) {
                if list.len() > 1 {
                    let n = list.iter().map(|m| self.panels(m.screen)).sum();
                    return (Some(list), Screen::default(), n);
                }
            }
        }
        let single = h.screen.as_ref().map(|f| f(&self.cfg.style)).unwrap_or_default();
        (None, single, self.panels(single))
    }

    fn next_name(&self, run: &RunState, prefix: &str) -> PathBuf {
        // On alterne entre deux fichiers : Windows ne rafraîchit pas toujours si le chemin ne change pas.
        self.c.dir.join(format!("{prefix}_{}.jpg", if run.flip { "b" } else { "a" }))
    }

    fn apply_to(
        &self,
        run: &mut RunState,
        next: &mut dyn FnMut() -> Res<Photo>,
        mons: &Option<Vec<Monitor>>,
        single: Screen,
    ) -> Res<PathBuf> {
        match mons {
            None => self.apply_single(run, next, single),
            Some(m) => self.apply_multi(run, next, m),
        }
    }

    fn take(next: &mut dyn FnMut() -> Res<Photo>, n: usize) -> Res<Vec<Photo>> {
        (0..n).map(|_| next()).collect()
    }

    fn apply_single(&self, run: &mut RunState, next: &mut dyn FnMut() -> Res<Photo>, sc: Screen) -> Res<PathBuf> {
        let photos = Self::take(next, self.panels(sc))?;
        let mut style = self.cfg.style.as_str();
        let jpg: Arc<Vec<u8>> = if (self.cfg.want_info() || photos.len() > 1) && sc.w > 0 {
            // L'image fait déjà la taille de l'écran. En « Étendre », elle fait celle du bureau
            // virtuel : il faut garder ce style pour qu'elle s'étale sur tous les écrans.
            if style != "span" {
                style = "fill";
            }
            Arc::new(self.render(run, &photos, sc)?)
        } else {
            let p = &photos[0];
            match &p.jpg {
                // Pas de réduction : la version gardée est déjà l'image complète.
                Some(j) if p.img.width().max(p.img.height()) <= KEPT_MAX_SIDE => j.clone(),
                _ => Arc::new(encode_jpeg(&p.img)?),
            }
        };
        drop(photos);
        let path = self.next_name(run, "wallpaper");
        run.flip = !run.flip;
        std::fs::write(&path, &*jpg)?;
        (self.c.hooks.set_wallpaper)(&path, style)?;
        Ok(path)
    }

    /// Rend les photos de chaque écran à sa taille exacte puis les applique.
    /// Les photos d'un écran sont libérées avant de traiter l'écran suivant.
    fn apply_multi(&self, run: &mut RunState, next: &mut dyn FnMut() -> Res<Photo>, mons: &[Monitor]) -> Res<PathBuf> {
        let mut paths = vec![];
        for (i, m) in mons.iter().enumerate() {
            let photos = Self::take(next, self.panels(m.screen))?;
            let jpg = self.render(run, &photos, m.screen)?;
            drop(photos);
            let path = self.next_name(run, &format!("wallpaper_ecran{}", i + 1));
            std::fs::write(&path, jpg)?;
            paths.push((m.id.clone(), path));
        }
        run.flip = !run.flip;
        let primary = paths[0].1.clone();
        match &self.c.hooks.set_monitor_wallpapers {
            Some(f) => f(&paths, "fill")?,
            None => return err("application par écran indisponible"),
        }
        Ok(primary)
    }

    /// Image d'un écran : une photo, ou deux côte à côte (écran très large), séparées par une
    /// fine bande sombre, chacune avec sa légende vers le bord extérieur.
    fn render(&self, run: &mut RunState, photos: &[Photo], sc: Screen) -> Res<Vec<u8>> {
        if sc.w < 320 || sc.h < 240 {
            return err(format!("taille d'écran inattendue {}x{}", sc.w, sc.h));
        }
        if photos.len() == 1 {
            return encode_jpeg(&self.render_panel(run, &photos[0], sc, &self.cfg.caption_position));
        }
        let mut dst = RgbImage::new(sc.w as u32, sc.h as u32);
        let count = photos.len() as i32;
        let gap = (sc.h / 270).max(2); // ≈ 4 px en 1080p, 5 px en 1440p
        let pw = (sc.w - gap * (count - 1)) / count;
        let mut x = 0;
        for (i, p) in photos.iter().enumerate() {
            let last = i == photos.len() - 1;
            let w = if last { sc.w - x } else { pw };
            let mut pos = self.cfg.caption_position.clone();
            if i == 0 && photos.len() == 2 {
                pos = pos.replacen("right", "left", 1); // panneau gauche : légende côté gauche (bord extérieur)
            } else if last {
                pos = pos.replacen("left", "right", 1);
            }
            let panel = self.render_panel(
                run,
                p,
                Screen {
                    w,
                    h: sc.h,
                    bottom_inset: sc.bottom_inset,
                },
                &pos,
            );
            blit(&mut dst, &panel, x, 0);
            x += w + gap;
        }
        encode_jpeg(&dst)
    }

    fn render_panel(&self, run: &mut RunState, p: &Photo, sc: Screen, pos: &str) -> RgbImage {
        let mut dst = compose(&p.img, sc, &self.cfg.style);
        if self.cfg.want_info() {
            let (place, date) = caption_parts(&p.info, self.cfg.show_place, self.cfg.show_date);
            let fonts = run.fonts.get_or_insert_with(load_fonts);
            draw_caption(&mut dst, fonts.as_ref(), &place, &date, pos, sc.bottom_inset);
        }
        dst
    }

    /// Applique des photos déjà mémorisées (compressées), en les répétant s'il y a plus
    /// d'emplacements que de photos. Renvoie les photos dans l'ordre d'affichage.
    fn apply_kept(
        &self,
        run: &mut RunState,
        set: &[KeptPhoto],
        mons: &Option<Vec<Monitor>>,
        single: Screen,
    ) -> Res<(Vec<KeptPhoto>, PathBuf)> {
        let mut shown: Vec<KeptPhoto> = vec![];
        let mut next = || -> Res<Photo> {
            let k = set[shown.len() % set.len()].clone();
            let img = decode_jpeg(&k.jpeg)?;
            shown.push(k.clone());
            Ok(Photo {
                asset: k.asset,
                img,
                info: k.info,
                jpg: Some(k.jpeg),
            })
        };
        let path = self.apply_to(run, &mut next, mons, single)?;
        Ok((shown, path))
    }

    /// Applique la photo de l'écran principal à l'écran de verrouillage ;
    /// l'erreur est journalisée et mémorisée, mais non bloquante.
    fn lock_screen(&self, path: &Path) {
        let Some(f) = &self.c.hooks.set_lock_screen else { return };
        if !self.cfg.lock_screen {
            return;
        }
        let res = f(path);
        if let Err(e) = &res {
            logf!("écran de verrouillage : {e}");
        }
        lock(&self.c.st).lock_err = res.err().map(|e| e.to_string());
    }

    // ---------- tirage ----------

    fn next(&self, run: &mut RunState) -> Res<Outcome> {
        let c = self.c;
        let (mons, single, n) = self.targets();

        // Photos préchargées d'abord (aucun accès réseau), puis téléchargement à la demande.
        let mut pre = self.take_preloaded();
        let mut online: Option<Source<'_>> = None;
        let src: Source<'_> = Box::new(move || {
            while let Some(it) = pre.0.pop_front() {
                let id = it.asset.id.clone();
                match self.from_preload(it) {
                    Ok(p) => return Ok(p),
                    Err(e) => logf!("photo préchargée {id} inutilisable : {e}"),
                }
            }
            if online.is_none() {
                online = Some(Box::new(self.online(n, self.refresh_albums()?)));
            }
            (online.as_mut().unwrap())()
        });

        let mut f = Feeder::new(self, src, true);
        let mut out = Outcome {
            asset: Asset::default(),
            offline: false,
            cause: None,
        };
        match f.fetch() {
            Ok(p) => f.head = Some(p),
            Err(e) => {
                let mut off = Feeder::new(self, Box::new(self.cached()), false);
                match off.fetch() {
                    Ok(p) => off.head = Some(p),
                    Err(_) => return Err(e),
                }
                logf!("Immich injoignable ({e}) : photo du cache hors ligne");
                f = off;
                out.offline = true;
                out.cause = Some(e);
            }
        }

        let primary = self.apply_to(run, &mut || f.next(), &mons, single)?;
        let first = f.shown[0].asset.clone();

        let offline_pool = c.cached_ids().len();
        let recent = {
            let mut st = lock(&c.st);
            let pool = if out.offline { offline_pool } else { st.assets.unwrap_or(0) };
            if !st.last_photos.is_empty() {
                // pour « Photo précédente »
                let prev = std::mem::take(&mut st.last_photos);
                st.history.push(prev);
                let extra = st.history.len().saturating_sub(HISTORY_DEPTH);
                st.history.drain(..extra);
            }
            st.last_photos = f.shown.clone();
            let limit = (pool / 2).min(100).max(n);
            for k in &f.distinct {
                st.recent.push(k.asset.id.clone());
                logf!("fond d'écran : {} ({})", k.asset.original_file_name, k.asset.id);
            }
            let extra = st.recent.len().saturating_sub(limit);
            st.recent.drain(..extra);
            st.last = Some(first.clone());
            serde_json::to_vec(&st.recent).unwrap_or_default()
        };
        if let Err(e) = std::fs::write(c.recent_path(), recent) {
            logf!("historique des photos : {e}");
        }

        if let Some(h) = &c.hooks.after_apply {
            h(&self.cfg, &first, &self.client.web_url(&first.id));
        }
        self.lock_screen(&primary);
        out.asset = first;
        Ok(out)
    }
}

/// Fournit les photos une par une au rendu : chaque écran est traité entièrement
/// (téléchargement → décodage → rendu → écriture) avant de passer au suivant, et seules les
/// versions compressées sont conservées entre deux écrans.
struct Feeder<'a> {
    job: &'a Job<'a>,
    src: Source<'a>,
    /// Photos venant d'Immich : à copier dans le cache hors ligne.
    cache: bool,
    /// Photo déjà obtenue, à servir en premier.
    head: Option<Photo>,
    /// Photos différentes obtenues.
    distinct: Vec<KeptPhoto>,
    /// Photos dans l'ordre d'affichage (répétitions comprises).
    shown: Vec<KeptPhoto>,
    done: bool,
}

impl<'a> Feeder<'a> {
    fn new(job: &'a Job<'a>, src: Source<'a>, cache: bool) -> Self {
        Feeder {
            job,
            src,
            cache,
            head: None,
            distinct: vec![],
            shown: vec![],
            done: false,
        }
    }

    fn kept(p: &Photo) -> KeptPhoto {
        KeptPhoto {
            asset: p.asset.clone(),
            info: p.info.clone(),
            jpeg: p.jpg.clone().unwrap_or_default(),
        }
    }

    fn next(&mut self) -> Res<Photo> {
        if let Some(p) = self.head.take() {
            self.shown.push(Self::kept(&p));
            return Ok(p);
        }
        if !self.done {
            match self.fetch() {
                Ok(p) => {
                    self.shown.push(Self::kept(&p));
                    return Ok(p);
                }
                Err(e) => {
                    self.done = true;
                    if self.distinct.is_empty() {
                        return Err(e);
                    }
                    logf!("moins de photos que d'emplacements ({e}) : réutilisation");
                }
            }
        }
        // Moins de photos que d'écrans : on réutilise celles déjà obtenues.
        let k = self.distinct[(self.shown.len() - self.distinct.len()) % self.distinct.len()].clone();
        let img = decode_jpeg(&k.jpeg)?;
        self.shown.push(k.clone());
        Ok(Photo {
            asset: k.asset,
            img,
            info: k.info,
            jpg: Some(k.jpeg),
        })
    }

    /// Tire une nouvelle photo de la source et calcule (une seule fois) sa version réduite,
    /// partagée entre le cache hors ligne et les photos mémorisées.
    fn fetch(&mut self) -> Res<Photo> {
        let mut p = (self.src)()?;
        let fresh = p.jpg.is_none();
        if fresh {
            p.jpg = Some(Arc::new(shrink_jpeg(&p.img)?));
        }
        let k = Self::kept(&p);
        if fresh && self.cache {
            self.job.save_to_cache(&k);
        }
        self.distinct.push(k);
        Ok(p)
    }
}

#[cfg(test)]
impl Core {
    /// Force une nouvelle vérification des albums au prochain tirage.
    pub fn force_album_check(&self) {
        lock(&self.st).albums_checked = None;
    }
    pub fn assets_count(&self) -> usize {
        lock(&self.st).assets.unwrap_or(0)
    }
    pub fn recent(&self) -> Vec<String> {
        lock(&self.st).recent.clone()
    }
    pub fn clear_recent(&self) {
        lock(&self.st).recent.clear();
    }
    pub fn last_photos(&self) -> Vec<KeptPhoto> {
        lock(&self.st).last_photos.clone()
    }
    pub fn history_len(&self) -> usize {
        lock(&self.st).history.len()
    }
    pub fn history_contains(&self, id: &str) -> bool {
        lock(&self.st).history.iter().flatten().any(|k| k.asset.id == id)
    }
    pub fn preloaded_ids(&self) -> Vec<String> {
        lock(&self.st).preloaded.iter().map(|p| p.asset.id.clone()).collect()
    }
    pub fn cached_count(&self) -> usize {
        self.cached_ids().len()
    }
}
