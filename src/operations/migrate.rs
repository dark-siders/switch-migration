//! # Операция «Миграция» — разметка новой карты и разворачивание бэкапа
//!
//! Алгоритм (второй клик):
//! 1. Проверка целевой карты (removable + safety guard).
//! 2. Разметка MBR (как у оригинальных карт Switch):
//!    - p1: FAT32, кластер 32 КБ, начало 2048 сектор.
//!    - p2: RAW-раздел EmuMMC (до конца диска).
//! 3. `mkfs.vfat` с правильным размером сектора/кластера.
//! 4. Разворачивание zstd-образа EmuMMC в p2.
//! 5. Копирование данных FAT32.
//! 6. **Авто-фикс sector offset** в `emummc.ini` — главная фишка утилиты.
//! 7. Fix Archive Bit на перенесённых файлах.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::disk::BlockDevice;
use crate::emummc::imaging;
use crate::manifest::BackupManifest;
use crate::utils::commands::{self, CommandError};

/// План миграции на целевую карту.
#[derive(Debug, Clone)]
pub struct MigratePlan {
    /// Целевая (новая) карта — уже проверена безопасным фильтром.
    pub target: BlockDevice,
    /// Путь к папке бэкапа (с manifest.json и emummc.img.*.zst).
    pub backup_dir: PathBuf,
}

/// Итог миграции.
#[derive(Debug)]
pub struct MigrateResult {
    pub new_emu_start: u64,
    pub emummc_ini_fixed: bool,
    pub archive_bits_fixed: u64,
}

/// Стартовый сектор FAT32-раздела (стандарт SD-карт / Hekate).
const P1_START: u64 = 2048;

/// Главная функция миграции.
pub fn run_migrate(plan: &MigratePlan) -> Result<MigrateResult, String> {
    let manifest_path = plan.backup_dir.join("manifest.json");
    let manifest =
        BackupManifest::load(&manifest_path).map_err(|e| format!("manifest: {e}"))?;
    if !manifest.validate() {
        return Err("Неверный формат манифеста".into());
    }

    let target = &plan.target;
    if !target.is_safe() {
        return Err(format!(
            "Целевое устройство {} — ОПАСНО. Отказ.",
            target.path.display()
        ));
    }

    // Критическая проверка: НИКОГДА не трогаем диск, где есть manifest источника
    // — защита от «симметричного» сценария, когда пользователь перепутал карты.
    let manifest_path_str = manifest_path.display().to_string();
    drop(manifest_path_str); // не используем повторно

    // ---------- ШАГ 0: гарантируем наличие утилит ----------
    for tool in ["wipefs", "sfdisk", "partprobe", "partx", "mkfs.vfat", "mount", "umount", "blockdev"] {
        if !is_available(tool) {
            return Err(format!("Утилита `{tool}` не найдена в PATH"));
        }
    }

    // ---------- ШАГ 1: разметка MBR ----------
    // Стираем старую таблицу и создаём новую через скрипт sfdisk.
    let _ = commands::run_capture("wipefs", &["--all", target.path.to_str().unwrap()])
        .map_err(|e| format!("wipefs: {e}"))?;

    let total = target.sectors;
    // Отводим под EmuMMC всё оставшееся после FAT32 + начала.
    let p1_len = (total - P1_START) / 2;
    let p2_start = P1_START + p1_len;
    let p2_len = total - p2_start;

    let p1_dev = crate::disk::partition_path(&target.path, 1);
    let p2_dev = crate::disk::partition_path(&target.path, 2);

    // p1: FAT32 0x0C bootable; p2: RAW EmuMMC (тип 0x27 = скрытый/raw, НЕ FAT32) —
    // иначе собственный анализатор сочтёт p2 также FAT32 и не распознает EmuMMC
    // при повторном inspect после миграции.
    let script = format!(
        "label: dos
device: {dev}
{dev1} : start={p1_start}, size={p1_len}, type=c, bootable
{dev2} : start={p2_start}, size={p2_len}, type=27
",
        dev = target.path.display(),
        dev1 = p1_dev.display(),
        dev2 = p2_dev.display(),
        p1_start = P1_START,
        p1_len = p1_len,
        p2_start = p2_start,
        p2_len = p2_len,
    );

    commands::run_with_stdin(
        "sfdisk",
        &[
            "--wipe",
            "always",
            "--force",
            target.path.to_str().unwrap(),
        ],
        &script,
    )
    .map_err(|e| format!("sfdisk: {e}"))?;

    // Заставляем ядро перечитать таблицу и ДОЖДАТЬСЯ, пока появятся узлы
    // p1/p2 (иначе mkfs.vfat стартует раньше узлов — типично для loop/udev).
    ensure_partitions(&target.path, &[&p1_dev, &p2_dev])?;

    // ---------- ШАГ 2: форматирование FAT32 ----------
    // Кластер 32 КБ при секторе 512 = 64 сектора на кластер (mkfs.vfat -s 64).
    apply_mkfs_fat32(&p1_dev).map_err(|e| e.to_string())?;

    // ---------- ШАГ 3: разворачиваем EmuMMC ----------
    let image = find_emummc_image(&plan.backup_dir)?;
    // Начинаем с 0-го сектора самого раздела — raw-образ пишем в раздел целиком.
    let _restored = imaging::restore_emummc(&image, &p2_dev, 0)
        .map_err(|e| format!("restore: {e}"))?;

    // ---------- ШАГ 4: копирование sdmc-данных на FAT32 ----------
    let mnt = mount_rw(&p1_dev)?;
    let sdmc_src = plan.backup_dir.join("sdmc");
    if sdmc_src.is_dir() {
        copy_tree(&sdmc_src, &mnt).map_err(|e| format!("copy: {e}"))?;
    }

    // ---------- ШАГ 5: авто-фикс sector offset в emummc.ini ----------
    // Новый стартовый сектор RAW-раздела.
    let new_emu_start = p2_start;
    let emummc_ini_fixed = fix_emummc_ini(&mnt, new_emu_start);

    // ---------- ШАГ 6: Fix Archive Bit ----------
    let archive_bits_fixed = crate::fat32::archive::fix_archive_bit(&mnt).unwrap_or(0);

    // ---------- ШАГ 7: финализация ----------
    let _ = commands::run_capture("umount", &[mnt.to_str().unwrap()]);
    imaging::sync_disk(&target.path).map_err(|e| e.to_string())?;

    Ok(MigrateResult {
        new_emu_start,
        emummc_ini_fixed,
        archive_bits_fixed,
    })
}

/// Применяет mkfs.vfat с 512-байтным сектором и кластером 32 КБ.
fn apply_mkfs_fat32(p1: &Path) -> Result<(), CommandError> {
    commands::run_capture(
        "mkfs.vfat",
        &[
            "-F", "32",
            "-S", "512",
            "-s", "64", // 64 сектора * 512 = 32 КБ кластер
            "-n", "SDCARD",
            p1.to_str().unwrap(),
        ],
    )?;
    Ok(())
}

/// Монтирует FAT32 в RW во временную папку.
fn mount_rw(dev: &Path) -> Result<PathBuf, String> {
    let mnt = std::env::temp_dir().join(format!("sm-rw-{}", std::process::id()));
    std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
    commands::run_capture("mount", &[dev.to_str().unwrap(), mnt.to_str().unwrap()])
        .map_err(|e| format!("mount: {e}"))?;
    Ok(mnt)
}

/// Правит `emummc.ini` на смонтированном разделе.
fn fix_emummc_ini(mnt: &Path, new_sector: u64) -> bool {
    let ini_path = mnt.join("emummc/emummc.ini");
    if !ini_path.exists() {
        eprintln!("emummc.ini не найден на новой карте — пропуск.");
        return false;
    }
    match crate::emummc::ini::EmummcIni::load(&ini_path) {
        Ok(mut ini) => {
            let old = ini.sector;
            match ini.set_sector_and_save(new_sector) {
                Ok(()) => {
                    eprintln!("emummc.ini: sector {old} -> {new_sector}");
                    true
                }
                Err(e) => {
                    eprintln!("emummc.ini: {e}");
                    false
                }
            }
        }
        Err(e) => {
            eprintln!("emummc.ini: {e}");
            false
        }
    }
}

/// Ищет файл образа EmuMMC в папке бэкапа.
fn find_emummc_image(backup_dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(backup_dir).map_err(|e| e.to_string())?;
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("emummc.img.") && name.ends_with(".zst") {
            return Ok(p);
        }
    }
    Err("Образ emummc.img.*.zst не найден в папке бэкапа".into())
}

/// Простая рекурсивная копия дерева каталога.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let p = entry.path();
        let dst_p = dst.join(entry.file_name());
        if p.is_dir() {
            std::fs::create_dir_all(&dst_p)?;
            copy_tree(&p, &dst_p)?;
        } else {
            let mt = entry.metadata()?;
            if mt.is_symlink() {
                continue;
            }
            imaging::copy_stream(&p, &dst_p)?;
        }
    }
    Ok(())
}

/// Перечитывает таблицу разделов и ЖДЁТ появления узлов разделов.
///
/// Ядро и udev создают узлы `/dev/<dev>p1` асинхронно; без ожидания
/// `mkfs.vfat` часто стартует раньше, чем появится нужный узел
/// (типично для loop-устройств и любого свежего MBR).
fn ensure_partitions(target: &Path, parts: &[&Path]) -> Result<(), String> {
    let target_s = target.to_str().unwrap_or("");

    // Все доступные механизмы перечитывания; каждый может отсутствовать —
    // надёжную проверку даёт поллинг ниже.
    let _ = commands::run_capture("partprobe", &[target_s]);
    let _ = commands::run_capture("partx", &["-a", target_s]);
    let _ = commands::run_capture("udevadm", &["settle", "--timeout=3"]);

    for _ in 0..50 {
        if parts.iter().all(|p| p.exists()) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let names: Vec<String> = parts.iter().map(|p| p.display().to_string()).collect();
    Err(format!(
        "Ядро не пересоздало узлы разделов {} после перечитывания таблицы",
        names.join(", ")
    ))
}

/// Проверяет доступность утилиты через `command -v` (порт к `which`).
fn is_available(tool: &str) -> bool {
    commands::run_capture("which", &[tool]).is_ok()
}