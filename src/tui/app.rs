//! # Приложение TUI (ratatui)
//!
//! Экраны: Devices → Inspect → (Backup | Migrate). Вся бизнес-логика — в
//! [`crate::operations`]; здесь только рендер, ввод и состояние интерфейса.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::disk::{BlockDevice, DiskFilter};
use crate::operations::backup::{BackupPlan, BackupResult};
use crate::operations::inspect::InspectResult;
use crate::operations::migrate::{MigratePlan, MigrateResult};

/// Текущий экран приложения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Devices,
    Inspect,
    Backup,
    BackupDone,
    MigrateTarget,
    Migrate,
    MigrateDone,
}

/// Сообщение от фонового потока-рабочего.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)] // имена смысловые: Inspect/Backup/Migrate Done
pub enum WorkerMsg {
    InspectDone(Result<InspectResult, String>),
    BackupDone(Result<BackupResult, String>),
    MigrateDone(Result<MigrateResult, String>),
}

/// Состояние приложения.
pub struct App {
    pub screen: Screen,
    pub devices: Vec<BlockDevice>,
    pub selected: usize,
    pub source_dev: Option<BlockDevice>,
    pub target_dev: Option<BlockDevice>,

    pub inspect: Option<InspectResult>,
    pub backup_dir: Option<PathBuf>,
    pub busy_msg: String,
    pub last_result: String,

    rx: Option<mpsc::Receiver<WorkerMsg>>,
    pub busy: bool,
    pub log: Vec<String>,
}

impl App {
    pub fn new() -> Self {
        App {
            screen: Screen::Welcome,
            devices: DiskFilter::safe_devices(),
            selected: 0,
            source_dev: None,
            target_dev: None,
            inspect: None,
            backup_dir: None,
            busy_msg: String::new(),
            last_result: String::new(),
            rx: None,
            busy: false,
            log: Vec::new(),
        }
    }

    /// Рефреш списка накопителей.
    pub fn refresh_devices(&mut self) {
        self.devices = DiskFilter::safe_devices();
        if !self.devices.is_empty() && self.selected >= self.devices.len() {
            self.selected = self.devices.len() - 1;
        }
    }

    pub fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 400 {
            self.log.drain(0..self.log.len() - 400);
        }
    }

    /// Обрабатывает клавиатурное событие.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let code = key.code;
        match self.screen {
            Screen::Welcome
                if (code == KeyCode::Enter || code == KeyCode::Char(' ')) => {
                    self.screen = Screen::Devices;
                }
            Screen::Devices => self.key_devices(code),
            Screen::Inspect => self.key_inspect(code),
            Screen::MigrateTarget => self.key_migrate_target(code),
            Screen::BackupDone | Screen::MigrateDone
                if (code == KeyCode::Enter || code == KeyCode::Esc || code == KeyCode::Char('q')
                    || code == KeyCode::Char('c'))
                => {
                    self.screen = Screen::Devices;
                }
            _ => {}
        }
    }

    fn key_devices(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j')
                if !self.devices.is_empty() => {
                    self.selected = (self.selected + 1).min(self.devices.len() - 1);
                }
            KeyCode::Char('r') => self.refresh_devices(),
            KeyCode::Enter => self.select_source(),
            _ => {}
        }
    }

    fn select_source(&mut self) {
        let Some(dev) = self.devices.get(self.selected).cloned() else {
            return;
        };
        self.source_dev = Some(dev.clone());
        self.busy = true;
        self.busy_msg = format!("Анализ: {}", dev.path.display());

        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.rx = Some(rx);

        thread::spawn(move || {
            let res = crate::operations::inspect::inspect_disk(&dev);
            let _ = tx.send(WorkerMsg::InspectDone(res));
        });
    }

    fn key_inspect(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('1') => self.start_backup(),
            KeyCode::Char('3') => {
                self.screen = Screen::MigrateTarget;
                self.selected = 0;
                self.refresh_devices();
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.screen = Screen::Devices;
                self.source_dev = None;
                self.inspect = None;
            }
            _ => {}
        }
    }

    fn start_backup(&mut self) {
        let Some(src) = self.source_dev.clone() else { return };
        let Some(insp) = self.inspect.clone() else { return };

        let out_dir = crate::operations::backup::default_backup_dir();
        let plan = BackupPlan {
            source: src,
            emu: insp.emummc.clone(),
            out_dir,
            games: insp.games.clone(),
        };
        self.screen = Screen::Backup;
        self.busy = true;
        self.busy_msg = "Создание полного слепка EmuMMC + FAT32...".to_string();

        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.rx = Some(rx);

        thread::spawn(move || {
            let r = crate::operations::backup::run_backup(plan);
            let _ = tx.send(WorkerMsg::BackupDone(r));
        });
    }

    fn key_migrate_target(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j')
                if !self.devices.is_empty() => {
                    self.selected = (self.selected + 1).min(self.devices.len() - 1);
                }
            KeyCode::Char('r') => self.refresh_devices(),
            KeyCode::Enter => self.start_migrate(),
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Inspect;
            }
            _ => {}
        }
    }

    fn start_migrate(&mut self) {
        let Some(target) = self.devices.get(self.selected).cloned() else {
            self.last_result = "Нет целевой карты. Вставьте новую SD-карту.".to_string();
            return;
        };
        // Защита: цель не должна совпадать с источником.
        if let Some(src) = &self.source_dev {
            if src.path == target.path {
                self.last_result =
                    "Целевая карта совпадает с исходной — отказ для вашей безопасности.".to_string();
                self.screen = Screen::BackupDone;
                return;
            }
        }
        let Some(backup_dir) = self.backup_dir.clone() else {
            self.last_result = "Сначала создайте бэкап (клавиша 1).".to_string();
            self.screen = Screen::Inspect;
            return;
        };

        self.target_dev = Some(target.clone());
        self.screen = Screen::Migrate;
        self.busy = true;
        self.busy_msg = format!("Разметка и перенос на {}", target.path.display());

        let plan = MigratePlan {
            target,
            backup_dir: backup_dir.clone(),
        };
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.rx = Some(rx);

        thread::spawn(move || {
            let r = crate::operations::migrate::run_migrate(&plan);
            let _ = tx.send(WorkerMsg::MigrateDone(r));
        });
    }

    /// Опрос фоновых задач (вызывается каждый кадр).
    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(msg) = rx.try_recv() {
                self.apply_worker_msg(msg);
                self.rx = None;
                self.busy = false;
            }
        }
    }

    fn apply_worker_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::InspectDone(res) => match res {
                Ok(insp) => {
                    self.inspect = Some(insp);
                    self.screen = Screen::Inspect;
                    self.push_log("Анализ завершён".to_string());
                }
                Err(e) => {
                    self.last_result = format!("Ошибка анализа: {e}");
                    self.push_log(self.last_result.clone());
                    self.screen = Screen::BackupDone;
                }
            },
            WorkerMsg::BackupDone(res) => match res {
                Ok(r) => {
                    self.backup_dir = Some(r.manifest_path.parent().unwrap().to_path_buf());
                    self.last_result = format!(
                        "Бэкап создан: {}\nИгр: {}\nНажмите 3, чтобы перенести на новую карту.",
                        r.manifest_path.parent().unwrap().display(),
                        r.total_bytes
                    );
                    self.push_log(self.last_result.clone());
                    self.screen = Screen::BackupDone;
                }
                Err(e) => {
                    self.last_result = format!("Ошибка бэкапа: {e}");
                    self.push_log(self.last_result.clone());
                    self.screen = Screen::BackupDone;
                }
            },
            WorkerMsg::MigrateDone(res) => match res {
                Ok(r) => {
                    self.last_result = format!(
                        "Миграция завершена.\nНовый sector EmuMMC: {}\nemummc.ini исправлен: {}\nArchive bits: {}",
                        r.new_emu_start, r.emummc_ini_fixed, r.archive_bits_fixed
                    );
                    self.push_log(self.last_result.clone());
                    self.screen = Screen::MigrateDone;
                }
                Err(e) => {
                    self.last_result = format!("Ошибка миграции: {e}");
                    self.push_log(self.last_result.clone());
                    self.screen = Screen::MigrateDone;
                }
            },
        }
    }
}