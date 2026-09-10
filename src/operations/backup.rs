//! # Операция «Бэкап» — полный слепок карты
//!
//! План действий:
//! 1. Дамп RAW-раздела EmuMMC в `.img.zst` (сжатие zstd на лету).
//! 2. Копирование FAT32 (папка Nintendo, Hekate, Atmosphere, bootloader).
//! 3. Генерация `manifest.json`.
//!
//! Выход — папка бэкапа с единой структурой.

use std::path::{Path, PathBuf};

use crate::disk::partition::EmuGeometry;
use crate::disk::BlockDevice;
use crate::emummc::imaging;
use crate::manifest::{BackupManifest, EmuSection, Fat32Section, GameEntry, HomebrewEntry};
use crate::utils::progress::{dir_size, Progress, ProgressSink};

use super::inspect::mount_fat32;

/// План, подготовленный перед началом копирования.
#[derive(Debug, Clone)]
pub struct BackupPlan {
    pub source: BlockDevice,
    pub emu: Option<EmuGeometry>,
    pub out_dir: PathBuf,
    pub games: Vec<GameEntry>,
    pub homebrew: Vec<HomebrewEntry>,
}

/// Итог бэкапа.
#[derive(Debug)]
pub struct BackupResult {
    pub manifest_path: PathBuf,
    pub emummc_image: Option<PathBuf>,
    #[allow(dead_code)] // data_dir востребован для копий sdmc
    pub data_dir: PathBuf,
    pub total_bytes: u64,
}

impl std::fmt::Display for BackupResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Бэкап: {} (образ EmuMMC: {}, данных: {})",
            self.manifest_path.display(),
            self.emummc_image
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "нет".to_string()),
            self.total_bytes
        )
    }
}

/// Главная функция: выполняет бэкап по плану.
///
/// `sink` — куда писать прогресс в % (stderr для CLI, канал для TUI).
pub fn run_backup(plan: BackupPlan, sink: ProgressSink) -> Result<BackupResult, String> {
    let out = &plan.out_dir;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;

    // 1. Копирование FAT32: данные sdmc + emummc.ini, из которого ниже
    // определяем РЕАЛЬНОЕ окно EmuMMC (данные могут начинаться не с начала
    // раздела — Hekate резервирует заголовок/выравнивание).
    let data_dir = out.join("sdmc");
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

    let fat_part = crate::disk::partition_path(&plan.source.path, 1);
    if fat_part.exists() {
        match mount_fat32(&fat_part) {
            Ok((mnt, owned)) => {
                let total = dir_size(&mnt);
                let mut progress = Progress::new(sink.clone(), total, "Бэкап sdmc (FAT32)");
                copy_tree(&mnt, &data_dir, &mut progress)
                    .map_err(|e| format!("copy sdmc: {e}"))?;
                progress.finish();
                if owned {
                    let _ = crate::utils::commands::run_capture("umount", &[mnt.to_str().unwrap()]);
                }
            }
            Err(e) => {
                return Err(format!("Не удалось смонтировать FAT32: {e}"));
            }
        }
    }

    // 2. Дамп RAW-образа EmuMMC.
    let mut emu_image = None;
    let mut img_bytes = 0u64;
    let emu_section = if let Some(emu) = &plan.emu {
        let (data_start, data_len) = resolve_dump_window(emu, &data_dir);
        // Раздел №2 — наш EmuMMC-раздел (после FAT32).
        let part_dev = crate::disk::partition_path(&plan.source.path, 2);
        if part_dev.exists() {
            let img_path = out.join(format!("emummc.img.{data_start}.zst"));
            let offset = data_start - emu.emu_start;
            let total = data_len * 512;
            let mut progress = Progress::new(sink, total, "Бэкап EmuMMC (RAW → zstd)");
            let done =
                imaging::dump_emummc(&part_dev, offset, data_len, &img_path, &mut progress)
                    .map_err(|e| format!("dump: {e}"))?;
            progress.finish();
            img_bytes = done;
            emu_image = Some(img_path);
        }
        EmuSection {
            start_sector: data_start,
            num_sectors: data_len,
            size_bytes: data_len * 512,
        }
    } else {
        EmuSection {
            start_sector: 0,
            num_sectors: 0,
            size_bytes: 0,
        }
    };

    // 3. Манифест
    let fat32_section = Fat32Section {
        start_sector: plan.emu.as_ref().map(|e| e.fat_start).unwrap_or(0),
        num_sectors: 0,
        size_bytes: plan.source.size_bytes - emu_section.size_bytes,
    };

    let mut manif = BackupManifest::new(
        plan.source.size_bytes,
        emu_section,
        fat32_section,
        plan.games.clone(),
    );
    manif.homebrew = plan.homebrew.clone();
    manif.image_zst_bytes = img_bytes;
    let mpath = out.join("manifest.json");
    manif.save(&mpath).map_err(|e| e.to_string())?;

    Ok(BackupResult {
        manifest_path: mpath,
        emummc_image: emu_image,
        data_dir,
        total_bytes: img_bytes,
    })
}

/// Возвращает окно RAW-дампа EmuMMC: `(стартовый_сектор, длина_в_секторах)`.
///
/// Реальные данные EmuNAND могут начинаться НЕ с начала раздела, а с сектора,
/// указанного в `emuMMC/emummc.ini` — Hekate резервирует до 16 МиБ
/// заголовка/выравнивания. Без ini (или при секторе вне границ раздела)
/// дампим весь раздел целиком.
fn resolve_dump_window(emu: &EmuGeometry, sdmc: &Path) -> (u64, u64) {
    let end = emu.emu_start + emu.emu_len;
    let ini_paths: [PathBuf; 2] = [
        sdmc.join("emuMMC/emummc.ini"),
        sdmc.join("emummc/emummc.ini"),
    ];
    let ini_sector = ini_paths
        .iter()
        .find_map(|p| crate::emummc::ini::EmummcIni::load(p).ok())
        .map(|ini| ini.sector);
    match ini_sector {
        Some(s) if s >= emu.emu_start && s < end => (s, end - s),
        _ => (emu.emu_start, emu.emu_len),
    }
}

/// Копирует содержимое одного каталога в другой (простая рекурсия).
///
/// Отчитывается о прогресс в `Progress` по мере копирования.
fn copy_tree(src: &Path, dst: &Path, progress: &mut Progress) -> std::io::Result<u64> {
    let mut done = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let dst_p = dst.join(entry.file_name());
        if p.is_dir() {
            std::fs::create_dir_all(&dst_p)?;
            done += copy_tree(&p, &dst_p, progress)?;
        } else {
            if entry.metadata()?.is_symlink() {
                continue;
            }
            done += crate::emummc::imaging::copy_stream(&p, &dst_p)?;
            progress.report(done);
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu() -> EmuGeometry {
        EmuGeometry {
            fat_start: 2048,
            emu_start: 1_000_000,
            emu_len: 1_000_000,
            emu_bytes: 512_000_000,
        }
    }

    #[test]
    fn dump_window_falls_back_without_ini() {
        let dir = std::env::temp_dir().join("sm-bk-test-no-ini");
        let (start, len) = resolve_dump_window(&emu(), &dir);
        assert_eq!((start, len), (1_000_000, 1_000_000));
    }

    #[test]
    fn dump_window_uses_ini_sector_inside_partition() {
        let dir = std::env::temp_dir().join("sm-bk-test-good-ini");
        let ini = dir.join("emuMMC/emummc.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        std::fs::write(&ini, "[emummc]\nenabled=1\nsector=1000704\n").unwrap();
        let (start, len) = resolve_dump_window(&emu(), &dir);
        assert_eq!(start, 1_000_704);
        assert_eq!(len, 2_000_000 - 1_000_704);
    }

    #[test]
    fn dump_window_ignores_ini_sector_outside_partition() {
        let dir = std::env::temp_dir().join("sm-bk-test-bad-ini");
        let ini = dir.join("emuMMC/emummc.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        // 2_000_000 == конец раздела — вне допустимого окна.
        std::fs::write(&ini, "[emummc]\nsector=2000000\n").unwrap();
        let (start, len) = resolve_dump_window(&emu(), &dir);
        assert_eq!((start, len), (1_000_000, 1_000_000));
    }
}