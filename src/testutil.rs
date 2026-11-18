//! Outils de test : faux serveur Immich, images, dossiers temporaires.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

pub const ALB1: &str = "11111111-1111-1111-1111-111111111111";
pub const ALB2: &str = "22222222-2222-2222-2222-222222222222";
pub const KEY: &str = "secret-token";

// Identifiants d'assets du faux serveur (Immich utilise des UUID ; tout autre format est refusé).
pub const P1: &str = "aaaaaaaa-0000-4000-8000-000000000001"; // portrait
pub const L1: &str = "aaaaaaaa-0000-4000-8000-000000000002"; // paysage (PNG)
pub const L2: &str = "aaaaaaaa-0000-4000-8000-000000000003"; // paysage (JPEG)
pub const V1: &str = "aaaaaaaa-0000-4000-8000-000000000004"; // vidéo
pub const T1: &str = "aaaaaaaa-0000-4000-8000-000000000005"; // corbeille

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "iw_test_{name}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn mk_img(w: u32, h: u32, format: &str) -> Vec<u8> {
    let mut img = image::RgbImage::new(w, h);
    img.put_pixel(1, 1, image::Rgb([255, 0, 0]));
    let mut buf = std::io::Cursor::new(vec![]);
    let f = if format == "png" {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    image::DynamicImage::ImageRgb8(img).write_to(&mut buf, f).unwrap();
    buf.into_inner()
}

pub struct FakeState {
    /// Simule une panne du serveur.
    pub down: AtomicBool,
    /// Nombre de photos annoncé pour l'album 2 (sa signature).
    pub count2: AtomicI64,
    pub fetches: Mutex<HashMap<String, usize>>,
    /// (méthode, chemin) de chaque requête.
    pub log: Mutex<Vec<(String, String)>>,
}

/// Faux serveur Immich : ALB1 renvoie ses assets directement (ancienne API),
/// ALB2 ne les renvoie pas → passage par /api/search/metadata (paginé).
pub struct FakeImmich {
    pub url: String,
    pub state: Arc<FakeState>,
    server: Arc<Server>,
}

impl Drop for FakeImmich {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

impl FakeImmich {
    pub fn start() -> FakeImmich {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let state = Arc::new(FakeState {
            down: AtomicBool::new(false),
            count2: AtomicI64::new(2),
            fetches: Mutex::new(HashMap::new()),
            log: Mutex::new(vec![]),
        });
        let (srv, st) = (server.clone(), state.clone());
        std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let url = req.url().to_string();
                let (path, query) = url.split_once('?').unwrap_or((&url, ""));
                *st.fetches.lock().unwrap().entry(path.to_string()).or_default() += 1;
                st.log.lock().unwrap().push((req.method().to_string(), path.to_string()));
                let key_ok = req.headers().iter().any(|h| h.field.equiv("x-api-key") && h.value.as_str() == KEY);
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let json = |v: serde_json::Value| (200, v.to_string().into_bytes(), None);
                let status = |c: u16| (c, vec![], None);
                let asset = |id: &str| format!("/api/assets/{id}");
                let (code, data, location): (u16, Vec<u8>, Option<String>) = if st.down.load(Ordering::SeqCst) {
                    status(503)
                } else if !key_ok {
                    status(401)
                } else if path == "/api/albums" && query.contains("shared=true") {
                    json(serde_json::json!([
                        {"id": ALB1, "albumName": "Finlande", "assetCount": 3, "updatedAt": "2026-01-01T00:00:00Z"},
                        {"id": ALB2, "albumName": "Bassin", "assetCount": st.count2.load(Ordering::SeqCst), "updatedAt": "2026-01-01T00:00:00Z", "lastModifiedAssetTimestamp": null},
                    ]))
                } else if path == "/api/albums" {
                    json(serde_json::json!([{"id": ALB1, "albumName": "Finlande", "assetCount": 3, "updatedAt": "2026-01-01T00:00:00Z"}]))
                } else if path == format!("/api/albums/{ALB1}") {
                    json(serde_json::json!({"albumName": "Finlande", "assetCount": 3, "assets": [
                        {"id": P1, "type": "IMAGE", "originalFileName": "portrait.jpg"},
                        {"id": V1, "type": "VIDEO"},
                        {"id": T1, "type": "IMAGE", "isTrashed": true},
                        {"id": "..\\config", "type": "IMAGE", "originalFileName": "piege.jpg"}, // identifiant non UUID : ignoré
                    ]}))
                } else if path == format!("/api/albums/{ALB2}") {
                    json(serde_json::json!({"albumName": "Bassin", "assetCount": 2}))
                } else if path == "/api/search/metadata" {
                    let page = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v["page"].as_i64())
                        .unwrap_or(1);
                    if page == 1 {
                        json(
                            serde_json::json!({"assets": {"items": [{"id": L1, "type": "IMAGE", "originalFileName": "paysage.png"}], "nextPage": "2"}}),
                        )
                    } else {
                        json(
                            serde_json::json!({"assets": {"items": [{"id": L2, "type": "IMAGE", "originalFileName": "paysage2.jpg"}], "nextPage": null}}),
                        )
                    }
                } else if path == asset(L1) || path == asset(L2) {
                    // Informations (GET) ou mise en favori (PUT). Immich renvoie `null` pour les champs absents.
                    json(
                        serde_json::json!({"localDateTime": "2025-03-14T10:00:00.000Z", "isFavorite": *req.method() == Method::Put,
                        "exifInfo": {"city": "Rovaniemi", "state": "Laponie", "country": "Finlande", "dateTimeOriginal": null}}),
                    )
                } else if path == format!("{}/thumbnail", asset(P1)) {
                    (200, mk_img(30, 60, "jpeg"), None)
                } else if path == format!("{}/thumbnail", asset(L1)) {
                    if query.contains("size=fullsize") {
                        status(404) // simule un vieux Immich sans fullsize
                    } else {
                        (200, mk_img(80, 40, "png"), None)
                    }
                } else if path == format!("{}/thumbnail", asset(L2)) && query.contains("size=fullsize") {
                    (302, vec![], Some("original?c=1".to_string())) // comme Immich sans fullsize généré
                } else if path == format!("{}/original", asset(L2)) {
                    status(403) // token sans asset.download
                } else if path == format!("{}/thumbnail", asset(L2)) {
                    (200, mk_img(80, 40, "jpeg"), None)
                } else {
                    status(404)
                };
                let mut resp = Response::from_data(data).with_status_code(code);
                if let Some(l) = location {
                    resp = resp.with_header(Header::from_bytes("Location", l).unwrap());
                }
                let _ = req.respond(resp);
            }
        });
        FakeImmich {
            url: format!("http://{}", server.server_addr()),
            state,
            server,
        }
    }

    pub fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
    }

    pub fn fetches(&self, path: &str) -> usize {
        self.state.fetches.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    /// Nombre total de requêtes reçues.
    pub fn total(&self) -> usize {
        self.state.fetches.lock().unwrap().values().sum()
    }

    /// Nombre de requêtes dont le chemin commence par `prefix` et finit par `suffix`.
    pub fn count(&self, prefix: &str, suffix: &str) -> usize {
        self.state
            .fetches
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix) && k.ends_with(suffix))
            .map(|(_, v)| *v)
            .sum()
    }
}
