// Icône et manifeste (DPI, contrôles communs) : on réutilise les ressources déjà compilées
// du dépôt (rsrc_windows_*.syso, objets COFF) en les passant telles quelles à l'éditeur de liens.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=IW_VERSION");
    println!("cargo:rerun-if-env-changed=IW_FLAVOR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "amd64",
    };
    let res = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("..")
        .join(format!("rsrc_windows_{arch}.syso"));
    println!("cargo:rerun-if-changed={}", res.display());
    if res.exists() {
        // Seulement pour l'exécutable final (pas pour les tests).
        println!("cargo:rustc-link-arg-bin=ImmichWallpaper={}", res.display());
    }
}
