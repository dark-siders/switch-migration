//! # Разбор таблицы разделов и расчёт смещения секторов (Sector Offset)
//!
//! Цель — найти на SD-карте:
//! - FAT32-раздел (обычно p1),
//! - скрытый RAW-раздел EmuMMC/EmuNAND (обычно p2) — тип 0xC (FAT32 hidden) / 0xDC /
//!   либо GPT. Определяем как «RAW» по сигнатуре файловой системы внутри.
//!
//! Глобальная проблема, которую решает утилита — **сдвиг секторов**: при переносе
//! RAW-образа на новую карту стартовый сектор раздела меняется, и Hekate перестаёт
//! его находить. Мы читаем реальный стартовый сектор и правим `emummc.ini`.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

use super::SECTOR_SIZE;

/// Тип раздела (по MBR partition type байту).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PartType {
    Data,     // FAT32 / 0x0C
    Raw,      // EmuMMC скрытый раздел
    Other(u8),
}

/// Тип по геометрической интерпретации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PartKind {
    Fat32,
    EmuRaw,
    Unsupported,
}

/// Описание одного раздела в записи MBR/GPT.
#[derive(Debug, Clone)]
#[allow(dead_code)] // type_byte/dev_path востребованы в будущих хелперах
pub struct Partition {
    /// Индекс в таблице (1-based для MBR-стиля отображения).
    pub index: usize,
    /// Стартовый сектор (LBA).
    pub start: u64,
    /// Количество секторов.
    pub length: u64,
    /// Тип MBR (байт) или идентификатор.
    pub type_byte: u8,
    /// Расшифрованный тип.
    pub kind: PartKind,
    /// Путь к блочному устройству раздела (например /dev/sdb1) если доступен.
    pub dev_path: Option<PathBuf>,
}

impl Partition {
    /// Стартовый байт во всём накопителе.
    #[allow(dead_code)]
    pub fn start_byte(&self) -> u64 {
        self.start * SECTOR_SIZE
    }

    /// Размер в байтах.
    pub fn size_bytes(&self) -> u64 {
        self.length * SECTOR_SIZE
    }
}

/// Таблица разделов, собранная с диска.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct PartitionTable {
    /// Стиль — MBR или GPT (или пусто, если не распознан).
    pub scheme: Option<&'static str>,
    pub partitions: Vec<Partition>,
}

impl PartitionTable {
    /// Найти раздел по типу.
    pub fn find(&self, kind: PartKind) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.kind == kind)
    }
}

/// Прямое чтение MBR-сектора (сектор 0) и парсинг 4 записей.
///
/// Использует `nix`-подобный прямой доступ через `File` (open read). Монтирование
/// НЕ требуется — читаем raw-блок устройства.
pub fn parse_mbr(dev_path: &std::path::Path) -> Result<PartitionTable, io::Error> {
    let mut f = fs::File::open(dev_path)?;
    let mut mbr = [0u8; 512];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut mbr)?;

    // Проверка сигнатуры 0x55 0xAA в конце.
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Не валидная MBR-сигнатура (0x55 0xAA)",
        ));
    }

    let mut table = PartitionTable {
        scheme: Some("MBR"),
        partitions: Vec::with_capacity(4),
    };

    // 4 записи по 16 байт, начиная с offset 446.
    for i in 0..4u32 {
        let off = 446 + i as usize * 16;
        if off + 16 > mbr.len() {
            break;
        }
        let status = mbr[off];
        // 0x80 — активный, 0x00 — неактивный, прочее — невалид.
        if status != 0x80 && status != 0x00 {
            continue;
        }
        let type_byte = mbr[off + 4];
        if type_byte == 0x00 {
            continue; // пустая запись
        }
        let start = u32::from_le_bytes([
            mbr[off + 8],
            mbr[off + 9],
            mbr[off + 10],
            mbr[off + 11],
        ]) as u64;
        let length = u32::from_le_bytes([
            mbr[off + 12],
            mbr[off + 13],
            mbr[off + 14],
            mbr[off + 15],
        ]) as u64;

        let kind = classify_type(type_byte);
        table.partitions.push(Partition {
            index: (i as usize) + 1,
            start,
            length,
            type_byte,
            kind,
            dev_path: None,
        });
    }
    Ok(table)
}

/// Классификация типа MBR-раздела.
fn classify_type(t: u8) -> PartKind {
    match t {
        0x0B | 0x0C | 0x1C => PartKind::Fat32, // FAT32 / FAT32 LBA / hidden
        0x07 => PartKind::Fat32,               // NTFS/exFAT тоже «данные», но не наш кейс
        // Скрытые/сырые — Hekate использует тип 0xC (скрытый FAT32) или просто
        // освободившееся сырое пространство. Отличительный признак — наличие
        // файловой структуры EmuMMC внутри, что проверяется сигнатурой.
        _ => PartKind::Unsupported,
    }
}

/// Тип, который показывает пользователю.
impl PartType {
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            PartType::Data => "FAT32/данные",
            PartType::Raw => "RAW EmuMMC",
            PartType::Other(_) => "прочее",
        }
    }
}

/// Проверка, что раздел содержит файловую систему vfat (по BPB).
///
/// Считываем 3 байта boot-jump + 8 байт OEM + 3 байта BPB (FAT32: "FAT32   ").
#[allow(dead_code)]
pub fn probe_fat_signature(dev_part: &std::path::Path, start_sector: u64) -> Result<bool, io::Error> {
    let mut f = fs::File::open(dev_part)?;
    f.seek(SeekFrom::Start(start_sector * SECTOR_SIZE))?;
    let mut bpb = [0u8; 512];
    f.read_exact(&mut bpb)?;
    // fatsz_32 field at byte 0x52 == 0
    // type string at 0x52+8? По spec: 0x52 = FATSz32. Имя FS в 0x36..0x3D
    Ok(&bpb[0x36..0x3D] == b"FAT32   ")
}

/// Определяет, является ли раздел скрытым EmuMMC.
///
/// Hekate хранит в начале EmuMMC-раздела свои заголовки (обычно меньше сектора),
/// но реальный признак для `file --filesystem` — сигнатура 0x55 0xAA в конце и
/// отсутствие FAT32-меток. Для устойчивости ищем известный паттерн «emmc».
#[allow(dead_code)]
pub fn probe_is_emummc(dev_part: &std::path::Path, start_sector: u64) -> io::Result<bool> {
    let mut f = fs::File::open(dev_part)?;
    f.seek(SeekFrom::Start(start_sector * SECTOR_SIZE))?;
    let mut buf = [0u8; 4096];
    f.read_exact(&mut buf)?;
    // В начале EmuMMC-раздела (после сдвига) Hekate кладёт структуру с текстом.
    // Реальная сигнатура обусловлена конкретным паттерном boot0/boot1 offset.
    // Признаем достаточно точным наличие ASCII «EMMC» или 0x55 0xAA.
    Ok(buf.windows(5).any(|w| w == b"EMMC\0") || buf[510..512] == [0x55, 0xAA])
}

/// Чтение стартового сектора раздела самым надёжным способом — через sysfs
/// `/sys/class/block/<dev>/start`. Это точнее, чем парсинг MBR.
#[allow(dead_code)]
pub fn read_start_from_sysfs(devname: &str) -> Option<u64> {
    fs::read_to_string(format!("/sys/class/block/{devname}/start"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Модель геометрии после расчёта: где начинается RAW EmuMMC.
#[derive(Debug, Clone)]
pub struct EmuGeometry {
    /// Стартовый сектор FAT32-раздела (обычно 2048).
    pub fat_start: u64,
    /// Стартовый сектор EmuMMC-раздела — то самое число для `emummc.ini`.
    pub emu_start: u64,
    /// Размер EmuMMC-раздела в секторах.
    pub emu_len: u64,
    /// Размер EmuMMC-раздела в байтах.
    pub emu_bytes: u64,
}

impl EmuGeometry {
    /// Значение, которое нужно вписать в `emummc.ini` (поле `sector=`).
    #[allow(dead_code)]
    pub fn ini_sector(&self) -> u64 {
        self.emu_start
    }
}
