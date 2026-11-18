//! Client de l'API Immich.

use crate::config::{is_uuid, normalize_fingerprint, null_default, Config};
use crate::util::{err, lock, Error, Res};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Requêtes JSON, et attente maximale entre deux lectures de la réponse.
const API_TIMEOUT: Duration = Duration::from_secs(120);
/// Téléchargement d'une image, corps compris.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Taille maximale d'une image téléchargée.
const MAX_DOWNLOAD: u64 = 200 << 20;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Asset {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(rename = "isTrashed", deserialize_with = "null_default")]
    pub is_trashed: bool,
    #[serde(rename = "originalFileName", deserialize_with = "null_default")]
    pub original_file_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Exif {
    #[serde(deserialize_with = "null_default")]
    pub city: String,
    #[serde(deserialize_with = "null_default")]
    pub state: String,
    #[serde(deserialize_with = "null_default")]
    pub country: String,
    #[serde(rename = "dateTimeOriginal", deserialize_with = "null_default")]
    pub date_time_original: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct AssetInfo {
    #[serde(rename = "localDateTime", deserialize_with = "null_default")]
    pub local_date_time: String,
    #[serde(rename = "exifInfo", deserialize_with = "null_default")]
    pub exif: Exif,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AlbumInfo {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "albumName", deserialize_with = "null_default")]
    pub album_name: String,
    #[serde(rename = "assetCount", deserialize_with = "null_default")]
    pub asset_count: i64,
    #[serde(rename = "updatedAt", deserialize_with = "null_default")]
    pub updated_at: String,
    #[serde(rename = "lastModifiedAssetTimestamp", deserialize_with = "null_default")]
    pub last_modified_asset_timestamp: String,
}

pub struct Client {
    base: String,
    key: String,
    agent: ureq::Agent,
    /// Empreinte SHA-256 (hex) du certificat accepté, si insecure_skip_verify.
    pin: Arc<Mutex<String>>,
    closed: AtomicBool,
}

/// Agent HTTP. `pin` : Some = certificat non vérifié mais épinglé (auto-signé) ; None = vérification normale.
pub fn http_agent(pin: Option<Arc<Mutex<String>>>, redirects: u32) -> ureq::Agent {
    let b = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(API_TIMEOUT)
        .redirects(redirects)
        .user_agent(&format!("ImmichWallpaper/{}", crate::VERSION));
    #[cfg(windows)]
    let b = b.tls_connector(Arc::new(tls::Schannel { pin }));
    #[cfg(not(windows))]
    let _ = pin;
    b.build()
}

/// Vérifie l'empreinte d'un certificat contre l'empreinte épinglée. Sans empreinte connue, le
/// premier certificat présenté est adopté (confiance au premier contact) et exigé ensuite.
pub fn check_pin(pin: &Mutex<String>, cert_der: &[u8]) -> Result<(), String> {
    let got = crate::util::sha256_hex(cert_der);
    let mut p = lock(pin);
    if p.is_empty() {
        crate::logf!("certificat non vérifié accepté et épinglé (SHA-256 {got})");
        *p = got;
        return Ok(());
    }
    if *p != got {
        return Err(format!(
            "le certificat du serveur a changé (empreinte {}… au lieu de {}…) : reteste la connexion \
             dans les paramètres puis enregistre si ce changement est attendu",
            &got[..16],
            &p[..p.len().min(16)]
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod tls {
    //! TLS par Schannel (magasin de certificats de Windows), avec épinglage optionnel vérifié
    //! juste après la poignée de main, avant tout envoi de la clé API.
    use std::io::{self, Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};

    pub struct Schannel {
        pub pin: Option<Arc<Mutex<String>>>,
    }

    fn io_err(e: impl ToString) -> ureq::Error {
        io::Error::new(io::ErrorKind::Other, e.to_string()).into()
    }

    impl ureq::TlsConnector for Schannel {
        fn connect(&self, dns_name: &str, io: Box<dyn ureq::ReadWrite>) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
            let mut b = native_tls::TlsConnector::builder();
            if self.pin.is_some() {
                b.danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true);
            }
            let conn = b.build().map_err(io_err)?;
            let stream = conn.connect(dns_name, io).map_err(|e| match e {
                native_tls::HandshakeError::Failure(e) => io_err(format!("TLS : {e}")),
                native_tls::HandshakeError::WouldBlock(_) => io_err("TLS : poignée de main interrompue"),
            })?;
            if let Some(pin) = &self.pin {
                let cert = stream
                    .peer_certificate()
                    .map_err(io_err)?
                    .ok_or_else(|| io_err("le serveur n'a présenté aucun certificat"))?;
                super::check_pin(pin, &cert.to_der().map_err(io_err)?).map_err(io_err)?;
            }
            Ok(Box::new(Stream(stream)))
        }
    }

    struct Stream(native_tls::TlsStream<Box<dyn ureq::ReadWrite>>);

    impl std::fmt::Debug for Stream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("TlsStream")
        }
    }
    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl ureq::ReadWrite for Stream {
        fn socket(&self) -> Option<&TcpStream> {
            self.0.get_ref().socket()
        }
    }
}

pub fn read_limited(r: impl Read, limit: u64) -> Res<Vec<u8>> {
    let mut out = vec![];
    r.take(limit).read_to_end(&mut out)?;
    Ok(out)
}

impl Client {
    pub fn new(cfg: &Config) -> Client {
        let pin = Arc::new(Mutex::new(normalize_fingerprint(&cfg.cert_fingerprint)));
        Client {
            base: cfg.server_url.clone(),
            key: cfg.api_key.clone(),
            agent: http_agent(cfg.insecure_skip_verify.then(|| pin.clone()), 5),
            pin,
            closed: AtomicBool::new(false),
        }
    }

    /// Refuse toute nouvelle requête (changement de configuration, arrêt).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Empreinte du certificat épinglé ("" si aucun).
    pub fn fingerprint(&self) -> String {
        lock(&self.pin).clone()
    }

    /// Envoie la requête. 401 et 403 deviennent des erreurs ; les autres statuts sont renvoyés.
    fn call(&self, method: &str, path: &str, body: Option<&serde_json::Value>, timeout: Duration) -> Res<(u16, ureq::Response)> {
        if self.closed.load(Ordering::SeqCst) {
            return err("requête annulée (configuration modifiée)");
        }
        let req = self
            .agent
            .request(method, &format!("{}{}", self.base, path))
            .set("x-api-key", &self.key)
            .timeout(timeout);
        let res = match body {
            Some(b) => req.set("Content-Type", "application/json").send_string(&b.to_string()),
            None => req.call(),
        };
        match res {
            Ok(r) => Ok((r.status(), r)),
            Err(ureq::Error::Status(401, _)) => Err(Error::Unauthorized),
            Err(ureq::Error::Status(403, _)) => Err(Error::Forbidden),
            Err(ureq::Error::Status(code, r)) => Ok((code, r)),
            Err(e) => Err(Error::Msg(e.to_string())),
        }
    }

    fn get_json<T: DeserializeOwned>(&self, method: &str, path: &str, body: Option<&serde_json::Value>) -> Res<T> {
        let (code, resp) = self.call(method, path, body, API_TIMEOUT)?;
        if code != 200 && code != 201 {
            let msg = read_limited(resp.into_reader(), 300).unwrap_or_default();
            return err(format!("{method} {path} : HTTP {code} {}", String::from_utf8_lossy(&msg).trim()));
        }
        serde_json::from_reader(resp.into_reader()).map_err(|e| Error::Msg(format!("{method} {path} : réponse illisible : {e}")))
    }

    /// Renvoie le nom et les images (hors corbeille) d'un album.
    pub fn album_images(&self, album_id: &str) -> Res<(String, Vec<Asset>)> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Album {
            #[serde(rename = "albumName", deserialize_with = "null_default")]
            album_name: String,
            #[serde(rename = "assetCount", deserialize_with = "null_default")]
            asset_count: i64,
            #[serde(deserialize_with = "null_default")]
            assets: Vec<Asset>,
        }
        let alb: Album = self.get_json("GET", &format!("/api/albums/{album_id}"), None)?;
        let mut all = alb.assets;
        // Les versions récentes d'Immich peuvent ne plus renvoyer les assets avec l'album :
        // on passe alors par la recherche par métadonnées.
        if all.is_empty() && alb.asset_count > 0 {
            all = self.search_album(album_id)?;
        }
        // L'identifiant sert de nom de fichier (cache) et dans des URL : UUID strict uniquement.
        all.retain(|a| a.kind == "IMAGE" && !a.is_trashed && is_uuid(&a.id));
        Ok((alb.album_name, all))
    }

    fn search_album(&self, album_id: &str) -> Res<Vec<Asset>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Page {
            #[serde(deserialize_with = "null_default")]
            items: Vec<Asset>,
            #[serde(rename = "nextPage")]
            next_page: serde_json::Value,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Found {
            #[serde(deserialize_with = "null_default")]
            assets: Page,
        }
        let mut out = vec![];
        for page in 1..=200 {
            let body = serde_json::json!({"albumIds": [album_id], "type": "IMAGE", "page": page, "size": 1000});
            let res: Found = self.get_json("POST", "/api/search/metadata", Some(&body))?;
            out.extend(res.assets.items);
            match &res.assets.next_page {
                serde_json::Value::Null => break,
                serde_json::Value::String(s) if s.is_empty() => break,
                _ => {}
            }
        }
        Ok(out)
    }

    /// Récupère l'image dans la taille demandée (fullsize retombe sur preview si absent).
    pub fn download(&self, asset_id: &str, size: &str) -> Res<Vec<u8>> {
        let path = if size == "original" {
            format!("/api/assets/{asset_id}/original")
        } else {
            format!("/api/assets/{asset_id}/thumbnail?size={size}")
        };
        let (code, resp) = match self.call("GET", &path, None, DOWNLOAD_TIMEOUT) {
            Ok(r) => r,
            // « fullsize » peut rediriger vers l'original, qui exige asset.download :
            // sans cette permission, on se rabat sur l'aperçu plutôt que d'échouer.
            Err(Error::Forbidden) if size == "fullsize" => {
                crate::logf!("fullsize refusé (permission asset.download absente ?), utilisation de l'aperçu");
                return self.download(asset_id, "preview");
            }
            Err(e) => return Err(e),
        };
        if code != 200 {
            if size == "fullsize" && (code == 400 || code == 404) {
                return self.download(asset_id, "preview");
            }
            return err(format!("téléchargement {asset_id} : HTTP {code}"));
        }
        read_limited(resp.into_reader(), MAX_DOWNLOAD)
    }

    pub fn asset_info(&self, id: &str) -> Res<AssetInfo> {
        self.get_json("GET", &format!("/api/assets/{id}"), None)
    }

    /// Ajoute la photo aux favoris Immich (permission asset.update).
    pub fn set_favorite(&self, id: &str) -> Res<()> {
        let body = serde_json::json!({"isFavorite": true});
        match self.get_json::<serde_json::Value>("PUT", &format!("/api/assets/{id}"), Some(&body)) {
            Ok(_) => Ok(()),
            Err(Error::Forbidden) => err("permission asset.update absente sur la clé API (Immich → Clés API)"),
            Err(e) => Err(e),
        }
    }

    /// Pour chaque album accessible, une empreinte qui change dès qu'une photo est ajoutée,
    /// retirée ou modifiée (une seule requête pour tous les albums).
    pub fn album_signatures(&self) -> Res<std::collections::HashMap<String, String>> {
        Ok(self
            .albums()?
            .into_iter()
            .map(|a| {
                let sig = format!("{}|{}|{}", a.asset_count, a.last_modified_asset_timestamp, a.updated_at);
                (a.id, sig)
            })
            .collect())
    }

    /// Liste les albums possédés et partagés avec l'utilisateur du token.
    pub fn albums(&self) -> Res<Vec<AlbumInfo>> {
        let mut out: Vec<AlbumInfo> = self.get_json("GET", "/api/albums", None)?;
        // Albums partagés : pas bloquant.
        if let Ok(shared) = self.get_json::<Vec<AlbumInfo>>("GET", "/api/albums?shared=true", None) {
            for a in shared {
                if !out.iter().any(|x| x.id == a.id) {
                    out.push(a);
                }
            }
        }
        Ok(out)
    }

    pub fn web_url(&self, asset_id: &str) -> String {
        format!("{}/photos/{}", self.base, asset_id)
    }
}
