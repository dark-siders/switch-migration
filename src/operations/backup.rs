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
use crate::manifest::{BackupManifest, EmuSection, Fat32Section, GameEntry};

use super::inspect::mount_readonly;

/// План, подготовленный перед началом копирования.
#[derive(Debug, Clone)]
pub struct BackupPlan {
    pub source: BlockDevice,
    pub emu: Option<EmuGeometry>,
    pub out_dir: PathBuf,
    pub games: Vec<GameEntry>,
}

/// Папка по умолчанию для бэкапа (переживающая перезапуск, в временной папке ОС).
pub fn default_backup_dir() -> PathBuf {
    std::env::temp_dir().join(format!("switch-migrator-backup-{}", std::process::id()))
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
pub fn run_backup(plan: BackupPlan) -> Result<BackupResult, String> {
    let out = &plan.out_dir;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;

    // 1. Дамп EmuMMC
    let mut emu_image = None;
    let mut img_bytes = 0u64;
    if let Some(emu) = &plan.emu {
        // Определяем устройство раздела №2 — это наш EmuMMC-раздел (после FAT32).
        let part_dev = crate::disk::partition_path(&plan.source.path, 2);
        if part_dev.exists() {
            let img_path = out.join(format!("emummc.img.{}.zst", emu.emu_start));
            let progress = imaging::ProgressHandle::none();
            let done = imaging::dump_emummc(
                &part_dev,
                0,
                emu.emu_len,
                &img_path,
                &progress,
            )
            .map_err(|e| format!("dump: {e}"))?;
            img_bytes = done;
            emu_image = Some(img_path);
        }
    }

    // 2. Копирование FAT32-данных
    let data_dir = out.join("sdmc");
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;

    // Монтируем FAT32 RO и копируем ключевые папки.
    let fat_part = crate::disk::partition_path(&plan.source.path, 1);
    if fat_part.exists() {
        match mount_readonly(&fat_part) {
            Ok(mnt) => {
                copy_tree(&mnt, &data_dir).map_err(|e| format!("copy sdmc: {e}"))?;
                let _ = crate::utils::commands::run_capture("umount", &[mnt.to_str().unwrap()]);
            }
            Err(e) => {
                return Err(format!("Не удалось смонтировать FAT32: {e}"));
            }
        }
    }

    // 3. Манифест
    let emu_section = plan
        .emu
        .as_ref()
        .map(|e| EmuSection {
            start_sector: e.emu_start,
            num_sectors: e.emu_len,
            size_bytes: e.emu_bytes,
        })
        .unwrap_or(EmuSection {
            start_sector: 0,
            num_sectors: 0,
            size_bytes: 0,
        });
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

/// Копирует содержимое одного каталога в другой (простая рекурсия).
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let dst_p = dst.join(entry.file_name());
        if p.is_dir() {
            std::fs::create_dir_all(&dst_p)?;
            copy_tree(&p, &dst_p)?;
        } else {
            if entry.metadata()?.is_symlink() {
                continue;
            }
            crate::emummc::imaging::copy_stream(&p, &dst_p)?;
        }
    }
    Ok(())
}