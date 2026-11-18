//! Mise à jour automatique : remplacement de l'exe en cours, surveillance du redémarrage,
//! retour arrière si la nouvelle version ne démarre pas.

use crate::logf;
use crate::update::{asset_name, check_update};
use crate::win::app::App;
use crate::win::install::{has_arg, installed_exe, same_path, update_displayed_version, write_file_sync};
use crate::win::platform::{confirm, data_dir, error, hidden_cmd, info, release_instance, single_instance};
use crate::{is_portable, VERSION};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", p.display()))
}

/// Remplace l'exe en cours d'exécution. Windows interdit d'écraser un exe lancé, mais autorise
/// à le renommer. Le nouvel exe est d'abord écrit entièrement dans un fichier .new ;
/// l'emplacement final ne change ensuite que par deux renommages (exe → .old, .new → exe) :
/// une coupure pendant l'écriture ne laisse jamais d'exécutable tronqué.
fn replace_self(bin: &[u8]) -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let (old, tmp) = (with_suffix(&me, ".old"), with_suffix(&me, ".new"));
    if let Err(e) = write_file_sync(&tmp, bin) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&me, &old) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &me) {
        let _ = std::fs::rename(&old, &me); // retour arrière
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(me)
}

/// Ménage au démarrage et version affichée dans « Applications installées ».
/// Après une mise à jour, l'ancien exe (.old) n'est supprimé qu'une fois la nouvelle version
/// déclarée en bonne santé (voir report_healthy), pour permettre un retour arrière.
pub fn cleanup_after_update() {
    let Ok(me) = std::env::current_exe() else { return };
    let _ = std::fs::remove_file(with_suffix(&me, ".bad"));
    let _ = std::fs::remove_file(with_suffix(&me, ".new")); // mise à jour interrompue avant le renommage
    if !has_arg("--after-update") {
        let _ = std::fs::remove_file(with_suffix(&me, ".old"));
    }
    if same_path(&me, &installed_exe()) {
        update_displayed_version();
    }
}

fn health_file() -> PathBuf {
    data_dir().join("update_ok")
}
fn skip_file() -> PathBuf {
    data_dir().join("skip_version")
}

/// La nouvelle version tourne depuis 20 s → elle le signale à l'ancienne, qui peut alors se
/// terminer ; on supprime ensuite l'ancien exe.
pub fn report_healthy() {
    std::thread::sleep(Duration::from_secs(20));
    if let Err(e) = std::fs::write(health_file(), VERSION) {
        logf!("signal de bonne santé : {e}");
    }
    logf!("mise à jour vers {VERSION} validée");
    let Ok(me) = std::env::current_exe() else { return };
    let old = with_suffix(&me, ".old");
    for _ in 0..120 {
        if std::fs::remove_file(&old).is_ok() || !old.exists() {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Nouvelle version lancée, à surveiller avant de quitter.
pub struct PendingUpdate {
    me: PathBuf,
    version: String,
    child: std::process::Child,
}

impl PendingUpdate {
    /// Attend le signal de bonne santé de la nouvelle version. Si elle s'arrête avant (plantage
    /// au démarrage), on remet l'ancien exe en place, on le relance et on ignore cette version
    /// pour les mises à jour automatiques.
    pub fn watch(mut self) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if std::fs::read_to_string(health_file()).map(|s| s == self.version).unwrap_or(false) {
                let _ = std::fs::remove_file(health_file());
                return;
            }
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                if health_file().exists() {
                    return;
                }
                logf!("la version {} s'est arrêtée au démarrage : retour à {VERSION}", self.version);
                let bad = with_suffix(&self.me, ".bad");
                let _ = std::fs::remove_file(&bad);
                if std::fs::rename(&self.me, &bad).is_err() || std::fs::rename(with_suffix(&self.me, ".old"), &self.me).is_err() {
                    logf!("retour arrière impossible");
                    return;
                }
                if let Err(e) = std::fs::write(skip_file(), &self.version) {
                    logf!("mémorisation de la version à ignorer : {e}");
                }
                if let Err(e) = hidden_cmd(&self.me).arg("--after-update").spawn() {
                    logf!("relance de l'ancienne version : {e}");
                }
                return;
            }
            if Instant::now() >= deadline {
                return; // toujours en vie sans signal : on la laisse tourner
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// Attend que l'instance précédente libère le verrou (après une mise à jour).
pub fn wait_for_instance() -> bool {
    for _ in 0..40 {
        if single_instance() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Cherche une nouvelle version ; manual = déclenché depuis le menu.
pub fn check_and_update(app: &Arc<App>, manual: bool) {
    if app.updating.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_update(app, manual)));
    app.updating.store(false, Ordering::SeqCst);
}

fn run_update(app: &Arc<App>, manual: bool) {
    if !manual && !app.auto_update() {
        return;
    }
    // Le zip portable (exe classique + portable.txt) se met à jour avec l'exe classique ;
    // seule l'ancienne variante portable en un seul exe télécharge l'exe portable.
    let u = match check_update(VERSION, &asset_name(std::env::consts::ARCH, crate::FLAVOR == "portable")) {
        Ok(u) => u,
        Err(e) => {
            logf!("mise à jour : {e}");
            if manual {
                error(&format!("Impossible de vérifier les mises à jour :\n{e}"));
            }
            return;
        }
    };
    let Some(u) = u else {
        if manual {
            info(&format!("Immich Wallpaper est à jour (version {VERSION})."));
        }
        return;
    };
    if !manual && std::fs::read_to_string(skip_file()).map(|s| s.trim() == u.version).unwrap_or(false) {
        logf!("version {} ignorée (échec au démarrage précédemment)", u.version);
        return;
    }
    // Version portable (PC professionnel, clé USB…) : rien n'est téléchargé ni remplacé sans
    // un clic. La version trouvée est seulement annoncée dans le menu de l'icône.
    if !manual && is_portable() {
        logf!(
            "version {} disponible : proposée dans le menu de l'icône, rien n'est téléchargé",
            u.version
        );
        app.offer_update(&u.version);
        return;
    }
    if manual
        && !confirm(&format!(
            "Version {} disponible (actuelle : {VERSION}).\n\nInstaller maintenant ? L'appli redémarrera.",
            u.version
        ))
    {
        return;
    }
    logf!("mise à jour {VERSION} → {} : téléchargement", u.version);
    app.set_status(&format!("Mise à jour vers {}…", u.version));
    // Chemin récupéré avant le renommage : après, le processus courant pointe vers le .old.
    let me = match u
        .download()
        .map_err(|e| e.to_string())
        .and_then(|bin| replace_self(&bin).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            logf!("mise à jour : {e}");
            app.set_status("Échec de la mise à jour (voir le journal)");
            if manual {
                error(&format!("Échec de la mise à jour :\n{e}"));
            }
            return;
        }
    };
    logf!("mise à jour installée, redémarrage");
    let _ = std::fs::remove_file(health_file());
    release_instance();
    match hidden_cmd(&me).arg("--after-update").spawn() {
        Ok(child) => {
            app.set_pending(PendingUpdate {
                me,
                version: u.version,
                child,
            });
            app.quit();
        }
        Err(e) => {
            logf!("redémarrage impossible : {e}");
            let _ = std::fs::rename(&me, with_suffix(&me, ".bad"));
            let _ = std::fs::rename(with_suffix(&me, ".old"), &me);
            single_instance();
        }
    }
}

/// Première vérification 2 min après le démarrage, puis une fois par jour.
pub fn update_loop(app: Arc<App>) {
    std::thread::sleep(Duration::from_secs(120));
    // Réveil toutes les heures et comparaison à l'heure réelle : un simple sommeil de 24 h
    // peut ne pas avancer pendant la veille du PC.
    let mut last: Option<std::time::SystemTime> = None;
    loop {
        let due = last
            .map(|t| t.elapsed().map(|d| d >= Duration::from_secs(24 * 3600)).unwrap_or(true))
            .unwrap_or(true);
        if due {
            check_and_update(&app, false);
            last = Some(std::time::SystemTime::now());
        }
        std::thread::sleep(Duration::from_secs(3600));
    }
}
