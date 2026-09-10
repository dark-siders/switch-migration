//! # JSON-манифест бэкапа
//!
//! Собирает всю метаинформацию о слепке: геометрию секторов, размеры, список
//! игр, дату и параметры разметки. Сериализуется в `manifest.json` рядом с
//! образом EmuMMC и служит для автоматического восстановления на новую карту.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Сигнатура файла-манифеста для быстрой валидации.
pub const MAGIC: &str = "SM-MIGRATOR-1";

/// Полное описание одного слепка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub magic: String,
    pub id: String,
    pub created_at: String,

    /// Размер исходной карты в байтах.
    pub source_size_bytes: u64,

    /// Геометрия EmuMMC-раздела исходной карты.
    pub emummc: EmuSection,
    /// Геометрия FAT32-раздела исходной карты.
    pub fat32: Fat32Section,

    /// Список найденных игр (по структуре Nintendo/Contents/registered).
    pub games: Vec<GameEntry>,
    /// Количество игр (для быстрого отображения без парсинга).
    pub game_count: usize,
    /// Список homebrew-приложений (`switch/<app>/*.nro`).
    #[serde(default)]
    pub homebrew: Vec<HomebrewEntry>,

    /// Размер сжатого образа EmuMMC на диске.
    pub image_zst_bytes: u64,
}

impl BackupManifest {
    /// Создаёт пустой манифест с уникальным ID и текущей датой.
    pub fn new(
        source_size_bytes: u64,
        emummc: EmuSection,
        fat32: Fat32Section,
        games: Vec<GameEntry>,
    ) -> Self {
        let game_count = games.len();
        let created_at = timestamp_iso();
        BackupManifest {
            magic: MAGIC.to_string(),
            id: Uuid::new_v4().to_string(),
            created_at,
            source_size_bytes,
            emummc,
            fat32,
            games,
            game_count,
            homebrew: Vec::new(),
            image_zst_bytes: 0,
        }
    }

    /// Валидирует, что это корректный манифест этого инструмента.
    pub fn validate(&self) -> bool {
        self.magic == MAGIC
    }

    /// Сохраняет манифест в файл.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }

    /// Читает манифест из файла.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let m: BackupManifest = serde_json::from_str(&raw)?;
        Ok(m)
    }
}

/// Геометрия EmuMMC-раздела.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmuSection {
    /// Стартовый сектор (для `emummc.ini`).
    pub start_sector: u64,
    /// Количество секторов.
    pub num_sectors: u64,
    /// Размер в байтах.
    pub size_bytes: u64,
}

/// Геометрия FAT32-раздела.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fat32Section {
    /// Стартовый сектор.
    pub start_sector: u64,
    /// Количество секторов.
    pub num_sectors: u64,
    pub size_bytes: u64,
}

/// Одна найденная игра.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEntry {
    /// ID приложения (title ID, 16 hex-цифр).
    pub title_id: String,
    /// Путь в `Nintendo/Contents/registered/<titleid>/`.
    pub path: String,
    /// Размер каталога в байтах.
    pub size_bytes: u64,
    /// Человекочитаемое название (пустая строка, если нет в таблице).
    #[serde(default)]
    pub name: String,
}

impl GameEntry {
    pub fn new(title_id: String, path: String, size_bytes: u64) -> Self {
        GameEntry {
            title_id,
            path,
            size_bytes,
            name: String::new(),
        }
    }
}

/// Одно установленное homebrew-приложение.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomebrewEntry {
    /// Имя приложения (по каталогу в `/switch/`).
    pub name: String,
    /// Путь, например `switch/App/App.nro`.
    pub path: String,
    /// Размер `.nro` в байтах.
    pub size_bytes: u64,
}

impl HomebrewEntry {
    pub fn new(name: String, path: String, size_bytes: u64) -> Self {
        HomebrewEntry { name, path, size_bytes }
    }
}

/// Текущее время в ISO-формате.
fn timestamp_iso() -> String {
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // простой формат: секунды с эпохи + человекочитаемость на уровне приложения
    format!("{}", t.as_secs())
}
