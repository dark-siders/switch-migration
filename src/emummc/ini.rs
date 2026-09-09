//! # Чтение/корректировка `sdmc:/emummc/emummc.ini`
//!
//! Hekate использует INI-конфиг, где основная секция `[emummc]` содержит поля:
//! - `enabled=1`
//! - `sector=0x...` — **стартовый сектор raw-раздела EmuMMC** (критично!)
//! - `id=...`
//! - `nintendo_path=...`, `emummc_path=...`, `emummc_type=partition`
//!
//! Проблема сдвига секторов: если перенести RAW-образ на новую карту, где
//! стартовый сектор другого размера таблицы разделов, Hekate не найдёт EmuMMC.
//! Этот модуль нативно правит `sector=` на актуальное значение.

use std::fs;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IniError {
    #[error("IO-ошибка: {0}")]
    Io(#[from] std::io::Error),
    #[error("Отсутствует секция [emummc] в файле {path}")]
    MissingSection { path: String },
    #[error("Не удалось распознать текущее значение sector в {path}")]
    BadSector { path: String },
}

/// Представление `emummc.ini`.
#[derive(Debug, Clone)]
pub struct EmummcIni {
    /// Необработанные строки файла — чтобы точно сохранить формат (комментарии).
    pub raw_lines: Vec<String>,
    /// Парсинг ключ/значение внутри `[emummc]`.
    pub values: std::collections::HashMap<String, String>,
    /// Найденное текущее значение `sector`.
    pub sector: u64,
    /// Путь файла (для записи).
    pub path: PathBuf,
}

use std::path::PathBuf;

impl EmummcIni {
    /// Читает и парсит `emummc.ini`.
    pub fn load(path: &Path) -> Result<Self, IniError> {
        let raw = fs::read_to_string(path)?;
        let raw_lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();

        let mut in_emummc = false;
        let mut values = std::collections::HashMap::new();
        let mut sector: Option<u64> = None;

        for line in raw.lines() {
            let trimmed = line.trim();
            // секция
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_emummc = trimmed.eq_ignore_ascii_case("[emummc]");
                continue;
            }
            if !in_emummc {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                let k = k.trim();
                let v = v.trim();
                values.insert(k.to_string(), v.to_string());
                if k.eq_ignore_ascii_case("sector") {
                    sector = parse_sector(v);
                }
            }
        }

        if !in_emummc {
            return Err(IniError::MissingSection {
                path: path.display().to_string(),
            });
        }
        let sector = sector.ok_or_else(|| IniError::BadSector {
            path: path.display().to_string(),
        })?;

        Ok(EmummcIni {
            raw_lines,
            values,
            sector,
            path: path.to_path_buf(),
        })
    }

    /// Возвращает значение ключа из `[emummc]`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    /// Меняет поле `sector` на новое значение и записывает файл на диск.
    ///
    /// Строка сектора записывается в шестнадцатеричном виде (`0x...`), как
    /// принято в конфигах Hekate. Если в оригинале было десятичное — сохраняем
    /// тот же стиль.
    pub fn set_sector_and_save(&mut self, new_sector: u64) -> Result<(), IniError> {
        let hex_style = self.get("sector").is_none_or(|s| s.starts_with("0x"));
        let new_val = if hex_style {
            format!("0x{new_sector:X}")
        } else {
            format!("{new_sector}")
        };

        // Пересобираем строки, заменяя значение sector в секции [emummc].
        let mut in_emummc = false;
        let mut replaced = false;
        for line in self.raw_lines.iter_mut() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_emummc = trimmed.eq_ignore_ascii_case("[emummc]");
                continue;
            }
            if !in_emummc {
                continue;
            }
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim().eq_ignore_ascii_case("sector") {
                    // сохраняем отступ слева
                    let leading: String =
                        line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    *line = format!("{leading}sector={new_val}");
                    replaced = true;
                }
            }
        }

        if !replaced {
            // Секции [emummc] нет поля sector — добавим его отдельной строкой.
            self.raw_lines.push(format!("sector={new_val}"));
        }

        // Обновляем кэш
        self.values.insert("sector".to_string(), new_val.clone());
        self.sector = new_sector;

        // Пишем
        let content = self.raw_lines.join("\n") + "\n";
        fs::write(&self.path, content)?;
        Ok(())
    }
}

/// Парсит значение сектора (может быть `0x...`, десятичным, или с 0X/0x).
fn parse_sector(v: &str) -> Option<u64> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        v.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const SAMPLE_HEX: &str = r#"[emummc]
enabled=1
sector=0x12902E0
id=000
nintendo_path=Nintendo
emummc_path=emummc
emummc_type=partition
"#;

    const SAMPLE_DEC: &str = r#"[emummc]
sector=19572768
"#;

    fn write_tmp(dir: &Path, content: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("emummc.ini");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        p
    }

    #[test]
    fn parses_hex_sector() {
        let dir = std::env::temp_dir().join(format!("sm-ini-hex-{}", std::process::id()));
        let p = write_tmp(&dir, SAMPLE_HEX);
        let ini = EmummcIni::load(&p).unwrap();
        assert_eq!(ini.sector, 0x12902E0);
        assert_eq!(ini.get("emummc_type"), Some("partition"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_decimal_sector() {
        let dir = std::env::temp_dir().join(format!("sm-ini-dec-{}", std::process::id()));
        let p = write_tmp(&dir, SAMPLE_DEC);
        let ini = EmummcIni::load(&p).unwrap();
        assert_eq!(ini.sector, 19572768);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fixes_sector_preserving_hex_style() {
        let dir = std::env::temp_dir().join(format!("sm-ini-fix-{}", std::process::id()));
        let p = write_tmp(&dir, SAMPLE_HEX);
        let mut ini = EmummcIni::load(&p).unwrap();
        ini.set_sector_and_save(2048).unwrap();
        let reloaded = EmummcIni::load(&p).unwrap();
        assert_eq!(reloaded.sector, 2048);
        assert_eq!(reloaded.get("sector"), Some("0x800"));
        assert_eq!(reloaded.get("nintendo_path"), Some("Nintendo"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_section_is_error() {
        let dir = std::env::temp_dir().join(format!("sm-ini-miss-{}", std::process::id()));
        let p = write_tmp(&dir, "[config]\nsector=0x1\n");
        assert!(EmummcIni::load(&p).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
