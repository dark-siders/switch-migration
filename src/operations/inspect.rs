//! # Анализ SD-карты (Inspect)
//!
//! Собирает: таблицу разделов, геометрию EmuMMC, список игр. Работает только с
//! разрешёнными (removable) устройствами — безопасность гарантируется фильтром
//! из `disk::detect`.

use std::path::PathBuf;

use crate::disk::partition::{self, EmuGeometry, PartKind};
use crate::disk::BlockDevice;
use crate::fat32::games;
use crate::manifest::HomebrewEntry;

/// Результат полного анализа выбранной карты.
#[derive(Debug, Clone, Default)]
pub struct InspectResult {
    /// Найденный EmuMMC-раздел (геометрия), если есть.
    pub emummc: Option<EmuGeometry>,
    /// Список игр из `Nintendo/Contents/registered`.
    pub games: Vec<crate::manifest::GameEntry>,
    /// Список homebrew-приложений из `/switch/`.
    #[allow(clippy::vec_box)]
    pub homebrew: Vec<HomebrewEntry>,
    /// Путь к смонтированному FAT32 (или None).
    pub fat32_mount: Option<PathBuf>,
    /// Человекочитаемый итог сканирования/ошибки FAT32 (для логов и TUI).
    pub scan_msg: String,
    /// Сводка для лога.
    pub summary: String,
}

/// Выполняет анализ карты.
pub fn inspect_disk(dev: &BlockDevice) -> Result<InspectResult, String> {
    // 1. Таблица разделов (MBR), читаем напрямую.
    let table = partition::parse_mbr(&dev.path).map_err(|e| e.to_string())?;

    // 2. Находим разделы: FAT32 и EmuMMC (RAW).
    let fat = table.find(PartKind::Fat32);
    // Для EmuMMC нам нужно найти НЕ-FAT32 раздел. По умолчанию — p2 после p1.
    // Пробуем «сырой» раздел: тот, что идёт сразу после FAT32 и не распознан как ФС.
    let emu_part = table
        .partitions
        .iter()
        .find(|p| p.kind != PartKind::Fat32 && p.start > 0)
        .cloned();

    let mut result = InspectResult::default();

    // 3. Геометрия EmuMMC.
    if let Some(ep) = &emu_part {
        result.emummc = Some(EmuGeometry {
            fat_start: fat.map(|f| f.start).unwrap_or(0),
            emu_start: ep.start,
            emu_len: ep.length,
            emu_bytes: ep.size_bytes(),
        });
    }

    // 4. Сканирование игр: монтируем FAT32 по-требованию (RO).
    let mut scan = String::new();
    if let Some(fp) = fat {
        let part_dev = crate::disk::partition_path(&dev.path, fp.index as u32);
        match mount_fat32(&part_dev) {
            Ok((mnt, owned)) => {
                result.games = games::scan_nintendo(&mnt);
                result.homebrew = games::scan_homebrew(&mnt);
                result.fat32_mount = Some(mnt.clone());
                let gsize = result.games.iter().map(|g| g.size_bytes).sum::<u64>();
                scan = format!(
                    "Игр: {} (всего {}), homebrew: {}",
                    result.games.len(),
                    crate::utils::human::human_bytes(gsize),
                    result.homebrew.len()
                );
                // Размонтируем только то, что смонтировали сами (чужую точку
                // из /run/media трогать нельзя).
                if owned {
                    let _ = crate::utils::commands::run_capture("umount", &[mnt.to_str().unwrap()]);
                }
            }
            Err(e) => {
                scan = format!("FAT32 недоступен для сканирования: {e}");
            }
        }
    } else {
        scan = "FAT32-раздел не найден".to_string();
    }
    result.scan_msg = scan.clone();
    result.summary = scan;

    Ok(result)
}

/// Возвращает точку монтирования FAT32 для сканирования.
///
/// Если раздел уже смонтирован (например, GUI-демоном в `/run/media/...`),
/// используем существующую точку и помечаем `owned=false` (размонтировать её
/// нельзя). Иначе монтируем сами в RO как `owned=true`.
pub(crate) fn mount_fat32(dev: &std::path::Path) -> Result<(PathBuf, bool), String> {
    if let Some(mnt) = existing_mount(dev) {
        return Ok((mnt, false));
    }
    mount_readonly(dev).map(|mnt| (mnt, true))
}

/// Ищет уже смонтированную точку для устройства через `/proc/self/mountinfo`.
fn existing_mount(dev: &std::path::Path) -> Option<PathBuf> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let dev_s = dev.to_str()?;
    for line in mountinfo.lines() {
        let (left, right) = line.split_once(" - ")?;
        let mut rparts = right.split_whitespace();
        let _fstype = rparts.next()?;
        let src = rparts.next()?;
        if src != dev_s {
            continue;
        }
        // Mount point — 5-е поле слева от разделителя.
        let mp = left.split_whitespace().nth(4)?;
        let path = PathBuf::from(unescape_octal(mp));
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

/// `mountinfo` кодирует спецсимволы как `\040` (октально) — раскодируем.
fn unescape_octal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Some(v) =
                std::str::from_utf8(&bytes[i + 1..i + 4])
                    .ok()
                    .and_then(|t| u8::from_str_radix(t, 8).ok())
            {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Монтирует блочное устройство в RO во временную директорию.
pub fn mount_readonly(dev: &std::path::Path) -> Result<PathBuf, String> {
    let mnt = std::env::temp_dir().join(format!("sm-mnt-{}", std::process::id()));
    std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
    crate::utils::commands::run_capture(
        "mount",
        &["-o", "ro", dev.to_str().ok_or("bad path")?, mnt.to_str().ok_or("bad")?],
    )
    .map_err(|e| format!("mount: {e}"))?;
    Ok(mnt)
}
