//! # E2E-тест миграции на loop-устройствах (без реальных SD-карт)
//!
//! Полный цикл «в 2 клика» на виртуальных картах-файлах:
//!
//! 1. Создаём файл-образ «старой карты» (512 МБ) и «новой карты» (8 ГБ).
//! 2. На старой карте размечаем MBR: p1 = FAT16 (содержит `emummc/emummc.ini`
//!    и `Nintendo/...`), p2 = RAW (тип 0x27) с псевдо-случайным содержимым.
//! 3. Подключаем loop-устройства через `losetup -P`.
//! 4. Запускаем реальный бинарь `switch-migrator` (с тест-режимом `SM_ALLOW_LOOP=1`):
//!    `ls` → `probe` → `backup` → `apply`.
//! 5. Проверяем: таблицу разделов новой карты, побайтовое совпадение RAW-секции,
//!    `emummc.ini` с новым sector offset, перенос Nintendo-каталога, повторный
//!    `probe` новой карты (самодетекция EmuMMC после миграции).
//!
//! Требует root (loop + mount). Если root нет или недоступны утилиты — тест
//! «пропускается» (print + Ok), а не падает, чтобы `cargo test` работал в CI.

use std::path::Path;
use std::process::Command;

const OLD_SIZE_SECTORS: u64 = 1_048_576; // 512 МБ
const NEW_SIZE_SECTORS: u64 = 16_777_216; // 8 ГБ
const P1_START: u64 = 2048;
const OLD_P1_LEN: u64 = 32_768; // 16 МБ FAT16 на «старой» карте
const OLD_P2_START: u64 = P1_START + OLD_P1_LEN;
const P2_RESERVE_BYTES: u64 = 512 * 1024 * 1024; // 0.5 ГиБ — как в migrate.rs
const P2_ALIGN_SECTORS: u64 = 2048;                // 1 МиБ — как в migrate.rs

/// Убеждаемся, что тест можно запускать (root, loop-control, утилиты).
/// Если нет — печатаем причину и возвращаем `false` (тест тихо пропускается).
fn prerequisites_ok() -> bool {
    let root = Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false);
    if !root {
        eprintln!("e2e: пропуск — нужен root (запустите `sudo cargo test --test e2e_loop`).");
        return false;
    }
    if !Path::new("/dev/loop-control").exists() {
        eprintln!("e2e: пропуск — нет /dev/loop-control (loop-устройства недоступны).");
        return false;
    }
    for tool in [
        "losetup",
        "sfdisk",
        "mkfs.vfat",
        "mount",
        "umount",
        "wipefs",
        "partprobe",
        "partx",
        "blockdev",
    ] {
        if Command::new("which").arg(tool).output().is_err() {
            eprintln!("e2e: пропуск — утилита `{tool}` не найдена в PATH.");
            return false;
        }
    }
    true
}

/// Запускает команду, при неудаче паникует с выводом.
fn run(cmd: &str, args: &[&str]) -> String {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("не удалось запустить `{cmd}`: {e}"));
    if !out.status.success() {
        panic!(
            "`{cmd} {}` завершилась с кодом {:?}\nstdout: {}\nstderr: {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Подаёт скрипт на stdin команды (для sfdisk) и проверяет успех.
fn run_with_stdin(cmd: &str, args: &[&str], input: &str) {
    use std::io::Write as _;
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "`{cmd} {}` завершилась с кодом {:?}: {}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Запускает сам `switch-migrator` с включённым тест-режимом.
fn migrator(args: &[&str]) -> String {
    let bin = env!("CARGO_BIN_EXE_switch-migrator");
    run(bin, args)
}

/// Подключает образ к свободному loop-устройству с партишн-сканом.
fn attach(img: &Path) -> String {
    // `--show` печатает имя, например /dev/loop5
    let out = run("losetup", &["-f", "-P", "--show", img.to_str().unwrap()]);
    out.trim().to_string()
}

fn detach(loop_dev: &str) {
    let _ = Command::new("losetup").arg("-d").arg(loop_dev).status();
}

/// Ждёт, пока ядро/udev создадут узлы `{dev}p1`/`{dev}p2` после `losetup -P`.
///
/// Создание узлов асинхронно, и без ожидания `mkfs.vfat`/`mount` гоняются с
/// udev (та же проблема, что решена в `migrate.rs::ensure_partitions`).
fn wait_for_partition_nodes(dev: &str) {
    let p1 = format!("{dev}p1");
    let p2 = format!("{dev}p2");
    let _ = Command::new("udevadm").args(["settle", "--timeout=3"]).status();
    for _ in 0..50 {
        if Path::new(&p1).exists() && Path::new(&p2).exists() {
            return;
        }
        let _ = Command::new("partprobe").arg(dev).status();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("узлы `{p1}`/`{p2}` не появились после losetup -P");
}

/// Создаёт псевдо-случайный блок (xorshift) для «сырого» содержимого EmuMMC.
fn pattern_block(seed: u64, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x = seed.wrapping_add(0x9E3779B97F4A7C15);
    for _ in 0..len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push((x & 0xFF) as u8);
    }
    v
}

/// Парсит партиции из `sfdisk -d` (числа могут быть выровнены пробелами).
struct SfdiskPart {
    start: u64,
    type_str: String,
    bootable: bool,
}

fn parse_partition_table(out: &str) -> Vec<SfdiskPart> {
    let mut res = Vec::new();
    for line in out.lines() {
        if !line.contains("start=") {
            continue;
        }
        let start = extract_named(line, "start").and_then(|s| s.parse::<u64>().ok());
        let type_str = extract_named(line, "type").map(|s| s.to_string());
        if let (Some(start), Some(type_str)) = (start, type_str) {
            let bootable = line.contains("bootable");
            res.push(SfdiskPart { start, type_str, bootable });
        }
    }
    res
}

/// Достаёт значение `key=...` из строки вида
/// `"/dev/loop6p1 : start=        2048, size= 8387584, type=c, bootable"` —
/// ищем маркер по всей строке, а не после запятой (первое поле содержит префикс устройства).
fn extract_named<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("{key}=");
    let after = line.find(&marker)? + marker.len();
    let rest = &line[after..];
    let end = rest.find(',').unwrap_or(rest.len());
    let val = rest[..end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

#[test]
fn parses_sfdisk_dump_format() {
    let out = "label: dos\n\
               label-id: 0x9856d020\n\
               device: /dev/loop6\n\
               unit: sectors\n\
               sector-size: 512\n\
               \n\
               /dev/loop6p1 : start=        2048, size=     8387584, type=c, bootable\n\
               /dev/loop6p2 : start=     8389632, size=     8387584, type=27\n";
    let parts = parse_partition_table(out);
    assert_eq!(parts.len(), 2, "должны распарситься обе партиции");
    let p1 = &parts[0];
    assert_eq!(p1.start, 2048);
    assert!(p1.bootable);
    assert_eq!(p1.type_str, "c");
    let p2 = &parts[1];
    assert_eq!(p2.start, 8389632);
    assert!(!p2.bootable);
    assert_eq!(p2.type_str, "27");
}

/// Сравнивает первые `len` байт двух блочных устройств (стрим, без полной загрузки).
fn compare_region(a: &Path, b: &Path, len: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut fa = std::fs::File::open(a).unwrap();
    let mut fb = std::fs::File::open(b).unwrap();
    fa.seek(SeekFrom::Start(0)).unwrap();
    fb.seek(SeekFrom::Start(0)).unwrap();
    let mut buf_a = vec![0u8; 4 * 1024 * 1024];
    let mut buf_b = vec![0u8; 4 * 1024 * 1024];
    let mut remaining = len;
    while remaining > 0 {
        let want = buf_a.len().min(remaining as usize);
        fa.read_exact(&mut buf_a[..want]).unwrap();
        fb.read_exact(&mut buf_b[..want]).unwrap();
        if buf_a[..want] != buf_b[..want] {
            return false;
        }
        remaining -= want as u64;
    }
    true
}

/// Полный e2e-цикл на loop-устройствах. Пропускается без root/утилит.
#[test]
fn full_migration_loop() {
    if !prerequisites_ok() {
        return; // тихий пропуск
    }

    // ---- 1. Файлы-образы ----
    let dir = std::env::temp_dir().join(format!("sm-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old_img = dir.join("sd_old.img");
    let new_img = dir.join("sd_new.img");
    let backup = dir.join("backup");
    let _ = run("truncate", &["-s", "512M", old_img.to_str().unwrap()]);
    let _ = run("truncate", &["-s", "8G", new_img.to_str().unwrap()]);

    // ---- 2. Разметка «старой карты» (файл напрямую, до losetup) ----
    let dev1 = format!("{}1", old_img.display());
    let dev2 = format!("{}2", old_img.display());
    run_with_stdin(
        "sfdisk",
        &["--force", old_img.to_str().unwrap()],
        &format!(
            "label: dos\n\
             device: {dev}\n\
             {dev1} : start={p1}, size={p1len}, type=c, bootable\n\
             {dev2} : start={p2}, size={p2len}, type=27\n",
            dev = old_img.display(),
            p1 = P1_START,
            p1len = OLD_P1_LEN,
            p2 = OLD_P2_START,
            p2len = OLD_SIZE_SECTORS - OLD_P2_START,
        ),
    );

    // ---- 3. Подключение loop ----
    let old_loop = attach(&old_img);
    let new_loop = attach(&new_img);

    let old_p1 = format!("{old_loop}p1");
    let old_p2 = format!("{old_loop}p2");
    let new_p1 = format!("{new_loop}p1");
    let new_p2 = format!("{new_loop}p2");

    // ---- 4. Наполнение «старой карты» ----
    // Узлы loop6p1/p2 появляются асинхронно — ждём их перед mkfs.
    wait_for_partition_nodes(&old_loop);
    // p1: FAT16 с гостевой структурой Switch-карты.
    run("mkfs.vfat", &["-F", "16", "-n", "SRC", &old_p1]);
    let mnt_old = dir.join("mnt_old");
    std::fs::create_dir_all(&mnt_old).unwrap();
    run("mount", &["-o", "ro", &old_p1, mnt_old.to_str().unwrap()]);
    // Не можем писать в RO-монтаж — снимаем и монтируем RW.
    run("umount", &[mnt_old.to_str().unwrap()]);
    run("mount", &[&old_p1, mnt_old.to_str().unwrap()]);

    let mnt_old_str = mnt_old.to_str().unwrap();
    std::fs::create_dir_all(format!("{mnt_old_str}/Nintendo/Contents/registered/0100AB0A1004C000"))
        .unwrap();
    std::fs::write(
        format!("{mnt_old_str}/Nintendo/Contents/registered/0100AB0A1004C000/fst.bin"),
        vec![0xABu8; 1024 * 1024],
    )
    .unwrap();
    std::fs::create_dir_all(format!("{mnt_old_str}/emummc")).unwrap();
    std::fs::write(
        format!("{mnt_old_str}/emummc/emummc.ini"),
        format!(
            "[emummc]\nenabled=1\nsector=0x{OLD_P2_START:X}\nid=000\nnintendo_path=Nintendo\nemummc_path=emummc\nemummc_type=partition\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(format!("{mnt_old_str}/bootloader")).unwrap();
    std::fs::write(format!("{mnt_old_str}/bootloader/payload.bin"), b"HEKATE-TEST").unwrap();
    run("umount", &[mnt_old_str]);

    // p2: RAW EmuMMC — псевдо-случайное содержимое в начале, остальное нули.
    {
        use std::io::{Seek, SeekFrom, Write as _};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&old_p2)
            .unwrap();
        let pat = pattern_block(42, 8 * 1024 * 1024);
        f.write_all(&pat).unwrap();
        // «дырявые» нули ниже — образ разреженный, восстановится корректно.
        f.seek(SeekFrom::Start(8 * 1024 * 1024 + 4096)).unwrap();
        f.write_all(b"EMMC-SIGNATURE").unwrap();
        f.flush().unwrap();
    }

    // ---- 5. ls + probe + backup + apply ----
    let ls_out = migrator(&["--allow-loop", "ls"]);
    assert!(
        ls_out.contains(&old_loop) && ls_out.contains(&new_loop),
        "ls не увидел loop-устройства:\n{ls_out}"
    );

    let probe_out = migrator(&["--allow-loop", "probe", &old_loop]);
    assert!(
        probe_out.contains("EmuMMC:") && probe_out.contains(&OLD_P2_START.to_string()),
        "probe не распознал EmuMMC: {probe_out}"
    );

    let backup_out =
        migrator(&["--allow-loop", "backup", &old_loop, "--out", backup.to_str().unwrap()]);
    assert!(
        backup_out.contains("Бэкап:"),
        "backup не отработал: {backup_out}"
    );
    assert!(backup.join("manifest.json").exists(), "нет manifest.json");
    let img_files: Vec<_> = std::fs::read_dir(&backup)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("emummc.img."))
        .collect();
    assert_eq!(img_files.len(), 1, "ожидался один образ EmuMMC");

    let apply_out = migrator(&[
        "--allow-loop",
        "apply",
        backup.to_str().unwrap(),
        &new_loop,
    ]);
    assert!(
        apply_out.contains("emummc.ini исправлен: true"),
        "ожидали фикс sector offet в emummc.ini:\n{apply_out}"
    );

    // ---- 6. Проверки новой карты ----
    // Ожидаемая геометрия новой карты — адаптивная разметка (как в migrate.rs).
    let emu_len_sectors = OLD_SIZE_SECTORS - OLD_P2_START;
    let emu_bytes = emu_len_sectors * 512;
    let need = emu_len_sectors
        .saturating_add(P2_RESERVE_BYTES / 512)
        .div_ceil(P2_ALIGN_SECTORS)
        * P2_ALIGN_SECTORS;
    let available = NEW_SIZE_SECTORS - P1_START;
    let new_p2_len = need.min(available);
    let new_p2_start = P1_START + (available - new_p2_len);

    // Таблица разделов: парсим численно — sfdisk выравнивает числа пробелами.
    let table = run("sfdisk", &["-d", &new_loop]);
    let parts = parse_partition_table(&table);
    let p1 = parts.first().unwrap_or_else(|| {
        panic!(
            "sfdisk -d не отдал ни одной партиции новой карты:\n{table}\n\nversion: {}",
            run("sfdisk", &["--version"])
        )
    });
    assert!(
        p1.bootable && p1.start == P1_START,
        "p1 не FAT32 bootable на старте {P1_START}:\n{table}"
    );
    let p2 = parts.get(1).expect("ожидался p2");
    assert!(
        p2.start == new_p2_start && p2.type_str == "27",
        "ожидали p2 start={new_p2_start}, type=27:\n{table}"
    );

    // RAW-секция новой карты совпадает с исходной побайтово.
    assert!(
        compare_region(Path::new(&old_p2), Path::new(&new_p2), emu_bytes),
        "RAW-секция новой карты не совпала с исходной"
    );

    // Монтируем новую карту и проверяем содержимое + ini.
    let mnt_new = dir.join("mnt_new");
    std::fs::create_dir_all(&mnt_new).unwrap();
    run("mount", &[&new_p1, mnt_new.to_str().unwrap()]);
    let mnt_new_str = mnt_new.to_str().unwrap();

    assert!(
        Path::new(&format!("{mnt_new_str}/Nintendo/Contents/registered/0100AB0A1004C000"))
            .is_dir(),
        "Nintendo-каталог не перенесён"
    );
    assert_eq!(
        std::fs::read(format!("{mnt_new_str}/bootloader/payload.bin")).unwrap(),
        b"HEKATE-TEST",
        "payload.bin не перенесён"
    );

    // Sector offset в ini исправлен на старт p2 новой карты.
    let ini = std::fs::read_to_string(format!("{mnt_new_str}/emummc/emummc.ini")).unwrap();
    assert!(
        ini.contains(&format!("sector=0x{:X}", new_p2_start))
            || ini.contains(&format!("sector={new_p2_start}")),
        "emummc.ini не содержит новый sector {new_p2_start}:\n{ini}"
    );
    run("umount", &[mnt_new_str]);

    // Самодетекция: повторный probe новой карты видит EmuMMC по исправленному MBR.
    let probe2 = migrator(&["--allow-loop", "probe", &new_loop]);
    assert!(
        probe2.contains(&new_p2_start.to_string()),
        "после миграции probe не видит EmuMMC на старте {new_p2_start}: {probe2}"
    );

    // ---- 7. Очистка ----
    detach(&new_loop);
    detach(&old_loop);
    std::fs::remove_dir_all(&dir).ok();
}