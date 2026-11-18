//! Immich Wallpaper : fond d'écran Windows tiré d'albums Immich.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
// Hors Windows, seul le code indépendant de la plateforme est compilé (pour les tests) : le reste paraît inutilisé.
#![cfg_attr(not(windows), allow(dead_code))]

mod caption;
mod colorprofile;
mod config;
mod core;
mod immich;
mod logfile;
mod render;
mod settings;
mod update;
mod util;

#[cfg(test)]
mod core_tests;
#[cfg(test)]
mod testutil;
#[cfg(windows)]
mod win;

/// Version : injectée par la pipeline (IW_VERSION), sinon celle de Cargo.toml.
pub const VERSION: &str = match option_env!("IW_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Variante de l'exécutable (IW_FLAVOR à la compilation) :
///   - ""         : exe classique, celui que la mise à jour automatique des copies installées
///     télécharge ; lancé hors du dossier d'installation, il propose de s'installer ;
///   - "setup"    : installeur — installe (ou met à jour) puis lance la copie installée ;
///   - "portable" : ne s'installe jamais, garde ses données dans un dossier à côté de l'exe.
pub const FLAVOR: &str = match option_env!("IW_FLAVOR") {
    Some(v) => v,
    None => "",
};

/// Fichier qui, posé à côté de l'exe, le rend portable quelle que soit sa variante : c'est ce
/// que contient le zip portable (l'exe classique et ce fichier). Le supprimer rend l'appli
/// « normale ».
pub const PORTABLE_MARKER: &str = "portable.txt";

/// Le dossier contient le fichier témoin de la version portable (un dossier du même nom ne compte pas).
pub fn has_portable_marker(dir: &std::path::Path) -> bool {
    dir.join(PORTABLE_MARKER).is_file()
}

/// Version portable : variante compilée comme telle, ou fichier témoin à côté de l'exe.
/// Évalué une fois : l'exe ne change pas de dossier en cours de route.
#[allow(dead_code)]
pub fn is_portable() -> bool {
    static BY_MARKER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    FLAVOR == "portable"
        || *BY_MARKER.get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(has_portable_marker))
                .unwrap_or(false)
        })
}

#[allow(dead_code)]
pub fn is_setup() -> bool {
    FLAVOR == "setup"
}

#[cfg(windows)]
fn main() {
    win::app::run();
}

#[cfg(not(windows))]
fn main() {
    println!("Immich Wallpaper {VERSION} ne fonctionne que sous Windows.");
}
