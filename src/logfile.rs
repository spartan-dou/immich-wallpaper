//! Journal avec rotation : au-delà de `max` octets, journal.txt devient journal.1.txt
//! (l'historique précédent reste consultable).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct RotatingLog {
    path: PathBuf,
    max: u64,
    file: Option<File>,
    size: u64,
}

impl RotatingLog {
    pub fn open(path: &Path, max: u64) -> std::io::Result<Self> {
        let mut l = RotatingLog {
            path: path.to_path_buf(),
            max,
            file: None,
            size: 0,
        };
        l.reopen()?;
        Ok(l)
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        let f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.size = f.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(f);
        Ok(())
    }

    pub fn backup_path(&self) -> PathBuf {
        let s = self.path.to_string_lossy();
        PathBuf::from(format!("{}.1.txt", s.strip_suffix(".txt").unwrap_or(&s)))
    }

    pub fn write(&mut self, p: &[u8]) -> std::io::Result<()> {
        if self.file.is_some() && self.size > 0 && self.size + p.len() as u64 > self.max {
            self.file = None; // ferme le fichier avant de le renommer
            let _ = std::fs::remove_file(self.backup_path());
            let _ = std::fs::rename(&self.path, self.backup_path());
        }
        if self.file.is_none() {
            self.reopen()?;
        }
        let f = self.file.as_mut().unwrap();
        f.write_all(p)?;
        self.size += p.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation() {
        let dir = crate::testutil::temp_dir("log");
        let path = dir.join("journal.txt");
        let mut l = RotatingLog::open(&path, 100).unwrap();
        let line = format!("{}\n", "x".repeat(39));
        for _ in 0..5 {
            l.write(line.as_bytes()).unwrap();
        }
        let cur = std::fs::read(&path).unwrap();
        let old = std::fs::read(l.backup_path()).unwrap();
        assert!(!cur.is_empty() && cur.len() <= 100 && !old.is_empty() && old.len() <= 100);
    }
}
