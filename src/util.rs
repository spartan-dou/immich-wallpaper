//! Erreurs, horloge locale, journal et petits utilitaires partagés.

use std::fmt;
use std::sync::{Mutex, MutexGuard};

/// Erreurs de l'appli. Les variantes nommées sont celles sur lesquelles le code prend des décisions.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// 401 : la clé elle-même est refusée (erreur globale).
    Unauthorized,
    /// 403 : la clé est valide mais n'a pas le droit d'accéder à cette ressource.
    Forbidden,
    /// Reapply ne peut pas couvrir les écrans actuels avec les photos mémorisées.
    NeedDraw,
    NoPrevious,
    Msg(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unauthorized => f.write_str("token refusé par Immich (401) : vérifie api_key"),
            Error::Forbidden => f.write_str("accès refusé par Immich (403) : vérifie les permissions du token"),
            Error::NeedDraw => f.write_str("pas assez de photos mémorisées pour les écrans actuels"),
            Error::NoPrevious => f.write_str("aucune photo précédente en mémoire"),
            Error::Msg(s) => f.write_str(s),
        }
    }
}

impl Error {
    pub fn is_auth(&self) -> bool {
        matches!(self, Error::Unauthorized | Error::Forbidden)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Msg(s)
    }
}
impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Msg(s.to_string())
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Msg(e.to_string())
    }
}

pub type Res<T> = Result<T, Error>;

pub fn err<T>(s: impl Into<String>) -> Res<T> {
    Err(Error::Msg(s.into()))
}

/// Verrouille un mutex même si un thread a paniqué en le tenant (l'état reste utilisable).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(data))
}

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Tronque à n caractères (pas octets) en terminant par « … ».
pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Message d'un panic récupéré.
pub fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

// ---------- horloge locale ----------

/// Date et heure locales (suffisant pour la planification, la légende et le journal).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Local {
    pub year: i32,
    pub month: u32, // 1..=12
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

#[cfg(windows)]
pub fn now_local() -> Local {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    Local {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
    }
}

/// Hors Windows (développement, tests) : heure UTC.
#[cfg(not(windows))]
pub fn now_local() -> Local {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Algorithme « civil from days » (H. Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    Local {
        year: y as i32,
        month: m as u32,
        day: d as u32,
        hour: (rem / 3600) as u32,
        minute: (rem % 3600 / 60) as u32,
        second: (rem % 60) as u32,
    }
}

// ---------- journal ----------

static LOGGER: Mutex<Option<crate::logfile::RotatingLog>> = Mutex::new(None);

/// Dirige le journal vers un fichier (sinon : sortie d'erreur, utile en test).
pub fn set_log_file(l: crate::logfile::RotatingLog) {
    *lock(&LOGGER) = Some(l);
}

pub fn log_line(msg: &str) {
    let t = now_local();
    let line = format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02} {}\n",
        t.year, t.month, t.day, t.hour, t.minute, t.second, msg
    );
    let mut g = lock(&LOGGER);
    match g.as_mut() {
        Some(l) => {
            let _ = l.write(line.as_bytes());
        }
        None => {
            if cfg!(test) || cfg!(not(windows)) {
                eprint!("{line}");
            }
        }
    }
}

#[macro_export]
macro_rules! logf {
    ($($arg:tt)*) => { $crate::util::log_line(&format!($($arg)*)) };
}
