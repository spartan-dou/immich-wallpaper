//! Installation, désinstallation, arrêt de l'instance en cours.

use crate::util::{Error, Res};
use crate::win::platform::{
    confirm, data_dir, error, hidden_cmd, info, remove_desktop_link, remove_installed_autostart, set_autostart_path, system_exe,
};
use crate::{is_portable, is_setup, VERSION};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows_registry::CURRENT_USER;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ImmichWallpaper";

pub fn has_arg(name: &str) -> bool {
    std::env::args().skip(1).any(|a| a == name)
}

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join("Programs").join("ImmichWallpaper")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("ImmichWallpaper.exe")
}

fn start_menu_shortcut() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"Microsoft\Windows\Start Menu\Programs\Immich Wallpaper.lnk")
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::path::absolute(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn is_installed() -> bool {
    installed_exe().exists()
}

// --- instance en cours (pour la mise à jour et la désinstallation) ---

fn pid_file() -> PathBuf {
    data_dir().join("instance.pid")
}

pub fn write_pid() {
    if let Err(e) = std::fs::write(pid_file(), std::process::id().to_string()) {
        crate::logf!("fichier PID : {e}");
    }
}

/// Termine l'autre instance (vérifie que le PID est bien Immich Wallpaper).
fn stop_running_instance() {
    stop_by_pid();
    // Filet de sécurité (anciennes versions sans fichier PID) : toute autre instance portant
    // exactement l'un des noms distribués (pas de motif, pour ne viser aucun autre programme).
    for name in ["ImmichWallpaper.exe", "ImmichWallpaper-x64.exe", "ImmichWallpaper-arm64.exe"] {
        let _ = hidden_cmd(system_exe("taskkill.exe"))
            .args(["/F", "/IM", name, "/FI", &format!("PID ne {}", std::process::id())])
            .status();
    }
    std::thread::sleep(Duration::from_millis(300));
}

fn stop_by_pid() {
    let Some(pid) = std::fs::read_to_string(pid_file()).ok().and_then(|s| s.trim().parse::<u32>().ok()) else {
        return;
    };
    if pid == std::process::id() {
        return;
    }
    unsafe {
        let Ok(h) = OpenProcess(
            PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) else {
            return;
        };
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ours = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok()
            && Path::new(&String::from_utf16_lossy(&buf[..n as usize]))
                .file_name()
                .map(|f| f.to_string_lossy().to_lowercase().starts_with("immichwallpaper"))
                .unwrap_or(false);
        if ours {
            let _ = TerminateProcess(h, 0);
            WaitForSingleObject(h, 5000);
            std::thread::sleep(Duration::from_millis(300)); // laisse Windows libérer le fichier
        }
        let _ = CloseHandle(h);
    }
}

// --- installation ---

/// Écrit le fichier et attend qu'il soit réellement sur le disque.
pub fn write_file_sync(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = PathBuf::from(format!("{}.new", dst.display()));
    let res = std::fs::read(src)
        .and_then(|data| write_file_sync(&tmp, &data))
        .and_then(|_| std::fs::rename(&tmp, dst));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Raccourci du menu Démarrer (IShellLink), pointant vers la copie installée.
fn create_shortcut(lnk: &Path, exe: &Path, dir: &Path) -> windows::core::Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

fn install() -> Res<()> {
    let me = std::env::current_exe()?;
    stop_running_instance();
    std::fs::create_dir_all(install_dir())?;
    let dst = installed_exe();
    let mut copied = Ok(());
    for _ in 0..5 {
        // l'ancien exe peut rester verrouillé un court instant
        copied = copy_file(&me, &dst);
        if copied.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    copied?;

    let size = std::fs::metadata(&dst).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    let d = dst.display().to_string();
    let reg = (|| -> windows_registry::Result<()> {
        let k = CURRENT_USER.create(UNINSTALL_KEY)?;
        k.set_string("DisplayName", "Immich Wallpaper")?;
        k.set_string("DisplayVersion", VERSION)?;
        k.set_string("Publisher", "Immich Wallpaper")?;
        k.set_string("DisplayIcon", &d)?;
        k.set_string("InstallLocation", install_dir().display().to_string())?;
        k.set_string("UninstallString", format!("\"{d}\" --uninstall"))?;
        k.set_string("QuietUninstallString", format!("\"{d}\" --uninstall --quiet"))?;
        k.set_u32("NoModify", 1)?;
        k.set_u32("NoRepair", 1)?;
        k.set_u32("EstimatedSize", size)
    })();
    reg.map_err(|e| Error::Msg(format!("entrée « Applications installées » : {e}")))?;

    // Raccourci menu Démarrer + démarrage automatique (pointant vers la copie installée).
    if let Err(e) = create_shortcut(&start_menu_shortcut(), &dst, &install_dir()) {
        // Non bloquant : l'appli reste installée et lancée au démarrage, sans raccourci.
        info(&format!("Raccourci du menu Démarrer non créé :\n{e}"));
    }
    set_autostart_path(&dst)
}

/// Version affichée dans « Applications installées » (après une mise à jour automatique).
pub fn update_displayed_version() {
    if let Ok(k) = CURRENT_USER.options().write().open(UNINSTALL_KEY) {
        if let Err(e) = k.set_string("DisplayVersion", VERSION) {
            crate::logf!("version affichée dans les applications installées : {e}");
        }
    }
}

/// Propose l'installation (ou la mise à jour) quand l'exe est lancé hors du dossier d'installation.
/// Renvoie true si le processus courant doit s'arrêter (la copie installée a été lancée, ou
/// l'installeur a été annulé).
pub fn maybe_install() -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    if is_portable() || same_path(&me, &installed_exe()) {
        return false;
    }
    // L'installeur ne sait qu'installer : refuser, c'est quitter. L'exe classique peut aussi
    // être lancé sans installation.
    let refuse = if is_setup() {
        "« Non » = annuler."
    } else {
        "« Non » = lancer sans installer."
    };
    let q = if is_installed() {
        format!(
            "Immich Wallpaper est déjà installé.\n\nRemplacer la version installée par celle-ci ({VERSION}) ?\n\
             Ta configuration est conservée.\n\n{refuse}"
        )
    } else {
        format!(
            "Installer Immich Wallpaper {VERSION} sur ce PC ?\n\n• copie dans {}\n\
             • raccourci dans le menu Démarrer, lancement avec Windows\n\
             • désinstallable depuis Paramètres → Applications installées\n\n{refuse}",
            install_dir().display()
        )
    };
    // --quiet : installation sans question (déploiement automatisé), installeur uniquement.
    if !(is_setup() && has_arg("--quiet")) && !confirm(&q) {
        return is_setup();
    }
    if let Err(e) = install() {
        error(&format!("Installation impossible : {e}"));
        return is_setup();
    }
    if let Err(e) = hidden_cmd(installed_exe()).spawn() {
        error(&format!("Installé, mais impossible de le lancer : {e}"));
    }
    true
}

// --- désinstallation ---

/// Attend ~3 s (le temps que ce processus se termine) puis supprime le dossier.
/// /s /c "…" : cmd retire seulement les guillemets extérieurs, ceux des chemins sont conservés.
pub fn uninstall_cmd_line(ping: &Path, target: &Path) -> String {
    format!(
        "/s /c \"\"{}\" -n 4 127.0.0.1 >nul & rd /s /q \"{}\"\"",
        ping.display(),
        target.display()
    )
}

/// Supprime tout : processus, démarrage auto, entrée « Applications », raccourci,
/// données (config, token, journal) et les fichiers du programme.
pub fn uninstall(quiet: bool) {
    if !quiet
        && !confirm(
            "Désinstaller Immich Wallpaper ?\n\nLa configuration (dont le token Immich) et le journal seront supprimés.\n\
             Le fond d'écran actuel restera en place.",
        )
    {
        return;
    }
    stop_running_instance();
    remove_installed_autostart();
    let _ = CURRENT_USER.remove_tree(UNINSTALL_KEY);
    let _ = std::fs::remove_file(start_menu_shortcut());
    remove_desktop_link();
    let _ = std::fs::remove_dir_all(data_dir());

    // Un exe ne peut pas se supprimer lui-même pendant qu'il tourne : on confie la suppression
    // à un cmd caché qui attend notre sortie.
    let me = std::env::current_exe().unwrap_or_default();
    let target = install_dir();
    let inside = me.parent().map(|d| same_path(d, &target)).unwrap_or(false);
    if !inside {
        let _ = std::fs::remove_dir_all(&target);
    }
    // Message d'abord : le dossier ne peut être supprimé qu'une fois ce processus terminé.
    if !quiet {
        info("Immich Wallpaper a été désinstallé.");
    }
    if !inside {
        return;
    }
    // Garde-fou : la ligne ci-dessous supprime un dossier entier ; on n'accepte que le dossier
    // d'installation attendu, sans caractère interprété par cmd.
    let t = target.to_string_lossy();
    if target.file_name().map(|f| f != "ImmichWallpaper").unwrap_or(true)
        || !target.is_absolute()
        || t.contains(['"', '%', '&', '|', '<', '>', '^'])
    {
        return;
    }
    // Ligne de commande brute : cmd ne comprend pas l'échappement \" que ferait la bibliothèque standard.
    let _ = hidden_cmd(system_exe("cmd.exe"))
        .raw_arg(uninstall_cmd_line(&system_exe("ping.exe"), &target))
        .spawn();
}
