//! Mise à jour depuis les releases GitHub (partie indépendante de Windows).

use crate::immich::{http_agent, read_limited};
use crate::util::{err, sha256_hex, Error, Res};
use serde::Deserialize;
use std::time::Duration;

/// Dépôt GitHub (public) dont les releases servent aux mises à jour.
pub const UPDATE_REPO: &str = "spartan-dou/immich-wallpaper";

pub fn update_api() -> String {
    format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest")
}

/// Seuls les fichiers de release de ce dépôt sont téléchargés.
pub fn update_asset_prefix() -> String {
    format!("https://github.com/{UPDATE_REPO}/releases/download/")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GhRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

/// Transforme « v1.4.0 » / « 1.4 » en [1, 4, 0]. None pour une version de dev.
pub fn parse_version(s: &str) -> Option<[u32; 3]> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut v = [0u32; 3];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        v[i] = p.parse().ok()?;
    }
    Some(v)
}

pub fn newer_version(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// Fichier de release à télécharger pour se mettre à jour. L'exe portable (`flavor` =
/// "portable") se met à jour avec l'exe portable ; les copies installées (quelle que soit leur
/// origine) et le zip portable (exe classique + portable.txt) avec l'exe classique.
pub fn asset_name(arch: &str, flavor: &str) -> String {
    let arch = if arch == "aarch64" || arch == "arm64" { "arm64" } else { "x64" };
    if flavor == "portable" {
        format!("ImmichWallpaper-portable-{arch}.exe")
    } else {
        format!("ImmichWallpaper-{arch}.exe")
    }
}

/// Fichier de release qui met à jour l'exe en cours (selon sa variante, pas le fichier témoin).
pub fn update_asset_name() -> String {
    asset_name(std::env::consts::ARCH, crate::FLAVOR)
}

/// Ce que fait une vérification qui a trouvé une nouvelle version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePlan {
    /// Vérification automatique d'une version qui a déjà échoué au démarrage : rien.
    Skip,
    /// Vérification automatique de la version portable (PC professionnel, clé USB…) : la
    /// version est seulement annoncée dans le menu de l'icône, rien n'est téléchargé.
    Offer,
    /// Clic dans le menu : confirmation, puis téléchargement et remplacement de l'exe.
    Confirm,
    /// Vérification automatique d'une copie non portable : installation sans question.
    Install,
}

/// manual : clic dans le menu ; skipped : version qui a échoué au démarrage précédemment.
pub fn update_plan(manual: bool, portable: bool, skipped: bool) -> UpdatePlan {
    if manual {
        UpdatePlan::Confirm
    } else if skipped {
        UpdatePlan::Skip
    } else if portable {
        UpdatePlan::Offer
    } else {
        UpdatePlan::Install
    }
}

/// GitHub redirige les fichiers de release vers son stockage ; toute redirection hors HTTPS
/// ou hors de ses domaines est refusée.
pub fn check_update_redirect(target: &str) -> Res<()> {
    let u = url::Url::parse(target).map_err(|_| Error::Msg("redirection invalide".into()))?;
    let h = u.host_str().unwrap_or("");
    if u.scheme() != "https" || !(h == "github.com" || h.ends_with(".github.com") || h.ends_with(".githubusercontent.com")) {
        return err(format!("redirection refusée vers {}://{h}", u.scheme()));
    }
    Ok(())
}

/// GET avec suivi manuel des redirections (chacune est contrôlée).
fn gh_get(url: &str) -> Res<ureq::Response> {
    let agent = http_agent(None, 0);
    let mut target = url.to_string();
    for _ in 0..6 {
        let res = agent
            .get(&target)
            .set("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(5 * 60))
            .call();
        let resp = match res {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return err(format!("GitHub : HTTP {code} sur {target}")),
            Err(e) => return Err(Error::Msg(e.to_string())),
        };
        match resp.status() {
            200 => return Ok(resp),
            301 | 302 | 303 | 307 | 308 => {
                let loc = resp.header("Location").unwrap_or("").to_string();
                let next = url::Url::parse(&target)
                    .and_then(|b| b.join(&loc))
                    .map_err(|_| Error::Msg("redirection invalide".into()))?;
                check_update_redirect(next.as_str())?;
                target = next.to_string();
            }
            code => return err(format!("GitHub : HTTP {code} sur {target}")),
        }
    }
    err("trop de redirections")
}

/// Mise à jour disponible.
#[derive(Debug, Clone)]
pub struct Update {
    pub version: String,
    exe_url: String,
    sums_url: String,
    name: String,
}

/// Renvoie la mise à jour disponible, ou None si on est à jour.
pub fn check_update(current: &str, asset: &str) -> Res<Option<Update>> {
    check_update_at(&update_api(), &update_asset_prefix(), current, asset)
}

pub fn check_update_at(api: &str, prefix: &str, current: &str, asset: &str) -> Res<Option<Update>> {
    if parse_version(current).is_none() {
        return err(format!("version de développement ({current}) : mise à jour automatique désactivée"));
    }
    let rel: GhRelease = serde_json::from_reader(gh_get(api)?.into_reader()).map_err(|e| Error::Msg(e.to_string()))?;
    if rel.draft || rel.prerelease || !newer_version(&rel.tag_name, current) {
        return Ok(None);
    }
    let find = |name: &str| {
        rel.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_default()
    };
    let u = Update {
        version: rel.tag_name.strip_prefix('v').unwrap_or(&rel.tag_name).to_string(),
        exe_url: find(asset),
        sums_url: find("SHA256SUMS.txt"),
        name: asset.to_string(),
    };
    if u.exe_url.is_empty() || u.sums_url.is_empty() {
        return err(format!("release {} incomplète (exe ou SHA256SUMS.txt manquant)", rel.tag_name));
    }
    for link in [&u.exe_url, &u.sums_url] {
        if !link.starts_with(prefix) {
            return err(format!("release {} : adresse de téléchargement inattendue ({link})", rel.tag_name));
        }
    }
    Ok(Some(u))
}

impl Update {
    /// Récupère l'exe et vérifie son empreinte SHA256 avant de le renvoyer.
    pub fn download(&self) -> Res<Vec<u8>> {
        let sums = read_limited(gh_get(&self.sums_url)?.into_reader(), 64 << 10)?;
        let sums = String::from_utf8_lossy(&sums);
        let want = sums
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                (f.len() == 2 && f[1].trim_start_matches('*') == self.name).then(|| f[0].to_lowercase())
            })
            .last();
        let Some(want) = want else {
            return err("empreinte introuvable dans SHA256SUMS.txt");
        };

        let bin = read_limited(gh_get(&self.exe_url)?.into_reader(), 100 << 20)?;
        if sha256_hex(&bin) != want {
            return err("empreinte SHA256 incorrecte : fichier téléchargé corrompu, mise à jour annulée");
        }
        if !bin.starts_with(b"MZ") {
            return err("le fichier téléchargé n'est pas un exécutable Windows");
        }
        Ok(bin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn versions() {
        for (l, c, want) in [
            ("v1.5.0", "1.4.0", true),
            ("v1.4.0", "1.4.0", false),
            ("v1.10.0", "1.9.3", true),
            ("v1.4", "1.4.0", false),
            ("v2.0.0", "dev-abc1234", false),
            ("v1.3.9", "1.4.0", false),
        ] {
            assert_eq!(newer_version(l, c), want, "{l} vs {c}");
        }
        assert_eq!(asset_name("x86_64", ""), "ImmichWallpaper-x64.exe");
        assert_eq!(asset_name("aarch64", ""), "ImmichWallpaper-arm64.exe");
        assert_eq!(asset_name("x86_64", "setup"), "ImmichWallpaper-x64.exe");
        assert_eq!(asset_name("x86_64", "portable"), "ImmichWallpaper-portable-x64.exe");
        assert_eq!(asset_name("aarch64", "portable"), "ImmichWallpaper-portable-arm64.exe");
    }

    #[test]
    fn redirects() {
        for (u, ok) in [
            ("https://release-assets.githubusercontent.com/x", true),
            ("https://github.com/x", true),
            ("http://github.com/x", false),
            ("https://evilgithub.com/x", false),
            ("https://example.org/x", false),
        ] {
            assert_eq!(check_update_redirect(u).is_ok(), ok, "{u}");
        }
    }

    #[test]
    fn check_and_download() {
        let mut bin = b"MZ".to_vec();
        bin.extend(std::iter::repeat(b'x').take(1000));
        let sum = sha256_hex(&bin);
        let name = asset_name("x86_64", "");
        let corrupt = Arc::new(AtomicBool::new(false));
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let (b2, n2, c2, bin2) = (base.clone(), name.clone(), corrupt.clone(), bin.clone());
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body: Vec<u8> = match req.url() {
                    "/latest" => serde_json::json!({"tag_name": "v1.5.0", "assets": [
                        {"name": n2, "browser_download_url": format!("{b2}/exe")},
                        {"name": "SHA256SUMS.txt", "browser_download_url": format!("{b2}/sums")},
                    ]})
                    .to_string()
                    .into_bytes(),
                    "/sums" => format!("{sum}  {n2}\nabc  autre.exe\n").into_bytes(),
                    "/exe" if c2.load(Ordering::SeqCst) => b"MZcorrompu".to_vec(),
                    "/exe" => bin2.clone(),
                    _ => vec![],
                };
                let _ = req.respond(tiny_http::Response::from_data(body));
            }
        });
        let api = format!("{base}/latest");
        let prefix = format!("{base}/");

        assert!(
            check_update_at(&api, &prefix, "1.5.0", &name).unwrap().is_none(),
            "déjà à jour attendu"
        );
        let u = check_update_at(&api, &prefix, "1.4.0", &name).unwrap().unwrap();
        assert_eq!(u.version, "1.5.0");
        assert_eq!(u.download().unwrap().len(), bin.len());
        corrupt.store(true, Ordering::SeqCst);
        assert!(u.download().unwrap_err().to_string().contains("SHA256"), "corruption non détectée");
        assert!(
            check_update_at(&api, &prefix, "dev-1234567", &name).is_err(),
            "une version de dev ne doit pas se mettre à jour"
        );
        // Fichiers hébergés ailleurs que dans les releases du dépôt : refus.
        let e = check_update_at(&api, &update_asset_prefix(), "1.4.0", &name).unwrap_err();
        assert!(e.to_string().contains("inattendue"), "{e}");
        // Exe d'une autre variante absent de la release : release incomplète.
        assert!(check_update_at(&api, &prefix, "1.4.0", &asset_name("x86_64", "portable")).is_err());
    }
}
