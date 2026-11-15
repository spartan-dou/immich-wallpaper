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

#[allow(dead_code)]
pub fn is_portable() -> bool {
    FLAVOR == "portable"
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
