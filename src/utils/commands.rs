//! # Обёртки над системными CLI-утилитами Linux
//!
//! Используем внешние бинари, но строго через `PATH` (никаких `/usr/bin/`).
//! Все вызовы аккуратно обрабатывают stderr/stdout и код возврата.

use std::process::Command;

use thiserror::Error;

/// Типы системных утилит, которые нужны для разметки/форматирования.
#[derive(Debug)]
#[allow(dead_code)]
pub enum SysTool {
    Sfdisk,
    Parted,
    MkfsVfat,
    Blockdev,
}

impl SysTool {
    /// Имя исполняемого файла (резолвится через PATH).
    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            SysTool::Sfdisk => "sfdisk",
            SysTool::Parted => "parted",
            SysTool::MkfsVfat => "mkfs.vfat",
            SysTool::Blockdev => "blockdev",
        }
    }
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("Утилита `{0}` не найдена в PATH. Установите {0}.")]
    NotFound(String),
    #[error("Команда `{cmd}` завершилась с кодом {code:?}. stderr: {stderr}")]
    Failed { cmd: String, code: Option<i32>, stderr: String },
}

/// Проверяет наличие утилиты в PATH.
#[allow(dead_code)]
pub fn which(tool: SysTool) -> bool {
    Command::new(tool.name())
        .arg("--version")
        .output()
        .map(|_| true)
        .unwrap_or(false)
}

/// Выполняет команду и возвращает stdout успеха или бросает ошибку.
pub fn run_capture(tool: &str, args: &[&str]) -> Result<String, CommandError> {
    let out = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CommandError::NotFound(tool.to_string())
            } else {
                CommandError::Failed {
                    cmd: format!("{tool} {:?}", args),
                    code: None,
                    stderr: e.to_string(),
                }
            }
        })?;

    if !out.status.success() {
        return Err(CommandError::Failed {
            cmd: format!("{tool} {}", args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Выполняет команду, подавая скрипт/данные на stdin (например, для `sfdisk`).
pub fn run_with_stdin(
    tool: &str,
    args: &[&str],
    stdin_data: &str,
) -> Result<String, CommandError> {
    use std::io::Write as _;
    use std::process::{Stdio};

    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CommandError::NotFound(tool.to_string())
            } else {
                CommandError::Failed {
                    cmd: format!("{tool} {:?}", args),
                    code: None,
                    stderr: e.to_string(),
                }
            }
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| CommandError::Failed {
        cmd: format!("{tool} {}", args.join(" ")),
        code: None,
        stderr: e.to_string(),
    })?;

    if !out.status.success() {
        return Err(CommandError::Failed {
            cmd: format!("{tool} {}", args.join(" ")),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Считывает размер блочного устройства (в байтах) через `blockdev --getsize64`.
#[allow(dead_code)]
pub fn blockdev_size(dev: &str) -> Result<u64, CommandError> {
    let out = run_capture("blockdev", &["--getsize64", dev])?;
    out.trim().parse::<u64>().map_err(|_| CommandError::Failed {
        cmd: format!("blockdev --getsize64 {dev}"),
        code: None,
        stderr: "неудачный парсинг числа".to_string(),
    })
}
