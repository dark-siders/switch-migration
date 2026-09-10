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
use std::path::{Path, PathBuf};

use crate::manifest::{GameEntry, HomebrewEntry};

use super::titles;

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
        let mut g = GameEntry::new(name.clone(), entry.path().display().to_string(), size);
        g.name = titles::lookup(&name).unwrap_or_default();
        games.push(g);
    }
    games.sort_by(|a, b| a.title_id.cmp(&b.title_id));
    games
}

/// True, если строка — 16-значный hex ключ (title ID файла игры).
fn is_title_id(name: &str) -> bool {
    let name = name.trim_end_matches('/');
    if name.len() != 16 {
        return false;
    }
    name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Сканирует homebrew-приложения: каталоги `switch/<app>/` c `.nro` внутри
/// (плюс `.nro` прямо в корне `switch/`).
pub fn scan_homebrew(sdmc_root: &Path) -> Vec<HomebrewEntry> {
    use std::fs;
    let switch_dir = sdmc_root.join("switch");
    let mut apps = Vec::new();
    let Ok(rd) = fs::read_dir(&switch_dir) else {
        return apps;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if let Some(nro) = find_nro(&p) {
                let size = fs::metadata(&nro).map(|m| m.len()).unwrap_or(0);
                apps.push(HomebrewEntry::new(
                    name,
                    nro.display().to_string(),
                    size,
                ));
            }
        } else if name.ends_with(".nro") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            apps.push(HomebrewEntry::new(
                name.trim_end_matches(".nro").to_string(),
                p.display().to_string(),
                size,
            ));
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

/// Ищет `.nro` в каталоге приложения (обычно `<app>/<app>.nro`);
/// на всякий случай берём первый найденный.
fn find_nro(dir: &Path) -> Option<PathBuf> {
    let nro_same = {
        let name = dir.file_name().map(|s| s.to_string_lossy().to_string())?;
        let mut p = std::path::PathBuf::from(dir);
        p.push(format!("{name}.nro"));
        p
    };
    if nro_same.is_file() {
        return Some(nro_same);
    }
    let Ok(rd) = fs::read_dir(dir) else { return None };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().map(|e| e == "nro").unwrap_or(false) {
            return Some(p);
        }
    }
    None
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

/// Сканирует игры на FAT32-корне SD-карты.
///
/// На file-based (`raw_based`) EmuMMC каталог Nintendo лежит не в корне, а по
/// пути `nintendo_path` из `emummc.ini` (например `emuMMC/RAW1/Nintendo`).
/// Учитываем оба места, дубликаты отсекаем по title ID (приоритет — ini).
pub fn scan_nintendo(sdmc_root: &Path) -> Vec<GameEntry> {
    let mut seen: std::collections::HashMap<String, GameEntry> =
        std::collections::HashMap::new();

    // 1. Путь из emummc.ini (nintendo_path), если файл читается.
    if let Some(emu_dir) = locate_emummc_dir(sdmc_root) {
        if let Ok(ini) = crate::emummc::ini::EmummcIni::load(&emu_dir.join("emummc.ini")) {
            if let Some(np) = ini.get("nintendo_path") {
                let dir = sdmc_root.join(np).join("Contents/registered");
                for g in scan_registered(&dir) {
                    seen.entry(g.title_id.clone()).or_insert(g);
                }
            }
        }
    }

    // 2. Классический корневой `Nintendo`.
    for g in scan_registered(&sdmc_root.join("Nintendo/Contents/registered")) {
        seen.entry(g.title_id.clone()).or_insert(g);
    }

    let mut games: Vec<GameEntry> = seen.into_values().collect();
    games.sort_by(|a, b| a.title_id.cmp(&b.title_id));
    games
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn title_ids() {
        assert!(is_title_id("0100F2C0115B6000"));
        assert!(!is_title_id("short"));
        assert!(!is_title_id("0100F2C0115B6000X"));
        assert!(!is_title_id("__MACOSX"));
    }

    #[test]
    fn scans_both_root_and_raw1_nintendo() {
        let dir = std::env::temp_dir().join(format!("sm-games-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Nintendo/Contents/registered")).unwrap();
        std::fs::create_dir_all(dir.join("emuMMC/RAW1/Nintendo/Contents/registered"))
            .unwrap();
        // корневой registered — пустой (как на реальной карте)
        // игры — в RAW1 (raw_based), дубликат отсекается
        std::fs::create_dir_all(dir.join("emuMMC/RAW1/Nintendo/Contents/registered/0100000000000001")).unwrap();
        std::fs::create_dir_all(dir.join("emuMMC/RAW1/Nintendo/Contents/registered/0100000000000002")).unwrap();
        std::fs::create_dir_all(dir.join("Nintendo/Contents/registered/0100000000000001")).unwrap();

        // настоящий emummc.ini с nintendo_path
        let ini = dir.join("emuMMC/emummc.ini");
        let mut f = std::fs::File::create(&ini).unwrap();
        f.write_all(
            b"[emummc]\nenabled=1\nsector=0xdae8000\npath=emuMMC/RAW1\nid=0x31574152\nnintendo_path=emuMMC/RAW1/Nintendo\n",
        )
        .unwrap();

        let games = scan_nintendo(&dir);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].title_id, "0100000000000001");
        assert_eq!(games[1].title_id, "0100000000000002");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falls_back_to_root_nintendo_without_ini() {
        let dir = std::env::temp_dir().join(format!("sm-games-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Nintendo/Contents/registered/0100F2C0115B6000"))
            .unwrap();

        let games = scan_nintendo(&dir);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title_id, "0100F2C0115B6000");
        // известный title_id подставил название из таблицы
        assert_eq!(
            games[0].name,
            "The Legend of Zelda: Skyward Sword HD"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scans_homebrew_apps() {
        let dir = std::env::temp_dir().join(format!("sm-hb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("switch/App1")).unwrap();
        std::fs::create_dir_all(dir.join("switch/App2")).unwrap();
        std::fs::write(dir.join("switch/App1/App1.nro"), vec![0u8; 42]).unwrap();
        std::fs::write(dir.join("switch/App2/App2.nro"), vec![0u8; 7]).unwrap();
        std::fs::write(dir.join("switch/Tool.nro"), vec![0u8; 3]).unwrap();

        let apps = scan_homebrew(&dir);
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].name, "App1");
        assert_eq!(apps[0].size_bytes, 42);
        assert_eq!(apps[1].name, "App2");
        assert_eq!(apps[2].name, "Tool");

        std::fs::remove_dir_all(&dir).ok();
    }
}
