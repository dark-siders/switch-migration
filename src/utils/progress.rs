//! # Прогресс операций (бэкап/миграция)
//!
//! Единый механизм отображения прогресса в процентах для CLI и TUI:
//! - `ProgressSink::Stderr` — печать `\r…%` в stderr (CLI, `sudo nix run …`);
//! - `ProgressSink::Callback` — вызов замыкания `Fn(u8)` при каждом изменении
//!   процента (TUI отправляет `WorkerMsg::Progress`, рисует Gauge);
//!
//! Проценты выводятся только когда значение меняется — не спамим терминал.

use std::path::Path;
use std::sync::Arc;

/// Куда отправлять прогресс.
///
/// `Callback` — универсальный вариант для TUI: замыкание `Fn(u8)`,
/// которое вызывается при каждом изменении процента (0..100).
#[derive(Clone)]
pub enum ProgressSink {
    /// Печать одной перезаписываемой строки в stderr.
    Stderr,
    /// Вызов замыкания при каждом изменении процента (TUI: отправляет WorkerMsg::Progress).
    Callback(Arc<dyn Fn(u8) + Send + Sync>),
}

/// Прогресс-отчёт с известным итоговым объёмом.
pub struct Progress {
    sink: ProgressSink,
    total: u64,
    label: String,
    last_pct: u8,
}

impl Progress {
    /// Создаёт отчёт с известным суммарным объёмом `total`.
    pub fn new(sink: ProgressSink, total: u64, label: &str) -> Self {
        Progress {
            sink,
            total,
            label: label.to_string(),
            last_pct: u8::MAX,
        }
    }

    /// Обновляет позицию: вычисляет процент и отправляет, только если он изменился.
    pub fn report(&mut self, pos: u64) {
        let pct = self.percent(pos);
        if pct == self.last_pct {
            return;
        }
        self.last_pct = pct;
        match &self.sink {
            ProgressSink::Stderr => {
                eprint!(
                    "\r{}: {}% ({}/{})",
                    self.label,
                    pct,
                    self.human(pos),
                    self.human(self.total)
                );
            }
            ProgressSink::Callback(f) => f(pct),
        }
    }

    /// Завершает отчёт: выводит финальные 100%/и сообщение.
    pub fn finish(&mut self) {
        if self.last_pct != 100 {
            self.report(self.total);
        }
        match &self.sink {
            ProgressSink::Stderr => eprintln!("\r{}: готово ({}).", self.label, self.human(self.total)),
            ProgressSink::Callback(f) => f(100),
        }
    }

    /// Процент 0..100 от текущей позиции.
    fn percent(&self, pos: u64) -> u8 {
        if self.total == 0 {
            return 100;
        }
        ((pos.saturating_mul(100)) / self.total).min(100) as u8
    }

    /// Человекочитаемый размер байт.
    fn human(&self, bytes: u64) -> String {
        crate::utils::human::human_bytes(bytes)
    }
}

/// Рекурсивно считает суммарный размер файлов в каталоге (для прогресса копирования).
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir).into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(md) = entry.metadata() {
            total += md.len();
        }
    }
    total
}