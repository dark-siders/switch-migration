//! # Сканер установленных игр и сохранений
//!
//! Анализирует структуру FAT32-раздела:
//! - `Nintendo/Contents/registered/<titleid>/` — установленные игры, обновления
//!   и DLC (каталоги с 16-hex именами contenta/contentb/...).
//! - `sdmc:/emummc/` — путь к EmuMMC (для корректного поиска).
//!
//! Игры перечисляются по именам каталогов (title IDs) внутри registered.
//! Сохранения лежат не в FAT32, а внутри EmuMMC (в eMMC пользовательской
//! области), поэтому здесь мы лишь сообщаем пользователю об их наличии —
//! они переносятся вместе с RAW-образом.

use std::fs;
use std::path::Path;

use crate::manifest::GameEntry;

/// Парсит «registered» каталог и возвращает список игр.
///
/// `registered_dir` обычно: `<mount>/Nintendo/Contents/registered`.
pub fn scan_registered(registered_dir: &Path) -> Vec<GameEntry> {
    let mut games = Vec::new();
    let Ok(rd) = fs::read_dir(registered_dir) else {
        return games;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // title ID: 16 hex-цифр (иногда с расширением .nca внутри, но каталог — hex).
        if !is_title_id(&name) {
            continue;
        }
        let size = dir_size(&entry.path()).unwrap_or(0);
        games.push(GameEntry::new(name.clone(), entry.path().display().to_string(), size));
    }
    games.sort_by(|a, b| a.title_id.cmp(&b.title_id));
    games
}

/// True, если строка — 16-значный hex ключ (title ID файла игры).
pub fn is_title_id(name: &str) -> bool {
    let name = name.trim_end_matches('/');
    if name.len() != 16 {
        return false;
    }
    name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Рекурсивный подсчёт размера каталога.
pub fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p)?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}

/// Ищет EmuMMC-каталог внутри sdmc (обычно `emummc/`).
#[allow(dead_code)]
pub fn locate_emummc_dir(sdmc_root: &Path) -> Option<std::path::PathBuf> {
    let candidates = ["emummc", "emuMMC", "EmuMMC"];
    for c in candidates {
        let p = sdmc_root.join(c);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_ids() {
        assert!(is_title_id("0100F2C0115B6000"));
        assert!(!is_title_id("short"));
        assert!(!is_title_id("0100F2C0115B6000X"));
        assert!(!is_title_id("__MACOSX"));
    }
}
