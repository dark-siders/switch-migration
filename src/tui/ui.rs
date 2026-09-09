//! # Рендер TUI (ratatui)
//!
//! Рисует каждый экран приложения на основе состояния [`crate::tui::app::App`].

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::utils::human::human_bytes;

use super::app::{App, Screen};

/// Точка входа рендера — рисует только нужный экран.
pub fn draw(frame: &mut Frame, app: &App) {
    frame.render_widget(Clear, frame.area());
    match app.screen {
        Screen::Welcome => draw_welcome(frame),
        Screen::Devices => draw_devices(frame, app),
        Screen::Inspect => draw_inspect(frame, app),
        Screen::Backup => draw_busy(frame, app),
        Screen::BackupDone | Screen::MigrateDone => draw_done(frame, app),
        Screen::MigrateTarget => draw_target(frame, app),
        Screen::Migrate => draw_busy(frame, app),
    }
}

fn draw_welcome(frame: &mut Frame) {
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            " switch-migrator ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Перенос данных Nintendo Switch на новую SD-карту за 2 клика"),
        Line::from(""),
        Line::from("  [Enter]  — начать"),
        Line::from("  [Ctrl-C] — выход"),
    ];
    let p = Paragraph::new(text)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Добро пожаловать"));
    frame.render_widget(p, frame.area());
}

fn draw_devices(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let title = Paragraph::new("Выберите ИСХОДНУЮ (старую) SD-карту")
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = app
        .devices
        .iter()
        .map(|d| {
            ListItem::new(Line::from(format!(
                "{:<12} {:>9}  {}",
                d.path.display(),
                human_bytes(d.size_bytes),
                d.model.as_deref().unwrap_or("")
            )))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Накопители (безопасные)"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let hint = " [↑/↓] выбор  [Enter] анализ  [r] обновить  [q] выход";
    frame.render_widget(Paragraph::new(hint), chunks[2]);
}

fn draw_inspect(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

    if let Some(src) = &app.source_dev {
        let head = Paragraph::new(format!(
            "Карта: {}  ({})",
            src.path.display(),
            human_bytes(src.size_bytes)
        ))
        .style(Style::default().add_modifier(Modifier::BOLD));
        frame.render_widget(head, chunks[0]);
    }

    match &app.inspect {
        Some(insp) => {
            let emu = insp
                .emummc
                .as_ref()
                .map(|e| {
                    format!(
                        "EmuMMC: сектор {}, {} секторов ({} ГБ)",
                        e.emu_start,
                        e.emu_len,
                        human_bytes(e.emu_bytes)
                    )
                })
                .unwrap_or_else(|| "EmuMMC: не найдена".to_string());
            frame.render_widget(Paragraph::new(emu), chunks[1]);

            let games = format!(
                "Игры на FAT32: {} (всего {})",
                insp.games.len(),
                human_bytes(insp.games.iter().map(|g| g.size_bytes).sum())
            );
            frame.render_widget(Paragraph::new(games), chunks[2]);

            let list_items: Vec<ListItem> = insp
                .games
                .iter()
                .take(30)
                .map(|g| ListItem::new(format!("{}  {}", g.title_id, human_bytes(g.size_bytes))))
                .collect();
            let list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title("Найденные игры"));
            frame.render_widget(list, chunks[3]);
        }
        None => {
            frame.render_widget(
                Paragraph::new("Анализ карты...").alignment(Alignment::Center),
                chunks[0].union(chunks[3]),
            );
        }
    }

    let menu = " [1] Создать бэкап  [3] Мигрировать на другую карту  [q] назад";
    frame.render_widget(Paragraph::new(menu), chunks[4]);
}

fn draw_busy(frame: &mut Frame, app: &App) {
    let p = Paragraph::new(app.busy_msg.clone())
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Выполняется"));
    frame.render_widget(p, frame.area());
}

fn draw_done(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(frame.area());

    let p = Paragraph::new(app.last_result.clone())
        .alignment(Alignment::Center)
        .wrap(ratatui::widgets::Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Готово"));
    frame.render_widget(p, chunks[0]);

    let hint = Paragraph::new(" [Enter] продолжить  [q] назад").alignment(Alignment::Center);
    frame.render_widget(hint, chunks[1]);
}

fn draw_target(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let warn = Paragraph::new(
        Span::styled(
            "ВНИМАНИЕ: все данные на целевой карте будут стёрты!",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    );
    frame.render_widget(warn, chunks[0]);

    let items: Vec<ListItem> = app
        .devices
        .iter()
        .map(|d| {
            let is_source = Some(&d.path) == app.source_dev.as_ref().map(|s| &s.path);
            ListItem::new(Line::from(format!(
                "{:<12} {:>9}  {}",
                d.path.display(),
                human_bytes(d.size_bytes),
                if is_source {
                    "⚠ исходная — запрещено"
                } else {
                    d.model.as_deref().unwrap_or("")
                }
            )))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Целевая SD-карта"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let hint = " [↑/↓] выбор  [Enter] миграция  [Esc] назад";
    frame.render_widget(Paragraph::new(hint), chunks[2]);
}