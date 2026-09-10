//! # Таблица названий игр (title_id → имя)
//!
//! Полная база игр Switch огромна, поэтому храним компактный файл
//! `src/data/titles.csv` и дополняем его по мере появления игр владельца.
//! Строка: `<16-hex id>,<название>`. Комментарии — строки, начинающиеся с `#`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Загруженная таблица (кэш первого вызова).
fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m = HashMap::new();
        for line in include_str!("../data/titles.csv").lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((id, name)) = line.split_once(',') {
                m.insert(id.trim().to_ascii_uppercase(), name.trim().to_string());
            }
        }
        m
    })
}

/// Возвращает название игры по title ID (None — нет в таблице).
pub fn lookup(title_id: &str) -> Option<String> {
    table().get(&title_id.to_ascii_uppercase()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_known_title() {
        assert_eq!(
            lookup("01007EF00011E000").as_deref(),
            Some("The Legend of Zelda: Breath of the Wild")
        );
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(lookup("0000000000000000"), None);
    }
}