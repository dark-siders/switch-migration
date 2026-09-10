//! # Дамп и восстановление RAW-раздела EmuMMC
//!
//! Реализует «на лету» перенос сырого раздела EmuMMC с карты на карту:
//! - **Дамп** в файл-образ со сжатием zstd (не занимает лишнего места на пустых
//!   секторах — реально экономит сотни ГБ на EmuNAND).
//! - **Восстановление** zstd-образа в RAW-раздел новой карты.
//!
//! Чтение выполняется блоками по 4 МБ напрямую с блочного устройства, что
//! обеспечивает корректную работу с незанятым пространством — сжатие убирает
//! «дыры» из нулевых секторов.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use thiserror::Error;

use crate::utils::progress::Progress;

/// Размер блока в байтах при копировании/сжатии.
pub const BLOCK_SIZE: usize = 4 * 1024 * 1024;

/// Расширение файла-образа EmuMMC.
#[allow(dead_code)]
pub const IMAGE_EXT: &str = "img.zst";

#[derive(Debug, Error)]
pub enum ImagingError {
    #[error("IO-ошибка: {0}")]
    Io(#[from] io::Error),
    #[error("Устройство {device} закончилось раньше ожидаемого размера образа ({expected} байт)")]
    ShortRead { device: String, expected: u64 },
}

/// Создаёт сжатый zstd-образ RAW-раздела EmuMMC.
///
/// # Аргументы
/// - `src_dev` — путь к блочному устройству раздела (например `/dev/sdb2`).
/// - `start_sector` — стартовый сектор EmuMMC в устройстве.
/// - `num_sectors` — количество секторов для чтения.
/// - `out_zst` — путь к результирующему `.img.zst` файлу.
/// - `progress` — отчёт прогресса (печатает % в stderr / шлёт в канал TUI).
pub fn dump_emummc(
    src_dev: &Path,
    start_sector: u64,
    num_sectors: u64,
    out_zst: &Path,
    progress: &mut Progress,
) -> Result<u64, ImagingError> {
    let mut src = File::open(src_dev)?;
    src.seek(SeekFrom::Start(start_sector * 512))?;

    let sink = File::create(out_zst)?;
    let mut writer = BufWriter::new(sink);
    let mut zst = zstd::stream::write::Encoder::new(&mut writer, 3)?;

    let total = num_sectors * 512;
    let mut read_bytes: u64 = 0;
    let mut buffer = vec![0u8; BLOCK_SIZE];

    while read_bytes < total {
        let want = std::cmp::min(BLOCK_SIZE as u64, total - read_bytes) as usize;
        let n = src.read(&mut buffer[..want])?;
        if n == 0 {
            // неожиданный конец устройства
            return Err(ImagingError::ShortRead {
                device: src_dev.display().to_string(),
                expected: total,
            });
        }
        // На случай, если прочитано меньше запрошенного (хвост), обнуляем
        // остаток — иначе сожмём мусор.
        if n < want {
            buffer[n..want].fill(0);
        }
        zst.write_all(&buffer[..want])?;
        read_bytes += want as u64;
        progress.report(read_bytes);
    }

    zst.finish()?;
    writer.flush()?;
    Ok(read_bytes)
}

/// Восстановление zstd-образа обратно в RAW-раздел.
///
/// # Аргументы
/// - `zst_path` — сжатый образ.
/// - `dest_dev` — блочное устройство раздела назначения (например `/dev/sdc2`).
/// - `dest_start_sector` — стартовый сектор нового RAW-раздела.
/// - `total_bytes` — ожидаемый объём данных после распаковки (`num_sectors * 512`),
///   нужен для процентов прогресса.
/// - `progress` — отчёт прогресса.
pub fn restore_emummc(
    zst_path: &Path,
    dest_dev: &Path,
    dest_start_sector: u64,
    progress: &mut Progress,
) -> Result<u64, ImagingError> {
    let in_file = File::open(zst_path)?;
    let reader = BufReader::new(in_file);
    let mut zst = zstd::stream::read::Decoder::new(reader)?;

    let mut dest = File::options().write(true).open(dest_dev)?;
    dest.seek(SeekFrom::Start(dest_start_sector * 512))?;

    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut written: u64 = 0;
    loop {
        let n = zst.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        dest.write_all(&buffer[..n])?;
        written += n as u64;
        progress.report(written);
    }
    dest.flush()?;
    Ok(written)
}

/// Синхронизация буферов ОС с диском (fsync) — важно до извлечения карты.
pub fn sync_disk(dev: &Path) -> io::Result<()> {
    let f = File::options().read(true).open(dev)?;
    f.sync_all()
}

/// Прямое копирование без сжатия — для файлов FAT32 (не RAW-разделов).
///
/// Используется при переносе папки `Nintendo/` и файлов Hekate/Atmosphere.
pub fn copy_stream(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut in_f = File::open(src)?;
    let mut out_f = File::create(dst)?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let n = in_f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out_f.write_all(&buf[..n])?;
        total += n as u64;
    }
    out_f.flush()?;
    Ok(total)
}
