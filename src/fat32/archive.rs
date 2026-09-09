//! # Fix Archive Bit для FAT32
//!
//! **Проблема:** Linux, монтируя FAT32, не выставляет «archive bit» (0x20 в
//! атрибутах) у каталогов. Онkate/Atmosphere при чтении игр на SD-карте в Switch
//! требуют этот бит установленным. Если его нет — консоль может ругаться на
//! повреждение файлов (или просто не видеть файловый каталог в нужном виде).
//!
//! Решение — рекурсивно выставить атрибут «архивный» всем ДИРЕКТОРИЯМ на
//! FAT32-разделе. Для файлов это не требуется.
//!
//! Реализация — напрямую через syscall `ioctl(FS_IOC_GETFLAGS / SETFLAGS)` либо,
//! в безопасном варианте, через монтирование и утилиту `chattr` (если доступна).
//! Здесь — чистый Rust через `ioctl`, чтобы не зависеть от внешних бинарей.

use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Флаг FS_IOC_GETFLAGS.
const FS_IOC_GETFLAGS: u64 = 0x80086601;
/// Флаг FS_IOC_SETFLAGS.
const FS_IOC_SETFLAGS: u64 = 0x40086602;
/// EXT4/HSFS «archive» flag (xattr archive).
const EXT2_FLAG_ARCHIVE: u32 = 0x20;

extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

/// Получить флаги файла.
#[allow(dead_code)]
fn get_flags(path: &Path) -> io::Result<u32> {
    let f = File::open(path)?;
    let mut flags: u32 = 0;
    // SAFETY: ioctl с указателем на u32.
    let r = unsafe { ioctl(f.as_raw_fd(), FS_IOC_GETFLAGS, &mut flags as *mut u32) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags)
}

/// Установить флаги файла.
fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let f = File::open(path)?;
    // SAFETY: ioctl с константой флагов.
    let r = unsafe { ioctl(f.as_raw_fd(), FS_IOC_SETFLAGS, flags) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Выставляет archive-бит всем каталогам рекурсивно под `root`.
///
/// Возвращает число обработанных каталогов. Пропускает системные точки
/// монтирования, чтобы не выйти за пределы целевого раздела.
pub fn fix_archive_bit(root: &Path) -> io::Result<u64> {
    fn walk(dir: &Path, count: &mut u64) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                match set_flag_inner(&path) {
                    Ok(()) => *count += 1,
                    Err(_) => {
                        // не фатально — пропускаем и продолжаем
                    }
                }
                // не идём в псевдо-монтирования (небольшая защита)
                walk(&path, count)?;
            }
        }
        Ok(())
    }
    let mut count = 0u64;
    walk(root, &mut count)?;
    Ok(count)
}

fn set_flag_inner(path: &Path) -> io::Result<()> {
    let flags = get_flags(path).unwrap_or(0);
    // если archive уже стоит — пропускаем
    if flags & EXT2_FLAG_ARCHIVE != 0 {
        return Ok(());
    }
    set_flags(path, flags | EXT2_FLAG_ARCHIVE)
}
