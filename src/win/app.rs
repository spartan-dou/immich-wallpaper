//! Application : icône de la barre système, menu, planification, événements système.
//!
//! Une fenêtre cachée de premier niveau reçoit tout : clics sur l'icône, minuteurs de la
//! planification, changements d'écran, déverrouillage, sortie de veille (les messages diffusés
//! comme WM_DISPLAYCHANGE ne parviennent qu'aux fenêtres de premier niveau).

use crate::config::{key_stored_plain, load_config, read_config, save_config, write_default_config, Config};
use crate::core::{Core, Hooks};
use crate::immich::Asset;
use crate::logf;
use crate::settings::{Settings, SettingsHooks};
use crate::util::{lock, now_local, truncate, Error};
use crate::win::context::blocked;
use crate::win::install::{has_arg, maybe_install, uninstall, write_pid};
use crate::win::monitors::{list_monitors, set_monitor_wallpapers};
use crate::win::platform::{self, hidden_cmd, shell_open, wide};
use crate::win::settings_window;
use crate::win::updater::{self, PendingUpdate};
use crate::{is_portable, VERSION};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_POKE: u32 = WM_APP + 2;
const WM_APP_SETTINGS: u32 = WM_APP + 3;
const TIMER_SCHED: usize = 1;
const TIMER_EVENTS: usize = 2;

const EV_DISPLAY: u8 = 1;
const EV_UNLOCK: u8 = 2;

// Identifiants des entrées du menu.
const M_NEXT: usize = 1;
const M_PREV: usize = 2;
const M_OPEN: usize = 3;
const M_FAV: usize = 4;
const M_HIDE: usize = 5;
const M_SETTINGS: usize = 6;
const M_PAUSE: usize = 7;
const M_AUTO: usize = 8;
const M_LOG: usize = 9;
const M_UPDATE: usize = 10;
const M_UNINSTALL: usize = 11;
const M_QUIT: usize = 12;

static ICON_ICO: &[u8] = include_bytes!("../../res/icon.ico");
static APP: OnceLock<Arc<App>> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

struct AppState {
    cfg: Option<Arc<Config>>,
    next: SystemTime,
    paused: bool,
    status: String,
    /// Échecs consécutifs (temporisation croissante).
    failures: u32,
    /// Pourquoi le changement automatique attend (batterie, plein écran…).
    hold: Option<&'static str>,
    pending: Option<PendingUpdate>,
    has_photo: bool,
    /// Version portable : nouvelle version annoncée dans le menu (installée sur clic seulement).
    offered: Option<String>,
    /// Événement système en attente (regroupement sur 3 s).
    event: u8,
}

pub struct App {
    cfg_path: PathBuf,
    log_path: PathBuf,
    core: Core,
    settings: OnceLock<Arc<Settings>>,
    st: Mutex<AppState>,
    busy: AtomicBool,
    pub updating: AtomicBool,
    hwnd: AtomicIsize,
    icon: AtomicIsize,
    tray_added: AtomicBool,
    selftest: bool,
}

/// Délai avant un nouvel essai après n échecs consécutifs (2, 4, 8… 30 min).
pub fn retry_delay(n: u32) -> Duration {
    let mut d = 2u64;
    for _ in 1..n {
        if d >= 30 {
            break;
        }
        d *= 2;
    }
    Duration::from_secs(d.min(30) * 60)
}

/// Remet le drapeau « occupé » à zéro en sortie de portée, même en cas de panic.
struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl App {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.load(Ordering::SeqCst) as *mut _)
    }

    /// Lance f dans un thread ; un panic y est journalisé au lieu de faire disparaître l'appli.
    fn spawn(self: &Arc<Self>, what: &'static str, f: impl FnOnce(&Arc<App>) + Send + 'static) {
        let app = self.clone();
        std::thread::spawn(move || {
            if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&app))) {
                logf!("panic ({what}) : {}", crate::util::panic_message(&*p));
            }
        });
    }

    /// Demande à la planification de recalculer sa prochaine échéance.
    fn poke(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_APP_POKE, WPARAM(0), LPARAM(0));
        }
    }

    pub fn quit(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    pub fn set_pending(&self, p: PendingUpdate) {
        lock(&self.st).pending = Some(p);
    }

    /// Annonce une nouvelle version dans le menu de l'icône (version portable).
    pub fn offer_update(&self, version: &str) {
        lock(&self.st).offered = Some(version.to_string());
    }

    pub fn auto_update(&self) -> bool {
        lock(&self.st).cfg.as_ref().map(|c| c.auto_update).unwrap_or(true)
    }

    // ---------- icône de la barre système ----------

    fn tray(&self, action: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE, tip: &str) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd(),
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: HICON(self.icon.load(Ordering::SeqCst) as *mut _),
            ..Default::default()
        };
        let t = wide(&truncate(tip, 120));
        let n = t.len().min(nid.szTip.len());
        nid.szTip[..n].copy_from_slice(&t[..n]);
        nid.szTip[nid.szTip.len() - 1] = 0;
        unsafe {
            if Shell_NotifyIconW(action, &nid).as_bool() && action == NIM_ADD {
                self.tray_added.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn set_status(&self, s: &str) {
        lock(&self.st).status = s.to_string();
        self.tray(NIM_MODIFY, &format!("Immich Wallpaper — {s}"));
    }

    fn get_status(&self) -> String {
        let st = lock(&self.st);
        let mut s = st.status.clone();
        if st.paused {
            s += " · en pause";
        } else if let Some(h) = st.hold {
            s += &format!(" · en attente ({h})");
        } else if st.cfg.is_some() {
            // Heure locale de la prochaine échéance.
            let wait = st.next.duration_since(SystemTime::now()).unwrap_or_default().as_secs();
            let now = now_local();
            let minutes = ((now.hour * 3600 + now.minute * 60 + now.second) as u64 + wait) / 60 % 1440;
            s += &format!(" · prochaine à {:02}:{:02}", minutes / 60, minutes % 60);
        }
        s
    }

    // ---------- configuration ----------

    /// Appelé au démarrage et à chaque enregistrement depuis la fenêtre de réglages.
    fn apply_config(&self, cfg: Config, startup: bool) {
        logf!(
            "config chargée : {}, {} album(s), toutes les {} min",
            cfg.server_url,
            cfg.albums.len(),
            cfg.interval_minutes
        );
        self.core.set_config(cfg.clone());
        {
            let mut st = lock(&self.st);
            st.next = if startup && !cfg.change_on_start {
                SystemTime::now() + Duration::from_secs(cfg.interval_minutes as u64 * 60)
            } else {
                SystemTime::now() // nouvelle config → on applique tout de suite
            };
            st.failures = 0;
            st.cfg = Some(Arc::new(cfg));
        }
        self.poke();
    }

    fn cfg(&self) -> Option<Arc<Config>> {
        lock(&self.st).cfg.clone()
    }

    // ---------- actions ----------

    fn change(self: &Arc<Self>) -> Result<Asset, String> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("un changement est déjà en cours".into());
        }
        let _busy = Busy(&self.busy);
        self.set_status("Téléchargement…");
        let res = self.core.next();
        {
            let mut st = lock(&self.st);
            let interval = Duration::from_secs(st.cfg.as_ref().map(|c| c.interval_minutes as u64).unwrap_or(60) * 60);
            let wait = match &res {
                Ok(_) => {
                    st.failures = 0;
                    st.hold = None;
                    st.has_photo = true;
                    interval
                }
                Err(e) if e.is_auth() => Duration::from_secs(30 * 60),
                // Réseau pas prêt (démarrage, VPN…) : nouvel essai bientôt, puis de plus en plus espacé.
                Err(_) => {
                    st.failures += 1;
                    retry_delay(st.failures)
                }
            };
            st.next = SystemTime::now() + wait;
        }
        self.poke();
        let out = match res {
            Ok(o) => o,
            Err(e) => {
                logf!("erreur : {e}");
                self.set_status(&format!("Erreur : {e}"));
                return Err(e.to_string());
            }
        };
        let name = &out.asset.original_file_name;
        let mut st = name.clone();
        if out.offline {
            st = if out.cause.as_ref().map(|c| c.is_auth()).unwrap_or(false) {
                format!("Token refusé, photo du cache : {name}")
            } else {
                format!("Hors ligne (photo du cache) : {name}")
            };
        }
        if self.core.lock_error().is_some() {
            st += " (écran de verrouillage : échec, voir le journal)";
        }
        self.set_status(&st);
        if !out.offline {
            self.spawn("préchargement", |a| a.preload()); // la photo suivante est prête avant le prochain changement
        }
        Ok(out.asset)
    }

    /// Prépare la photo suivante en arrière-plan, sauf si le contexte s'y oppose.
    fn preload(&self) {
        let Some(cfg) = self.cfg() else { return };
        if !cfg.preload || blocked(&cfg, false).is_some() {
            return;
        }
        if let Err(e) = self.core.preload() {
            logf!("préchargement : {e}");
        }
    }

    /// Réaffiche le fond d'écran précédent et repousse le prochain changement.
    fn previous(&self) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let _busy = Busy(&self.busy);
        match self.core.previous() {
            Err(e) => {
                if e != Error::NoPrevious {
                    logf!("photo précédente : {e}");
                }
                self.set_status(&format!("Photo précédente : {e}"));
            }
            Ok(out) => {
                {
                    let mut st = lock(&self.st);
                    if let Some(c) = st.cfg.clone() {
                        st.next = SystemTime::now() + Duration::from_secs(c.interval_minutes as u64 * 60);
                    }
                }
                self.poke();
                self.set_status(&format!("Photo précédente : {}", out.asset.original_file_name));
            }
        }
    }

    /// Ajoute la photo affichée aux favoris Immich.
    fn favorite(&self) {
        match self.core.favorite() {
            Ok(a) => {
                logf!("ajoutée aux favoris Immich : {} ({})", a.original_file_name, a.id);
                self.set_status(&format!("Ajoutée aux favoris Immich : {}", a.original_file_name));
            }
            Err(e) => {
                logf!("ajout aux favoris : {e}");
                self.set_status(&format!("Favori impossible : {e}"));
            }
        }
    }

    /// Masque définitivement la photo affichée (liste locale) et passe à la suivante.
    fn hide(self: &Arc<Self>) {
        match self.core.exclude_current() {
            Ok(_) => {
                let _ = self.change();
            }
            Err(e) => {
                logf!("photo masquée : {e}");
                self.set_status(&format!("Impossible de masquer la photo : {e}"));
            }
        }
    }

    /// Recalcule le fond d'écran (mêmes photos) pour la nouvelle configuration d'écrans.
    /// S'il faut plus de photos que celles mémorisées (écran ajouté, aucune photo encore affichée),
    /// un nouveau tirage passe par change() — statut et échéance à jour — sauf si l'appli est en pause.
    fn reapply(self: &Arc<Self>) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let res = self.core.reapply();
        self.busy.store(false, Ordering::SeqCst);
        match res {
            Ok(_) => {}
            Err(Error::NeedDraw) => {
                if lock(&self.st).paused {
                    logf!("écrans modifiés : nouveau tirage nécessaire, ignoré (en pause)");
                } else {
                    logf!("écrans modifiés : nouveau tirage nécessaire");
                    let _ = self.change();
                }
            }
            Err(e) => logf!("recalcul après changement d'écran : {e}"),
        }
    }

    // ---------- planification (thread de la fenêtre) ----------

    /// Un minuteur réarmé sur la prochaine échéance. Plafonné à 1 min : l'horloge des minuteurs
    /// peut ne pas avancer pendant la veille du PC, on recompare donc régulièrement à l'heure réelle.
    fn check(self: &Arc<Self>) {
        let (cfg, active, wait) = {
            let st = lock(&self.st);
            (
                st.cfg.clone(),
                st.cfg.is_some() && !st.paused,
                st.next.duration_since(SystemTime::now()).unwrap_or_default(),
            )
        };
        let mut hold = None;
        let mut wait = wait.min(Duration::from_secs(60));
        if !active {
            wait = Duration::from_secs(60);
        } else if wait.is_zero() {
            // Échéance atteinte : on attend si le contexte s'y oppose (réévalué chaque minute).
            hold = cfg.as_ref().and_then(|c| blocked(c, true));
            if hold.is_some() {
                wait = Duration::from_secs(60);
            } else {
                self.spawn("changement d'image", |a| {
                    let _ = a.change();
                });
                wait = Duration::from_secs(10); // change() réveille la boucle dès qu'il a fixé la suite
            }
        }
        lock(&self.st).hold = hold;
        unsafe {
            SetTimer(Some(self.hwnd()), TIMER_SCHED, wait.as_millis().max(50) as u32, None);
        }
    }

    /// Les événements système arrivent souvent en rafale (plusieurs WM_DISPLAYCHANGE,
    /// déverrouillage + écran rallumé) : on attend 3 s de calme, et un déverrouillage
    /// l'emporte sur un simple changement d'écran.
    fn system_event(&self, ev: u8) {
        {
            let mut st = lock(&self.st);
            st.event = st.event.max(ev);
        }
        unsafe {
            SetTimer(Some(self.hwnd()), TIMER_EVENTS, 3000, None);
        }
    }

    fn fire_event(self: &Arc<Self>) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), TIMER_EVENTS);
        }
        let (cfg, paused, ev) = {
            let mut st = lock(&self.st);
            (st.cfg.clone(), st.paused, std::mem::take(&mut st.event))
        };
        let Some(cfg) = cfg else { return };
        if ev == EV_UNLOCK && cfg.change_on_unlock && !paused && blocked(&cfg, true).is_none() {
            logf!("session déverrouillée : nouvelle image");
            self.spawn("changement d'image", |a| {
                let _ = a.change();
            });
        } else {
            self.spawn("recalcul après changement d'écran", |a| a.reapply());
        }
        self.poke(); // sortie de veille possible : l'échéance est peut-être dépassée
    }

    // ---------- menu ----------

    fn build_menu(&self) -> HMENU {
        let (status, paused, has_photo, offered) = {
            let st = lock(&self.st);
            (st.status.clone(), st.paused, st.has_photo, st.offered.clone())
        };
        unsafe {
            let m = CreatePopupMenu().unwrap_or_default();
            let item = |id: usize, text: &str, flags: MENU_ITEM_FLAGS| {
                let t = wide(text);
                let _ = AppendMenuW(m, MF_STRING | flags, id, PCWSTR(t.as_ptr()));
            };
            let sep = || {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            };
            let photo = if has_photo { MF_ENABLED } else { MF_GRAYED };
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            item(0, &truncate(&status, 60), MF_GRAYED);
            sep();
            item(M_NEXT, "Image suivante", MF_ENABLED);
            item(M_PREV, "Photo précédente", MF_ENABLED);
            item(M_OPEN, "Voir la photo dans Immich", photo);
            item(M_FAV, "Ajouter aux favoris Immich", photo);
            item(M_HIDE, "Ne plus afficher cette photo", photo);
            sep();
            item(M_SETTINGS, "Paramètres…", MF_ENABLED);
            item(M_PAUSE, "Pause", check(paused));
            item(M_AUTO, "Démarrer avec Windows", check(platform::autostart_enabled()));
            item(M_LOG, "Ouvrir le journal", MF_ENABLED);
            let update = match offered {
                // Rien n'est installé sans ce clic et une confirmation.
                Some(v) => format!("Installer la version {v}…"),
                None => format!("Rechercher une mise à jour (version {VERSION})"),
            };
            item(M_UPDATE, &update, MF_ENABLED);
            if !is_portable() {
                item(M_UNINSTALL, "Désinstaller…", MF_ENABLED); // portable : il suffit de supprimer le dossier
            }
            sep();
            item(M_QUIT, "Quitter", MF_ENABLED);
            m
        }
    }

    fn show_menu(self: &Arc<Self>) {
        let m = self.build_menu();
        let id = unsafe {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Nécessaire pour que le menu se ferme quand on clique ailleurs.
            let _ = SetForegroundWindow(self.hwnd());
            let id = TrackPopupMenu(
                m,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                self.hwnd(),
                None,
            );
            let _ = PostMessageW(Some(self.hwnd()), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(m);
            id.0 as usize
        };
        self.menu_command(id);
    }

    fn open_photo(&self) {
        if let Some((_, url)) = self.core.last() {
            shell_open(&url);
        }
    }

    /// Ouvre la fenêtre de réglages (sur le thread de l'interface : appelable de partout).
    fn open_settings(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), WM_APP_SETTINGS, WPARAM(0), LPARAM(0));
        }
    }

    fn menu_command(self: &Arc<Self>, id: usize) {
        match id {
            M_NEXT => self.spawn("changement d'image", |a| {
                let _ = a.change();
            }),
            M_PREV => self.spawn("photo précédente", |a| a.previous()),
            M_OPEN => self.open_photo(),
            M_FAV => self.spawn("favori", |a| a.favorite()),
            M_HIDE => self.spawn("photo masquée", |a| a.hide()),
            M_SETTINGS => self.open_settings(),
            M_PAUSE => {
                {
                    let mut st = lock(&self.st);
                    st.paused = !st.paused;
                }
                self.poke();
            }
            M_AUTO => {
                if let Err(e) = platform::set_autostart(!platform::autostart_enabled()) {
                    platform::error(&format!("Impossible de modifier le démarrage auto : {e}"));
                }
            }
            M_LOG => shell_open(&self.log_path.to_string_lossy()),
            M_UPDATE => self.spawn("mise à jour", |a| updater::check_and_update(a, true)),
            M_UNINSTALL => {
                // Lancé dans un processus séparé : il nous arrête puis supprime nos fichiers.
                if let Ok(me) = std::env::current_exe() {
                    if let Err(e) = hidden_cmd(me).arg("--uninstall").spawn() {
                        logf!("lancement de la désinstallation : {e}");
                    }
                }
            }
            M_QUIT => self.quit(),
            _ => {}
        }
    }

    // ---------- démarrage ----------

    fn on_ready(self: &Arc<Self>) {
        if !self.cfg_path.exists() {
            if let Err(e) = write_default_config(&self.cfg_path) {
                logf!("création de la configuration par défaut : {e}");
            }
        }
        if key_stored_plain(&self.cfg_path) {
            // Clé API en clair (ancienne version, édition à la main) : on la réécrit chiffrée.
            if let Ok(cfg) = read_config(&self.cfg_path) {
                match save_config(&self.cfg_path, &cfg) {
                    Err(e) => logf!("chiffrement de la clé API : {e}"),
                    Ok(()) if !key_stored_plain(&self.cfg_path) => logf!("clé API chiffrée dans config.json (DPAPI)"),
                    Ok(()) => {}
                }
            }
        }
        match load_config(&self.cfg_path) {
            Ok(cfg) => {
                self.apply_config(cfg, true);
                self.set_status("Prêt");
            }
            Err(e) => {
                logf!("config incomplète : {e}");
                self.set_status("Configuration à compléter");
                if !self.selftest {
                    self.open_settings();
                }
            }
        }
        if self.selftest {
            return;
        }
        self.spawn("mise à jour", |a| updater::update_loop(a.clone()));
        if has_arg("--after-update") {
            std::thread::spawn(updater::report_healthy);
        }
    }
}

impl SettingsHooks for Arc<App> {
    fn saved(&self, cfg: Config) {
        self.apply_config(cfg, false);
    }
    fn next(&self) -> Result<String, String> {
        self.change().map(|a| format!("Nouvelle image : {}", a.original_file_name))
    }
    fn status(&self) -> String {
        self.get_status()
    }
    fn excluded(&self) -> usize {
        self.core.excluded_count()
    }
    fn clear_excluded(&self) -> Result<(), String> {
        self.core.clear_excluded().map_err(|e| e.to_string())
    }
}

/// Choisit dans icon.ico l'image la plus proche de la taille demandée et en fait une icône.
pub(crate) fn load_icon(size: i32) -> Option<HICON> {
    let b = ICON_ICO;
    let count = u16::from_le_bytes([*b.get(4)?, *b.get(5)?]) as usize;
    let mut best: Option<(i32, usize, usize)> = None;
    for i in 0..count {
        let e = b.get(6 + 16 * i..22 + 16 * i)?;
        let w = if e[0] == 0 { 256 } else { e[0] as i32 };
        let len = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        // La plus petite image au moins aussi grande que demandé, sinon la plus grande.
        let better = match best {
            None => true,
            Some((bw, _, _)) => (w >= size && (bw < size || w < bw)) || (w < size && bw < size && w > bw),
        };
        if better {
            best = Some((w, off, len));
        }
    }
    let (_, off, len) = best?;
    let data = b.get(off..off.checked_add(len)?)?;
    unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR).ok() }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(app) = APP.get() else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };
    match msg {
        WM_APP_TRAY => {
            match lparam.0 as u32 & 0xFFFF {
                WM_LBUTTONUP => app.open_photo(), // clic gauche : ouvrir la photo affichée dans Immich
                WM_RBUTTONUP | WM_CONTEXTMENU => app.show_menu(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_POKE => {
            app.check();
            LRESULT(0)
        }
        WM_APP_SETTINGS => {
            if let Some(s) = app.settings.get() {
                settings_window::open(s.clone());
            }
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_SCHED => app.check(),
                TIMER_EVENTS => app.fire_event(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            app.system_event(EV_DISPLAY);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_WTSSESSION_CHANGE => {
            if wparam.0 as u32 == WTS_SESSION_UNLOCK {
                app.system_event(EV_UNLOCK);
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            match wparam.0 as u32 {
                PBT_APMRESUMEAUTOMATIC => app.system_event(EV_DISPLAY), // sortie de veille
                PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
                    // Données : 1 = écran allumé.
                    let s = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                    if s.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && s.DataLength >= 1 && s.Data[0] == 1 {
                        app.system_event(EV_DISPLAY);
                    }
                }
                _ => {}
            }
            LRESULT(1)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            app.tray(NIM_DELETE, "");
            PostQuitMessage(0);
            LRESULT(0)
        }
        // L'explorateur a redémarré : l'icône doit être recréée.
        m if Some(&m) == TASKBAR_CREATED.get() => {
            app.tray(NIM_ADD, "Immich Wallpaper");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("ImmichWallpaperEvents");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        // Fenêtre de premier niveau jamais affichée (style WS_POPUP, taille nulle).
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            class,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
    }
}

/// Fonds d'écran « appliqués » en mode vérification (rien n'est réellement changé).
static SELFTEST_APPLIED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn applied_count() -> usize {
    lock(&SELFTEST_APPLIED).len()
}

/// Attend qu'une condition devienne vraie (au plus `secs` secondes).
fn wait_for(secs: u64, cond: impl Fn() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cond()
}

/// Mode de vérification (`--selftest <fichier> [--selftest-dir <dossier>]`) : démarre l'appli
/// dans un dossier à part, sans toucher au fond d'écran, à l'écran de verrouillage, au bureau ni
/// à la configuration réelle. Vérifie l'icône, le menu et la fenêtre de réglages ; si le dossier
/// contient une configuration valide (serveur de test), déroule aussi un parcours complet :
/// tirage, image suivante, photo précédente, changement d'écran, pause, favori, exclusion.
/// Le résultat est écrit dans le fichier, puis l'appli quitte.
fn selftest_report(app: &Arc<App>, out: PathBuf) {
    std::thread::sleep(Duration::from_millis(1500));
    let mut problems: Vec<String> = vec![];
    let mut check = |ok: bool, what: &str| {
        if !ok {
            problems.push(what.to_string());
        }
    };
    check(app.tray_added.load(Ordering::SeqCst), "icône de la barre système non créée");
    let m = app.build_menu();
    unsafe {
        check(GetMenuItemCount(Some(m)) >= 12, "menu incomplet");
        let _ = DestroyMenu(m);
    }
    // Fenêtre de réglages : ouverte depuis le menu, remplie (albums du serveur de test si la
    // configuration est valide), puis refermée sans modification.
    app.menu_command(M_SETTINGS);
    check(wait_for(10, settings_window::is_open), "fenêtre de réglages non ouverte");
    // Ouverte ne suffit pas : elle doit être visible, même quand l'appli a été lancée « cachée »
    // (comme le fait la mise à jour depuis les anciennes versions).
    check(
        wait_for(5, || unsafe {
            IsWindowVisible(HWND(settings_window::open_hwnd() as *mut _)).as_bool()
        }),
        "fenêtre de réglages ouverte mais invisible",
    );
    if app.cfg().is_some() {
        check(
            wait_for(20, || settings_window::loaded_albums() > 0),
            "albums absents de la fenêtre de réglages",
        );
    }
    unsafe {
        let _ = PostMessageW(Some(HWND(settings_window::open_hwnd() as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    check(wait_for(10, || !settings_window::is_open()), "fenêtre de réglages non refermée");

    let mut steps = 0;
    if app.cfg().is_some() {
        let status = || lock(&app.st).status.clone();
        // 1. Premier tirage, déclenché par la planification au démarrage.
        check(wait_for(40, || applied_count() >= 1), "aucun fond d'écran après le démarrage");
        let first = lock(&SELFTEST_APPLIED).first().cloned().unwrap_or_default();
        let dims = std::fs::read(&first).ok().and_then(|b| crate::render::jpeg_dims(&b));
        let sc = platform::screen_for("fill");
        check(
            dims == Some((sc.w as u32, sc.h as u32)),
            &format!("image rendue {dims:?} au lieu de la taille de l'écran {}x{}", sc.w, sc.h),
        );
        check(wait_for(20, || !app.busy.load(Ordering::SeqCst)), "premier tirage non terminé");
        check(app.get_status().contains("prochaine à"), "prochaine échéance absente du statut");
        // 2. Image suivante depuis le menu.
        let n = applied_count();
        app.menu_command(M_NEXT);
        check(
            wait_for(40, || applied_count() > n && !app.busy.load(Ordering::SeqCst)),
            "« Image suivante » sans effet",
        );
        // 3. Photo précédente.
        let n = applied_count();
        app.menu_command(M_PREV);
        check(
            wait_for(20, || applied_count() > n && status().starts_with("Photo précédente")),
            "« Photo précédente » sans effet",
        );
        // 4. Changement d'écran : le fond est recalculé après 3 s de calme.
        let n = applied_count();
        unsafe {
            let _ = PostMessageW(Some(app.hwnd()), WM_DISPLAYCHANGE, WPARAM(32), LPARAM(0));
        }
        check(wait_for(15, || applied_count() > n), "changement d'écran non pris en compte");
        // 5. Pause.
        app.menu_command(M_PAUSE);
        check(app.get_status().contains("en pause"), "pause non prise en compte");
        app.menu_command(M_PAUSE);
        check(!app.get_status().contains("en pause"), "reprise non prise en compte");
        // 6. Favori.
        app.menu_command(M_FAV);
        check(
            wait_for(20, || status().starts_with("Ajoutée aux favoris")),
            &format!("favori : {}", status()),
        );
        // 7. Exclusion : la photo est masquée et une autre est affichée.
        let n = applied_count();
        app.menu_command(M_HIDE);
        check(
            wait_for(40, || {
                applied_count() > n && app.core.excluded_count() == 1 && !app.busy.load(Ordering::SeqCst)
            }),
            "« Ne plus afficher » sans effet",
        );
        // 8. Préchargement de la photo suivante (lancé après chaque tirage).
        let log = || std::fs::read_to_string(&app.log_path).unwrap_or_default();
        check(wait_for(30, || log().contains("préchargée")), "aucun préchargement dans le journal");
        check(!log().contains("panic"), "panic dans le journal");
        steps = applied_count();
    }
    drop(check);

    let report = format!(
        "{}\nversion={VERSION} portable={} fonds_appliqués={steps} écrans={} batterie={} plein_écran={} connexion_limitée={}\n",
        if problems.is_empty() {
            "OK".to_string()
        } else {
            format!("ÉCHEC : {}", problems.join(" ; "))
        },
        is_portable(),
        list_monitors().len(),
        crate::win::context::on_battery(),
        crate::win::context::fullscreen_busy(),
        crate::win::context::metered_connection(),
    );
    let _ = std::fs::write(out, report);
    app.quit();
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let selftest = arg_value("--selftest");

    if selftest.is_none() {
        // La version portable n'est pas installée : rien à désinstaller (et surtout pas une
        // éventuelle copie installée sur le même PC).
        if has_arg("--uninstall") && !is_portable() {
            uninstall(has_arg("--quiet"));
            return;
        }
        if maybe_install() {
            return;
        }
        if has_arg("--after-update") {
            if !updater::wait_for_instance() {
                return;
            }
        } else if !platform::single_instance() {
            platform::info("L'application est déjà lancée (icône près de l'horloge).");
            return;
        }
    }

    let dir = match &selftest {
        Some(_) => arg_value("--selftest-dir")
            .unwrap_or_else(|| std::env::temp_dir().join(format!("ImmichWallpaper-selftest-{}", std::process::id()))),
        None => platform::data_dir(),
    };
    let dir_err = std::fs::create_dir_all(&dir).err();
    if let (Some(e), true) = (&dir_err, is_portable() && selftest.is_none()) {
        platform::error(&format!(
            "Version portable : impossible de créer le dossier de données à côté de l'exe.\n\
             Place-la dans un dossier où tu peux écrire.\n\n{e}"
        ));
        return;
    }
    let log_path = dir.join("journal.txt");
    // Journal borné à 1 Mo, l'ancien est gardé dans journal.1.txt.
    if let Ok(l) = crate::logfile::RotatingLog::open(&log_path, 1 << 20) {
        crate::util::set_log_file(l);
    }
    std::panic::set_hook(Box::new(|info| logf!("panic : {info}")));
    logf!(
        "--- démarrage v{VERSION}{}, dossier {}",
        if is_portable() { " (portable)" } else { "" },
        dir.display()
    );
    if let Some(e) = dir_err {
        logf!("dossier de données : {e}");
    }
    if selftest.is_none() {
        write_pid();
        updater::cleanup_after_update();
    }
    unsafe {
        // COM multithread pour tout le processus : les threads de travail (écrans, écran de
        // verrouillage, coût de la connexion) n'ont rien à initialiser.
        let _ = CoIncrementMTAUsage();
    }

    let hooks = if selftest.is_some() {
        // Vérification : les images sont rendues dans le dossier de test mais jamais appliquées.
        let mut h = Hooks::new(|p, _| {
            lock(&SELFTEST_APPLIED).push(p.to_path_buf());
            Ok(())
        });
        h.screen = Some(Box::new(platform::screen_for));
        h
    } else {
        let mut h = Hooks::new(platform::set_wallpaper);
        h.set_lock_screen = Some(Box::new(platform::set_lock_screen));
        h.screen = Some(Box::new(platform::screen_for));
        h.monitors = Some(Box::new(|_| list_monitors()));
        h.set_monitor_wallpapers = Some(Box::new(set_monitor_wallpapers));
        h.after_apply = Some(Box::new(platform::update_desktop_link));
        h
    };

    let app = Arc::new(App {
        cfg_path: dir.join("config.json"),
        log_path,
        core: Core::new(&dir, hooks),
        settings: OnceLock::new(),
        st: Mutex::new(AppState {
            cfg: None,
            next: SystemTime::now(),
            paused: false,
            status: "Démarrage…".into(),
            failures: 0,
            hold: None,
            pending: None,
            has_photo: false,
            offered: None,
            event: 0,
        }),
        busy: AtomicBool::new(false),
        updating: AtomicBool::new(false),
        hwnd: AtomicIsize::new(0),
        icon: AtomicIsize::new(0),
        tray_added: AtomicBool::new(false),
        selftest: selftest.is_some(),
    });
    let _ = APP.set(app.clone());

    let settings = Settings::new(&app.cfg_path);
    settings.set_hooks(Arc::new(app.clone()));
    let _ = app.settings.set(settings);

    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            logf!("fenêtre de l'appli : {e}");
            platform::error(&format!("Impossible de démarrer l'icône de la barre système : {e}"));
            return;
        }
    };
    app.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
    unsafe {
        // Le premier ShowWindow d'un processus suit la façon dont il a été lancé : « caché » quand
        // c'est par la mise à jour des versions précédentes (Go : HideWindow) ou un script. Il est
        // consommé ici, sur la fenêtre cachée de l'appli, et non par la fenêtre de réglages.
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    unsafe {
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
        if let Some(icon) = load_icon(GetSystemMetrics(SM_CXSMICON)) {
            app.icon.store(icon.0 as isize, Ordering::SeqCst);
        }
        app.tray(NIM_ADD, "Immich Wallpaper");
        if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
            logf!("notification de déverrouillage indisponible : {e}");
        }
        let _ = RegisterPowerSettingNotification(hwnd.into(), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE);
    }

    app.on_ready();
    app.check();
    if let Some(out) = selftest {
        app.spawn("vérification", move |a| selftest_report(a, out));
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            // Fenêtre de réglages : navigation au clavier des boîtes de dialogue.
            if settings_window::pre_translate(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    logf!("--- arrêt");
    let pending = lock(&app.st).pending.take();
    if let Some(p) = pending {
        p.watch(); // surveille le démarrage de la nouvelle version (retour arrière si plantage)
    }
    if app.selftest && arg_value("--selftest-dir").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}
