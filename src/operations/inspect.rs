//! # Анализ SD-карты (Inspect)
//!
//! Собирает: таблицу разделов, геометрию EmuMMC, список игр. Работает только с
//! разрешёнными (removable) устройствами — безопасность гарантируется фильтром
//! из `disk::detect`.

use std::path::PathBuf;

use crate::disk::partition::{self, EmuGeometry, PartKind};
use crate::disk::BlockDevice;
use crate::fat32::games;

/// Результат полного анализа выбранной карты.
#[derive(Debug, Clone, Default)]
pub struct InspectResult {
    /// Найденный EmuMMC-раздел (геометрия), если есть.
    pub emummc: Option<EmuGeometry>,
    /// Список игр из `Nintendo/Contents/registered`.
    pub games: Vec<crate::manifest::GameEntry>,
    /// Путь к смонтированному FAT32 (или None).
    pub fat32_mount: Option<PathBuf>,
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
    let mut sum = String::new();
    if let Some(fp) = fat {
        let part_dev = crate::disk::partition_path(&dev.path, fp.index as u32);
        match mount_readonly(&part_dev) {
            Ok(mnt) => {
                result.games =
                    games::scan_registered(&mnt.join("Nintendo/Contents/registered"));
                result.fat32_mount = Some(mnt.clone());
                let size = result.games.iter().map(|g| g.size_bytes).sum::<u64>();
                sum = format!(
                    "Игр: {} ({} ГБ)",
                    result.games.len(),
                    crate::utils::human::human_bytes(size)
                );
                let _ = crate::utils::commands::run_capture("umount", &[mnt.to_str().unwrap()]);
            }
            Err(e) => {
                sum = format!("FAT32 недоступен для сканирования: {e}");
            }
        }
    }

    result.summary = sum;
    Ok(result)
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
