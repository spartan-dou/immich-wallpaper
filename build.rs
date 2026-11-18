// Icône et manifeste (DPI par écran, contrôles communs v6) : objets COFF précompilés depuis
// res/app.rc (voir README), passés tels quels à l'éditeur de liens de l'exécutable.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=IW_VERSION");
    println!("cargo:rerun-if-env-changed=IW_FLAVOR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x64",
    };
    let res = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("res")
        .join(format!("app-{arch}.o"));
    println!("cargo:rerun-if-changed={}", res.display());
    if res.exists() {
        // Seulement pour l'exécutable final (pas pour les tests).
        println!("cargo:rustc-link-arg-bin=ImmichWallpaper={}", res.display());
    }
}
