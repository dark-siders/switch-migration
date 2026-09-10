//! # switch-migrator — перенос данных Nintendo Switch на новую SD-карту
//!
//! TUI (ratatui) поверх безопасных операций над блочными устройствами.
//!
//! Режимы вызова:
//! - `switch-migrator`          — запуск TUI.
//! - `switch-migrator ls`       — список безопасных накопителей.
//! - `switch-migrator probe <dev>` — анализ конкретного устройства.
//! - `switch-migrator backup <dev> --out <dir>` — бэкап карты.
//!
//! Всё остальное — через интерфейс.

mod disk;
mod emummc;
mod fat32;
mod manifest;
mod operations;
mod tui;
mod utils;

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

fn main() -> std::io::Result<()> {
    let mut args: Vec<String> = std::env::args().collect();

    // Тест-режим loop-устройств: флаг `--allow-loop` или переменная SM_ALLOW_LOOP.
    let allow_loop = args.iter().any(|a| a == "--allow-loop")
        || std::env::var("SM_ALLOW_LOOP").is_ok_and(|v| {
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        });
    disk::set_allow_loop(allow_loop);
    args.retain(|a| a != "--allow-loop");

    match args.get(1).map(|s| s.as_str()) {
        Some("ls") => cmd_list(),
        Some("probe") => cmd_probe(args.get(2)),
        Some("backup") => cmd_backup(args.get(2), args.get(4)),
        Some("apply") => cmd_migrate(args.get(2), args.get(3)),
        _ => run_tui(),
    }
}

/// Распаковывает Result или печатает ошибку и выходит.
fn or_die<T>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            std::process::exit(1);
        }
    }
}

/// Вывод списка безопасных накопителей (для скриптов).
fn cmd_list() -> std::io::Result<()> {
    let devs = disk::DiskFilter::safe_devices();
    if devs.is_empty() {
        println!("Нет removable-накопителей.");
        return Ok(());
    }
    for d in &devs {
        println!("{}\t{}", d.path.display(), d.size_bytes);
    }
    Ok(())
}

/// Анализ конкретного устройства (probe).
fn cmd_probe(dev: Option<&String>) -> std::io::Result<()> {
    let Some(dev) = dev else {
        eprintln!("Использование: switch-migrator probe /dev/sdX");
        std::process::exit(2);
    };
    let bd = or_die(disk::DiskFilter::validate_for_probe(dev));
    let insp = or_die(operations::inspect::inspect_disk(&bd));

    println!("Устройство: {}", bd.path.display());
    println!(
        "  Размер:   {} секторов ({})",
        bd.sectors,
        utils::human::human_bytes(bd.size_bytes)
    );
    match &insp.emummc {
        Some(e) => println!(
            "  EmuMMC:   старт {0}, длин {1} секторов",
            e.emu_start, e.emu_len
        ),
        None => println!("  EmuMMC:   не найдена"),
    }
    println!("  Игры:     {}", insp.games.len());
    println!("  Сводка:   {}", insp.summary);
    Ok(())
}

/// Бэкап карты в --out директорию.
fn cmd_backup(dev: Option<&String>, out: Option<&String>) -> std::io::Result<()> {
    let Some(dev) = dev else {
        eprintln!("Использование: switch-migrator backup /dev/sdX --out <dir>");
        std::process::exit(2);
    };
    let bd = or_die(disk::DiskFilter::validate_for_probe(dev));
    let insp = or_die(operations::inspect::inspect_disk(&bd));

    let out_dir = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("backup-{:x}", std::process::id())));
    std::fs::create_dir_all(&out_dir).ok();

    let plan = operations::backup::BackupPlan {
        source: bd,
        emu: insp.emummc,
        out_dir,
        games: insp.games,
        homebrew: insp.homebrew,
    };
    let res = or_die(operations::backup::run_backup(
        plan,
        utils::progress::ProgressSink::Stderr,
    ));
    println!("\r{res}");
    Ok(())
}

/// CLI-миграция: apply <backup_dir> <target_dev>
/// (используется в основном через TUI, но доступно из консоли).
fn cmd_migrate(backup: Option<&String>, target: Option<&String>) -> std::io::Result<()> {
    let (Some(backup), Some(target)) = (backup, target) else {
        eprintln!("Использование: switch-migrator apply <backup_dir> /dev/sdX");
        std::process::exit(2);
    };
    let bd = or_die(disk::DiskFilter::validate_for_probe(target));
    let plan = operations::migrate::MigratePlan {
        target: bd,
        backup_dir: PathBuf::from(backup.as_str()),
    };
    let res = or_die(operations::migrate::run_migrate(
        &plan,
        utils::progress::ProgressSink::Stderr,
    ));
    println!(
        "\rГотово: новый sector={}, emummc.ini исправлен: {}, archive bits: {}",
        res.new_emu_start, res.emummc_ini_fixed, res.archive_bits_fixed
    );
    Ok(())
}

/// Запуск интерактивного TUI.
fn run_tui() -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = tui::app::App::new();
    let tick = Duration::from_millis(100);
    let result = (|| -> std::io::Result<()> {
        loop {
            terminal.draw(|f| tui::ui::draw(f, &app))?;
            app.poll();

            if event::poll(tick)? {
                match event::read()? {
                    event::Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if k.code == crossterm::event::KeyCode::Char('q')
                            && app.screen == tui::app::Screen::Devices
                        {
                            break;
                        }
                        if k.code == crossterm::event::KeyCode::Char('c')
                            && k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        {
                            break;
                        }
                        app.handle_key(k);
                    }
                    event::Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    result
}