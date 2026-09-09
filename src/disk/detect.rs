//! # Обнаружение дисков и «Защита от дурака» (Safety Guard)
//!
//! Модуль отвечает за:
//! 1. Перечисление блочных устройств Linux из `/sys/class/block`.
//! 2. Жёсткую фильтрацию по признаку removable-накопителя (USB/SD).
//! 3. Категорический запрет выбора системных дисков (sda, nvme, etc.).
//!
//! Все пути берутся из `/sys` и `/dev`, никаких зашитых путей вроде `/usr/bin/`.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Признак, по которому каждый блок-девайс получает «вердикт» для работы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Internal — зарезервированный класс для будущих сценариев
pub enum BlockClass {
    /// Безопасно — removable-накопитель (SD/USB-ридер). С ним можно работать.
    Removable,
    /// Опасно — системный/внутренний диск (NVMe, internal SATA). Работа НЕ разрешена.
    Internal,
    /// Сомнительно — удалённый/loop/оптический. Работа запрещена по умолчанию.
    Forbidden,
}

/// Структура, описывающая одно блочное устройство после фильтрации.
#[derive(Debug, Clone)]
pub struct BlockDevice {
    /// Имя устройства, напр. `sdb` / `mmcblk0`.
    pub name: String,
    /// Полный путь: `/dev/sdb` или `/dev/mmcblk0`.
    pub path: PathBuf,
    /// Размер в байтах.
    pub size_bytes: u64,
    /// Число секторов накопителя (сектор = 512 байт по стандарту ядра).
    pub sectors: u64,
    /// Признак removable из `/sys/block/<name>/removable`.
    pub removable: bool,
    /// Класс безопасности, вычисленный в [`classify`].
    pub class: BlockClass,
    /// Человекочитаемый vendor/model, если доступен.
    pub model: Option<String>,
}

impl BlockDevice {
    /// True, если с устройством разрешено работать.
    pub fn is_safe(&self) -> bool {
        self.class == BlockClass::Removable
    }

    /// Размер в человекочитаемом виде (GiB).
    pub fn size_gib(&self) -> f64 {
        self.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }
}

impl fmt::Display for BlockDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<10} {:>8.1} GiB  removable={}  [{}]{}",
            self.path.display(),
            self.size_gib(),
            self.removable,
            match self.class {
                BlockClass::Removable => "БЕЗОПАСНО",
                BlockClass::Internal => "ОПАСНО",
                BlockClass::Forbidden => "ЗАПРЕЩЕНО",
            },
            self.model
                .as_ref()
                .map(|m| format!("  ({m})"))
                .unwrap_or_default()
        )
    }
}

/// Читает текстовый атрибут из `/sys/class/block/<dev>/<attr>`.
fn read_sys_string(devname: &str, attr: &str) -> Option<String> {
    let p = format!("/sys/class/block/{devname}/{attr}");
    fs::read_to_string(&p).ok().map(|s| s.trim().to_string())
}

/// Собирает полную информацию о блочном устройстве.
///
/// Возвращает `None`, если девайс отсутствует в `/sys/class/block`.
pub fn detect_disk(devname: &str) -> Option<BlockDevice> {
    let sys_dev = format!("/sys/class/block/{devname}");
    if !Path::new(&sys_dev).exists() {
        return None;
    }

    // Значение "1", когда носитель съёмный.
    let removable = read_sys_string(devname, "removable")
        .map(|s| s.trim() == "1")
        .unwrap_or(false);

    // Размер в секторах по 512 (документировано в ядре: всегда блочная единица 512).
    let sectors = read_sys_string(devname, "size")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);

    let model = read_sys_string(devname, "device/model")
        .or_else(|| read_sys_string(devname, "model"));

    // Тип носителя для mmcblk*: "SD" (карта в ридере) или "MMC" (внутренний eMMC).
    let sd_type = read_sys_string(devname, "device/type");
    let class = classify(removable, devname, sd_type.as_deref());

    Some(BlockDevice {
        name: devname.to_string(),
        path: PathBuf::from(format!("/dev/{devname}")),
        size_bytes: sectors * 512,
        sectors,
        removable,
        class,
        model,
    })
}

/// Определяет класс безопасности.
///
/// Главный критерий — флаг `removable`, который ядро выставляет для USB/SD.
/// Но встроенные (PCIe/USB) SD-кардридеры часто сообщают `removable=0`, хотя в
/// слоте лежит съёмная SD-карта. Отличительный признак — `device/type == "SD"`
/// в sysfs (у внутреннего eMMC там "MMC", у прочих дисков файла нет).
///
/// Тест-режим (`allow-loop` / `SM_ALLOW_LOOP`): loop-устройства, созданные через
/// `losetup` на файлах-образах, разрешаются к работе — это даёт возможность
/// прогнать весь цикл (бэкап/миграцию) на виртуальных «картах» без риска.
fn classify(removable: bool, devname: &str, sd_type: Option<&str>) -> BlockClass {
    if devname.starts_with("loop") && allow_loop_devices() {
        // Тест-режим: loop-устройство (losetup) считается безопасным.
        return BlockClass::Removable;
    }
    // SD-карта в слоте ридера, даже если ядро не выставило removable=1.
    let sd_in_reader = devname.starts_with("mmcblk") && sd_type.is_some_and(|t| t == "SD");
    if !removable && !sd_in_reader {
        // Не removable => точно внутренний диск (SATA/NVMe/eMMC) или сплот-диск.
        return BlockClass::Forbidden;
    }
    // removable-накопитель либо SD в ридере. Допускаем к работе.
    BlockClass::Removable
}

/// Флаг тест-режима (устанавливается из main до первого использования).
static ALLOW_LOOP: AtomicBool = AtomicBool::new(false);

/// Включает тест-режим loop-устройств (или отключает, если передать `false`).
pub fn set_allow_loop(allow: bool) {
    ALLOW_LOOP.store(allow, Ordering::Relaxed);
}

/// True, если loop-устройства разрешены (тест-режим). По умолчанию — выключено.
fn allow_loop_devices() -> bool {
    ALLOW_LOOP.load(Ordering::Relaxed)
}

/// Список всех безопасных removable-накопителей — главный вход для TUI.
pub fn list_block_devices() -> Vec<BlockDevice> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir("/sys/class/block") {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Пропускаем разделы (имена с p<digit> на конце).
            if is_partition_name(&name) {
                continue;
            }
            if let Some(dev) = detect_disk(&name) {
                // Пустые loop-устройства (без attached image) — шум в списке.
                if dev.sectors == 0 {
                    continue;
                }
                out.push(dev);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// True, если имя — раздел (а не целый диск).
///
/// Целые диски: `sda`, `mmcblk0`, `nvme0n1`, `loop0`, `sr0`.
/// Разделы: `sda1`, `mmcblk0p1`, `nvme0n1p1`, `loop0p1`.
fn is_partition_name(name: &str) -> bool {
    // Суффикс «<цифры>p<цифры>» — раздел с явным индексом (mmcblk0p1, nvme0n1p1).
    // Исключаем «loop0»: у него нет цифры перед 'p'.
    if let Some(idx) = name.rfind('p') {
        let suffix = &name[idx + 1..];
        let prefix = &name[..idx];
        if !suffix.is_empty()
            && suffix.chars().all(|c| c.is_ascii_digit())
            && prefix.ends_with(|c: char| c.is_ascii_digit())
        {
            return true;
        }
    }
    // Семейство sdX / vdX / xvdX: имя = префикс + буква + необязательные цифры.
    for fam in ["sd", "vd", "xvd"] {
        if let Some(rest) = name.strip_prefix(fam) {
            let letters: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            let digits = &rest[letters.len()..];
            if !letters.is_empty()
                && !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
    }
    false
}

/// True, если `path` — это файл блочного устройства.
pub fn is_block_device(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path)
        .map(|m| m.file_type().is_block_device())
        .unwrap_or(false)
}

/// Путь к разделу `index` устройства `dev_path`.
///
/// Соглашения имён разделов различаются:
/// - `sda1` (sd/vd/xvd — цифра сразу после имени),
/// - `mmcblk0p1`, `nvme0n1p1`, `loop0p1` (после имени-с-цифрой идёт `p`).
///
/// Сначала ищем точное имя в `/sys/class/block` (надёжно для loop/mmcblk/nvme),
/// при отсутствии — используем эвристику по последней цифре имени.
pub fn partition_path(dev_path: &Path, index: u32) -> PathBuf {
    if let Some(name) = dev_path.file_name().and_then(|n| n.to_str()) {
        let idx = index.to_string();
        if let Ok(rd) = fs::read_dir("/sys/class/block") {
            for entry in rd.flatten() {
                let n = entry.file_name().to_string_lossy().to_string();
                if n == format!("{name}{idx}") || n == format!("{name}p{idx}") {
                    return PathBuf::from(format!("/dev/{n}"));
                }
            }
        }
    }
    // Фолбэк по соглашению: имена, оканчивающиеся на цифру → «p»-стиль.
    let name = dev_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let last_is_digit = name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false);
    if last_is_digit {
        PathBuf::from(format!("{}p{index}", dev_path.display()))
    } else {
        PathBuf::from(format!("{}{index}", dev_path.display()))
    }
}

/// Публичный фильтр для выбора безопасных накопителей.
pub struct DiskFilter;

impl DiskFilter {
    /// Возвращает только безопасные и разумные устройства.
    pub fn safe_devices() -> Vec<BlockDevice> {
        list_block_devices()
            .into_iter()
            .filter(|d| d.is_safe())
            .collect()
    }

    /// Проверяет, что переданный путь — существующее блочное устройство, причём безопасное.
    pub fn validate_for_probe(path: &str) -> Result<BlockDevice, String> {
        let p = Path::new(path);
        if !is_block_device(p) {
            return Err(format!("{path} не является блочным устройством"));
        }
        let name = p
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .ok_or_else(|| format!("Не удалось извлечь имя устройства из {path}"))?;
        let info = detect_disk(&name)
            .ok_or_else(|| format!("Не удалось прочитать параметры устройства {path}"))?;
        if !info.is_safe() {
            return Err(format!(
                "Устройство {path} помечено как ОПАСНОЕ (системный/внутренний диск). Работа запрещена."
            ));
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_internal_blocks_sda() {
        assert_eq!(classify(false, "sda", None), BlockClass::Forbidden);
        assert_eq!(classify(false, "nvme0n1", None), BlockClass::Forbidden);
    }

    #[test]
    fn class_removable_pass() {
        assert_eq!(classify(true, "sdb", None), BlockClass::Removable);
        assert_eq!(classify(true, "mmcblk0", None), BlockClass::Removable);
    }

    #[test]
    fn sd_card_in_reader_pass() {
        // PCIe/USB-ридеры сообщают removable=0 даже со вставленной SD-картой:
        // засчитываем их по device/type == "SD".
        assert_eq!(classify(false, "mmcblk0", Some("SD")), BlockClass::Removable);
        // Внутренний eMMC (type "MMC") остаётся ЗАПРЕЩЁННЫМ.
        assert_eq!(classify(false, "mmcblk0", Some("MMC")), BlockClass::Forbidden);
        // Без device/type (обычный диск) с removable=0 — запрещён.
        assert_eq!(classify(false, "mmcblk0", None), BlockClass::Forbidden);
    }

    #[test]
    fn partition_names_detected() {
        assert!(is_partition_name("sdb1"));
        assert!(is_partition_name("mmcblk0p1"));
        assert!(is_partition_name("nvme0n1p1"));
        assert!(!is_partition_name("sdb"));
        assert!(!is_partition_name("mmcblk0"));
        assert!(!is_partition_name("nvme0n1"));
        assert!(!is_partition_name("loop0"));
    }

    #[test]
    fn loop_devices_pass_only_in_test_mode() {
        assert_eq!(classify(false, "loop0", None), BlockClass::Forbidden);
        set_allow_loop(true);
        assert_eq!(classify(false, "loop0", None), BlockClass::Removable);
        // Сброс для остальных тестов процесса.
        set_allow_loop(false);
        assert_eq!(classify(false, "loop0", None), BlockClass::Forbidden);
    }

    #[test]
    fn partition_path_fallback_naming() {
        // Несуществующие устройства — проверяем эвристику суффикса.
        assert_eq!(
            partition_path(&PathBuf::from("/dev/sda"), 1),
            PathBuf::from("/dev/sda1")
        );
        assert_eq!(
            partition_path(&PathBuf::from("/dev/vdb"), 2),
            PathBuf::from("/dev/vdb2")
        );
        assert_eq!(
            partition_path(&PathBuf::from("/dev/mmcblk0"), 1),
            PathBuf::from("/dev/mmcblk0p1")
        );
        assert_eq!(
            partition_path(&PathBuf::from("/dev/loop0"), 2),
            PathBuf::from("/dev/loop0p2")
        );
        assert_eq!(
            partition_path(&PathBuf::from("/dev/nvme0n1"), 1),
            PathBuf::from("/dev/nvme0n1p1")
        );
    }
}
