//! Page de réglages servie sur 127.0.0.1, protégée par un jeton aléatoire dans l'URL.

use crate::config::{normalize_fingerprint, read_config_or_default, same_server, save_config, Config};
use crate::immich::Client;
use crate::util::{err, lock, Error, Res};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_http::{Header, Method, Request, Response};

const SETTINGS_HTML: &[u8] = include_bytes!("../../settings.html");

/// Taille maximale du corps d'une requête de la page (la configuration fait quelques Ko).
const MAX_BODY: u64 = 1 << 20;

/// Ce que la page peut demander à l'appli.
pub trait SettingsHooks: Send + Sync {
    fn saved(&self, cfg: Config);
    fn next(&self) -> Result<String, String>;
    fn status(&self) -> String;
    /// Nombre de photos masquées.
    fn excluded(&self) -> usize;
    /// Les réautorise toutes.
    fn clear_excluded(&self) -> Result<(), String>;
}

pub struct SettingsServer {
    cfg_path: PathBuf,
    token: String,
    addr: String,
    /// Protège les lectures/écritures du fichier de configuration.
    file: Mutex<()>,
    /// Empreinte du certificat vu lors du dernier test réussi, par URL de serveur (option
    /// « certificat auto-signé ») : c'est elle qui est épinglée à l'enregistrement.
    seen_pins: Mutex<HashMap<String, String>>,
    hooks: OnceLock<Arc<dyn SettingsHooks>>,
}

fn json_response(code: u16, v: &serde_json::Value) -> (u16, &'static str, Vec<u8>) {
    (code, "application/json", v.to_string().into_bytes())
}

fn error_response(e: impl std::fmt::Display) -> (u16, &'static str, Vec<u8>) {
    json_response(400, &serde_json::json!({ "error": e.to_string() }))
}

fn plain(code: u16, msg: &str) -> (u16, &'static str, Vec<u8>) {
    (code, "text/plain; charset=utf-8", msg.as_bytes().to_vec())
}

fn header(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

impl SettingsServer {
    pub fn start(cfg_path: &Path) -> Res<Arc<SettingsServer>> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| Error::Msg(format!("générateur aléatoire indisponible : {e}")))?;
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| Error::Msg(e.to_string()))?;
        let s = Arc::new(SettingsServer {
            cfg_path: cfg_path.to_path_buf(),
            token: crate::util::hex(&b),
            addr: server.server_addr().to_string(),
            file: Mutex::new(()),
            seen_pins: Mutex::new(HashMap::new()),
            hooks: OnceLock::new(),
        });
        let srv = s.clone();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                // Un thread par requête : « Image suivante » ne répond qu'à la fin du téléchargement.
                let srv = srv.clone();
                std::thread::spawn(move || srv.serve(req));
            }
            crate::logf!("page de réglages arrêtée");
        });
        Ok(s)
    }

    pub fn set_hooks(&self, h: Arc<dyn SettingsHooks>) {
        let _ = self.hooks.set(h);
    }

    pub fn url(&self) -> String {
        format!("http://{}/{}/", self.addr, self.token)
    }

    #[cfg(test)]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    fn serve(&self, mut req: Request) {
        let (code, ctype, body) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(&mut req))) {
            Ok(r) => r,
            Err(_) => plain(500, "erreur interne"),
        };
        let mut resp = Response::from_data(body).with_status_code(code);
        for (k, v) in [
            ("Content-Type", ctype),
            ("Cache-Control", "no-store"),
            ("Referrer-Policy", "no-referrer"),
            ("X-Content-Type-Options", "nosniff"),
            ("X-Frame-Options", "DENY"),
            (
                "Content-Security-Policy",
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; \
                 base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ),
        ] {
            if let Ok(h) = Header::from_bytes(k, v) {
                resp = resp.with_header(h);
            }
        }
        let _ = req.respond(resp);
    }

    fn read_json(&self, req: &mut Request) -> Res<Config> {
        let mut body = vec![];
        req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body)?;
        if body.len() as u64 > MAX_BODY {
            return err("requête trop volumineuse");
        }
        serde_json::from_slice(&body).map_err(|e| Error::Msg(e.to_string()))
    }

    fn handle(&self, req: &mut Request) -> (u16, &'static str, Vec<u8>) {
        // Host inattendu : requête forgée par un autre site (DNS rebinding).
        if header(req, "Host").as_deref() != Some(self.addr.as_str()) {
            return plain(403, "hôte refusé");
        }
        let url = req.url().to_string();
        let path = url.split('?').next().unwrap_or("");
        let prefix = format!("/{}/", self.token);
        let Some(route) = path.strip_prefix(&prefix) else {
            return plain(404, "introuvable");
        };

        match route {
            "" | "api/config" | "api/status" => {
                if !matches!(req.method(), Method::Get | Method::Head) {
                    return plain(405, "méthode refusée");
                }
            }
            // Routes d'action : POST JSON de la page elle-même uniquement.
            "api/albums" | "api/save" | "api/next" | "api/clear-excluded" => {
                if *req.method() != Method::Post {
                    return plain(405, "méthode refusée");
                }
                if let Some(o) = header(req, "Origin") {
                    if o != format!("http://{}", self.addr) {
                        return plain(403, "origine refusée");
                    }
                }
                if !header(req, "Content-Type").unwrap_or_default().starts_with("application/json") {
                    return plain(415, "type de contenu refusé");
                }
            }
            _ => return plain(404, "introuvable"),
        }

        let hooks = self.hooks.get();
        match route {
            "" => (200, "text/html; charset=utf-8", SETTINGS_HTML.to_vec()),
            "api/config" => {
                let cfg = {
                    let _g = lock(&self.file);
                    read_config_or_default(&self.cfg_path)
                };
                let has_key = !cfg.api_key.is_empty();
                let mut v = serde_json::to_value(&cfg).unwrap_or_default();
                if let Some(o) = v.as_object_mut() {
                    o.insert("api_key".into(), "".into()); // le token n'est jamais renvoyé à la page
                    o.remove("api_key_protected");
                    o.insert("has_key".into(), has_key.into());
                    o.insert("excluded_count".into(), hooks.map(|h| h.excluded()).unwrap_or(0).into());
                }
                json_response(200, &v)
            }
            "api/albums" => {
                let res = self.read_json(req).and_then(|c| self.merge_key(c)).and_then(|mut input| {
                    input.albums = vec!["00000000-0000-0000-0000-000000000000".into()]; // pour passer la validation
                    input.cert_fingerprint.clear(); // test : on relève le certificat présenté
                    input.normalize()?;
                    let client = Client::new(&input);
                    let albums = client.albums()?;
                    let fp = client.fingerprint();
                    if !fp.is_empty() {
                        lock(&self.seen_pins).insert(input.server_url.clone(), fp);
                    }
                    Ok(albums)
                });
                match res {
                    Ok(albums) => json_response(200, &serde_json::to_value(albums).unwrap_or_default()),
                    Err(e) => error_response(e),
                }
            }
            "api/save" => {
                let res = self.read_json(req).and_then(|c| self.merge_key(c)).and_then(|mut check| {
                    check.cert_fingerprint.clear();
                    check.normalize()?;
                    let _g = lock(&self.file);
                    if check.insecure_skip_verify {
                        // Empreinte relevée au dernier test de ce serveur, sinon celle déjà enregistrée pour lui.
                        if let Some(fp) = lock(&self.seen_pins).get(&check.server_url) {
                            check.cert_fingerprint = fp.clone();
                        } else {
                            let old = read_config_or_default(&self.cfg_path);
                            if same_server(&old.server_url, &check.server_url) {
                                check.cert_fingerprint = normalize_fingerprint(&old.cert_fingerprint);
                            }
                        }
                    }
                    save_config(&self.cfg_path, &check)?;
                    Ok(check)
                });
                match res {
                    Ok(cfg) => {
                        if let Some(h) = hooks {
                            h.saved(cfg);
                        }
                        json_response(200, &serde_json::json!({"ok": "enregistré"}))
                    }
                    Err(e) => error_response(e),
                }
            }
            "api/next" => match hooks.map(|h| h.next()) {
                None => json_response(200, &serde_json::json!({})),
                Some(Ok(msg)) => json_response(200, &serde_json::json!({ "ok": msg })),
                Some(Err(e)) => error_response(e),
            },
            "api/clear-excluded" => match hooks.map(|h| h.clear_excluded()) {
                Some(Err(e)) => error_response(e),
                _ => json_response(200, &serde_json::json!({"ok": "photos réaffichées"})),
            },
            // "api/status"
            _ => json_response(200, &serde_json::json!({ "status": hooks.map(|h| h.status()).unwrap_or_default() })),
        }
    }

    /// Garde le token enregistré si le champ est laissé vide dans la page, mais seulement pour
    /// le serveur auquel il a été confié : sinon une requête pourrait le faire envoyer ailleurs.
    fn merge_key(&self, mut input: Config) -> Res<Config> {
        if !input.api_key.trim().is_empty() {
            return Ok(input);
        }
        let old = {
            let _g = lock(&self.file);
            read_config_or_default(&self.cfg_path)
        };
        if !old.api_key.is_empty() && !same_server(&old.server_url, &input.server_url) {
            return err("l'URL du serveur a changé : saisis de nouveau le token (clé API)");
        }
        input.api_key = old.api_key;
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::write_default_config;
    use crate::testutil::{temp_dir, FakeImmich, ALB1, ALB2, KEY};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct TestHooks {
        saved: Mutex<Option<Config>>,
        excluded: AtomicUsize,
    }

    impl SettingsHooks for TestHooks {
        fn saved(&self, cfg: Config) {
            *lock(&self.saved) = Some(cfg);
        }
        fn next(&self) -> Result<String, String> {
            Ok("ok".into())
        }
        fn status(&self) -> String {
            "prêt".into()
        }
        fn excluded(&self) -> usize {
            self.excluded.load(Ordering::SeqCst)
        }
        fn clear_excluded(&self) -> Result<(), String> {
            self.excluded.store(0, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Requête brute : renvoie (statut, corps JSON ou Null, en-têtes utiles).
    fn request(method: &str, url: &str, ctype: Option<&str>, body: Option<&Value>, host: Option<&str>) -> (u16, Value, Option<String>) {
        let mut r = ureq::request(method, url);
        if let Some(c) = ctype {
            r = r.set("Content-Type", c);
        }
        if let Some(h) = host {
            r = r.set("Host", h);
        }
        let res = match body {
            Some(b) => r.send_string(&b.to_string()),
            None => r.call(),
        };
        let resp = match res {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => panic!("{e}"),
        };
        let code = resp.status();
        let csp = resp.header("Content-Security-Policy").map(|s| s.to_string());
        let text = resp.into_string().unwrap_or_default();
        (code, serde_json::from_str(&text).unwrap_or(Value::Null), csp)
    }

    #[test]
    fn settings_server() {
        let fake = FakeImmich::start();
        let cfg_path = temp_dir("settings").join("config.json");
        write_default_config(&cfg_path).unwrap();
        let s = SettingsServer::start(&cfg_path).unwrap();
        let hooks = Arc::new(TestHooks::default());
        hooks.excluded.store(3, Ordering::SeqCst);
        s.set_hooks(hooks.clone());
        let post = |p: &str, v: Value| request("POST", &format!("{}{p}", s.url()), Some("application/json"), Some(&v), None);

        // Page sans jeton → 404 ; avec jeton → 200 et en-têtes de sécurité.
        assert_eq!(request("GET", &format!("http://{}/", s.addr()), None, None, None).0, 404);
        let (code, _, csp) = request("GET", &s.url(), None, None, None);
        assert!(code == 200 && csp.is_some());

        // Liste des albums.
        let (code, v, _) = post("api/albums", json!({"server_url": fake.url, "api_key": KEY}));
        assert!(code == 200 && v.to_string().contains("Bassin"), "{code} {v}");
        let (code, v, _) = post("api/albums", json!({"server_url": fake.url, "api_key": "bad"}));
        assert!(code == 400 && v["error"].as_str().unwrap().contains("token"), "{code} {v}");

        // Enregistrement.
        let (code, v, _) = post(
            "api/save",
            json!({"server_url": fake.url, "api_key": KEY, "albums": [ALB2], "interval_minutes": 15, "style": "fit"}),
        );
        assert_eq!(code, 200, "{v}");
        assert_eq!(lock(&hooks.saved).as_ref().unwrap().style, "fit");
        // Nouvel enregistrement sans token → le token est conservé.
        let (code, v, _) = post("api/save", json!({"server_url": fake.url, "api_key": "", "albums": [ALB1, ALB2]}));
        assert_eq!(code, 200, "{v}");
        {
            let saved = lock(&hooks.saved);
            let saved = saved.as_ref().unwrap();
            assert!(saved.api_key == KEY && saved.albums.len() == 2);
        }
        // Le token n'est jamais renvoyé ; le nombre de photos masquées est indiqué.
        let (_, uc, _) = request("GET", &format!("{}api/config", s.url()), None, None, None);
        assert!(
            uc["api_key"] == "" && uc["has_key"] == true && uc["excluded_count"] == 3 && uc["preload"] == true,
            "{uc}"
        );
        assert!(uc.get("api_key_protected").is_none());
        // Album manquant → erreur.
        assert_eq!(post("api/save", json!({"server_url": fake.url, "albums": []})).0, 400);

        // Le token enregistré n'est jamais envoyé à un autre serveur que le sien.
        let leaked = Arc::new(AtomicBool::new(false));
        let other = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let other_url = format!("http://{}", other.server_addr());
        let l = leaked.clone();
        std::thread::spawn(move || {
            for req in other.incoming_requests() {
                if req.headers().iter().any(|h| h.field.equiv("x-api-key") && h.value.as_str() == KEY) {
                    l.store(true, Ordering::SeqCst);
                }
                let _ = req.respond(Response::from_string("[]"));
            }
        });
        for route in ["api/albums", "api/save"] {
            let (code, v, _) = post(route, json!({"server_url": other_url, "api_key": "", "albums": [ALB1]}));
            assert!(
                code == 400 && !leaked.load(Ordering::SeqCst),
                "{route} : token réutilisé pour un autre serveur ({code} {v})"
            );
        }

        // Durcissements : Host inattendu, méthode et type de contenu des routes d'action.
        assert_eq!(request("GET", &s.url(), None, None, Some("evil.example")).0, 403);
        assert_eq!(request("GET", &format!("{}api/next", s.url()), None, None, None).0, 405);
        assert_eq!(
            request("POST", &format!("{}api/next", s.url()), Some("text/plain"), Some(&json!({})), None).0,
            415
        );
        assert_eq!(post("api/next", json!({})).1["ok"], "ok");
        assert_eq!(
            request("GET", &format!("{}api/status", s.url()), None, None, None).1["status"],
            "prêt"
        );

        // Photos masquées : réaffichage.
        assert_eq!(post("api/clear-excluded", json!({})).0, 200);
        let (_, uc, _) = request("GET", &format!("{}api/config", s.url()), None, None, None);
        assert_eq!(uc["excluded_count"], 0);
    }
}
