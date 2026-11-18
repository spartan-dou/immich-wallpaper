//! Appels à l'API Windows : fond d'écran, registre, boîtes de dialogue, DPAPI, écran de verrouillage.

use crate::config::{style_values, Config};
use crate::immich::Asset;
use crate::logf;
use crate::render::Screen;
use crate::util::{err, lock, Error, Res};
use base64::Engine;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL, RECT};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, ShellExecuteW, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MessageBoxW, SystemParametersInfoW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_YESNO,
    MESSAGEBOX_STYLE, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE,
    SPI_GETWORKAREA, SPI_SETDESKWALLPAPER, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};
use windows_registry::CURRENT_USER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "ImmichWallpaper";
pub const RUN_VALUE_PORTABLE: &str = "ImmichWallpaperPortable";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Msg(format!("{what} : {e}"))
}

// ---------- boîtes de dialogue, ouverture ----------

fn msg_box(text: &str, flags: MESSAGEBOX_STYLE) -> bool {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Immich Wallpaper"), flags) == IDYES }
}

pub fn info(text: &str) {
    msg_box(text, MB_ICONINFORMATION);
}

pub fn error(text: &str) {
    msg_box(text, MB_ICONERROR);
}

pub fn confirm(text: &str) -> bool {
    msg_box(text, MB_YESNO | MB_ICONQUESTION)
}

/// Ouvre une adresse ou un fichier avec le programme associé.
pub fn shell_open(target: &str) {
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as usize <= 32 {
        logf!("ouverture impossible ({target}) : code {}", r.0 as usize);
    }
}

/// Commande lancée sans fenêtre de console.
pub fn hidden_cmd(exe: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut c = std::process::Command::new(exe);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Chemin absolu d'un programme de Windows (System32), pour ne pas dépendre du PATH.
pub fn system_exe(name: &str) -> PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    let dir = if n > 0 && n < buf.len() {
        PathBuf::from(String::from_utf16_lossy(&buf[..n]))
    } else {
        PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32")
    };
    dir.join(name)
}

// ---------- dossiers ----------

/// Dossier de données de la version portable, à côté de l'exe.
pub fn portable_data_dir(exe: &Path) -> PathBuf {
    exe.parent().unwrap_or(Path::new(".")).join("ImmichWallpaper-data")
}

/// %APPDATA%\ImmichWallpaper, ou un dossier à côté de l'exe pour la version portable
/// (réglages, journal, cache et images suivent alors l'exe, rien n'est écrit ailleurs).
pub fn data_dir() -> PathBuf {
    data_dir_for(crate::is_portable())
}

pub fn data_dir_for(portable: bool) -> PathBuf {
    if portable {
        if let Ok(exe) = std::env::current_exe() {
            return portable_data_dir(&exe);
        }
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("ImmichWallpaper")
}

// ---------- fond d'écran ----------

/// Repasse l'arrière-plan en mode « Image » si « Windows à la une » ou un diaporama était actif,
/// sinon Windows peut remettre ses propres images par-dessus.
pub fn set_background_type_picture() {
    let Ok(k) = CURRENT_USER
        .options()
        .read()
        .write()
        .open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Wallpapers")
    else {
        return;
    };
    if matches!(k.get_u32("BackgroundType"), Ok(v) if v != 0) {
        if let Err(e) = k.set_u32("BackgroundType", 0) {
            logf!("arrière-plan en mode « Image » : {e}");
        }
    }
}

pub fn set_wallpaper(path: &Path, style: &str) -> Res<()> {
    let (ws, tile) = style_values(style).unwrap_or(("10", "0"));
    let reg = CURRENT_USER
        .create(r"Control Panel\Desktop")
        .and_then(|k| k.set_string("WallpaperStyle", ws).and_then(|_| k.set_string("TileWallpaper", tile)));
    if let Err(e) = reg {
        logf!("style du fond d'écran (registre) : {e}");
    }
    set_background_type_picture();
    let p = wide(&path.to_string_lossy());
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(p.as_ptr() as *mut _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .map_err(|e| win_err("SystemParametersInfo", e))
    }
}

/// Taille de l'écran principal (ou du bureau virtuel en mode span), en pixels physiques
/// grâce au manifeste PerMonitorV2.
pub fn screen_for(style: &str) -> Screen {
    unsafe {
        if style == "span" {
            return Screen {
                w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
                bottom_inset: 0,
            };
        }
        let mut sc = Screen {
            w: GetSystemMetrics(SM_CXSCREEN),
            h: GetSystemMetrics(SM_CYSCREEN),
            bottom_inset: 0,
        };
        let mut rc = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rc as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
        {
            let inset = sc.h - rc.bottom;
            if inset > 0 && inset < sc.h / 4 {
                sc.bottom_inset = inset;
            }
        }
        sc
    }
}

/// Écran de verrouillage, par l'API WinRT Windows.System.UserProfile.LockScreen.
pub fn set_lock_screen(path: &Path) -> Res<()> {
    let p = HSTRING::from(path.as_os_str());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = (|| -> windows::core::Result<()> {
            let file = windows::Storage::StorageFile::GetFileFromPathAsync(&p)?.join()?;
            windows::System::UserProfile::LockScreen::SetImageFileAsync(&file)?.join()
        })();
        let _ = tx.send(r);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(r) => r.map_err(|e| win_err("écran de verrouillage", e)),
        Err(_) => err("écran de verrouillage : pas de réponse de Windows après 60 s"),
    }
}

// ---------- raccourci du bureau ----------

const DESKTOP_LINK_NAME: &str = "Voir cette photo dans Immich.url";

fn desktop_link_path() -> PathBuf {
    // Gère le bureau redirigé (OneDrive…).
    let dir = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
    };
    let dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Desktop"));
    dir.join(DESKTOP_LINK_NAME)
}

/// Écrit (ou supprime) le raccourci du bureau vers la photo affichée, à la manière de
/// l'icône « En savoir plus sur cette image » de Windows à la une.
pub fn update_desktop_link(cfg: &Config, _a: &Asset, web_url: &str) {
    let path = desktop_link_path();
    if !cfg.desktop_link {
        let _ = std::fs::remove_file(path);
        return;
    }
    if web_url.contains(['\r', '\n']) {
        return; // fichier INI : jamais de retour à la ligne injecté
    }
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let content = format!("[InternetShortcut]\r\nURL={web_url}\r\nIconFile={exe}\r\nIconIndex=0\r\n");
    if let Err(e) = std::fs::write(&path, content) {
        logf!("raccourci du bureau : {e}");
    }
}

pub fn remove_desktop_link() {
    let _ = std::fs::remove_file(desktop_link_path());
}

// ---------- démarrage automatique ----------

/// Nom de l'entrée « démarrage automatique ». La version portable a la sienne, pour ne pas
/// modifier celle d'une copie installée sur le même PC.
pub fn autostart_value(portable: bool) -> &'static str {
    if portable {
        RUN_VALUE_PORTABLE
    } else {
        RUN_VALUE
    }
}

pub fn autostart_enabled() -> bool {
    CURRENT_USER
        .open(RUN_KEY)
        .and_then(|k| k.get_string(autostart_value(crate::is_portable())))
        .is_ok()
}

pub fn set_autostart(on: bool) -> Res<()> {
    let name = autostart_value(crate::is_portable());
    let k = CURRENT_USER.create(RUN_KEY).map_err(|e| win_err("registre", e))?;
    if !on {
        if k.get_string(name).is_err() {
            return Ok(()); // déjà absent
        }
        return k.remove_value(name).map_err(|e| win_err("registre", e));
    }
    let exe = std::env::current_exe()?;
    k.set_string(name, format!("\"{}\"", exe.display()))
        .map_err(|e| win_err("registre", e))
}

/// Démarrage automatique de la copie installée.
pub fn set_autostart_path(exe: &Path) -> Res<()> {
    CURRENT_USER
        .create(RUN_KEY)
        .and_then(|k| k.set_string(RUN_VALUE, format!("\"{}\"", exe.display())))
        .map_err(|e| win_err("registre", e))
}

pub fn remove_installed_autostart() {
    if let Ok(k) = CURRENT_USER.create(RUN_KEY) {
        let _ = k.remove_value(RUN_VALUE);
    }
}

// ---------- instance unique ----------

static INSTANCE: Mutex<Option<isize>> = Mutex::new(None);

/// Renvoie false si l'appli tourne déjà.
pub fn single_instance() -> bool {
    unsafe {
        match CreateMutexW(None, false, w!("Local\\ImmichWallpaperSingleton")) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(h);
                    return false;
                }
                *lock(&INSTANCE) = Some(h.0 as isize);
                true
            }
            Err(_) => true, // verrou indisponible : on ne bloque pas le lancement
        }
    }
}

/// Libère le verrou d'instance unique (avant de lancer la nouvelle version).
pub fn release_instance() {
    if let Some(h) = lock(&INSTANCE).take() {
        unsafe {
            let _ = CloseHandle(HANDLE(h as *mut _));
        }
    }
}

// ---------- clé API chiffrée (DPAPI) ----------

fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: b.len() as u32,
        pbData: b.as_ptr() as *mut u8,
    }
}

unsafe fn take_blob(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    v
}

/// Chiffre la valeur pour le compte Windows courant ; None si impossible (la valeur est alors
/// enregistrée en clair, comme avant).
pub fn protect_secret(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        match CryptProtectData(&blob(s.as_bytes()), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) {
            Ok(()) => Some(base64::engine::general_purpose::STANDARD.encode(take_blob(out))),
            Err(e) => {
                logf!("chiffrement de la clé API impossible, enregistrée en clair : {e}");
                None
            }
        }
    }
}

pub fn unprotect_secret(enc: &str) -> Result<String, String> {
    let b = base64::engine::general_purpose::STANDARD
        .decode(enc)
        .map_err(|_| "valeur chiffrée illisible".to_string())?;
    if b.is_empty() {
        return Err("valeur chiffrée illisible".into());
    }
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&blob(&b), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| e.to_string())?;
        String::from_utf8(take_blob(out)).map_err(|_| "valeur déchiffrée illisible".to_string())
    }
}
