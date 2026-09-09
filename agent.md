# AGENTS.md — switch-migrator

Описательный файл контекста проекта для рабочего агента (opencode).

> **Расположение:** `/home/darkside/switch-migration/` — отдельная папка, НЕ
> часть репозитория `nixosniri`. Проект автономный, со своим flake и git-историей.

---

## 0. Кратко о проекте

- **Название:** switch-migrator.
- **Назначение:** утилита переноса данных Nintendo Switch на новую (более крупную)
  SD-карту «в два клика»: бэкап (клик 1) → миграция (клик 2).
- **Язык:** Rust (бинарник), TUI на **ratatui** + crossterm.
- **Целевая платформа:** Linux, в первую очередь NixOS. Сборка через Nix flake.
- **Коммуникация:** с владельцем — только русский. Код и комментарии — русские
  (допустимы англ. термины в коде).
- **Статус:** MVP с рабочим каркасом всех модулей; низкоуровневые операции с
  дисками требуют осторожного тестирования на реальном железе.

## 0.1 Статус (актуально на 2026-09-09)

- **Уровень 0 (loop-диски): ПРОЙДЕН.** Полный e2e-цикл на реальном прогоне:
  `parses_sfdisk_dump_format ok` + `full_migration_loop ok` (~3.85s под root).
  Миграция loop→loop: разметка, бэкап, apply, побайтовая сверка RAW,
  перенос `Nintendo/`, фикс `emummc.ini`, самодетекция — всё зелёное.
  Ещё 15 юнит-тестов (детект, ini, окно дампа), clippy 0.
- **Уровень 1 (реальные данные): dry-путь ПРОЙДЕН и проверен побайтово.**
  Основная карта 128 ГиБ: полный цикл `ls` → `probe` → `backup` → `apply`
  (последний — на loop-образ 256 ГиБ). Результат: выравнивание данных в
  offset 0, `sector→268436480`, **sha256 совпал** (`48bb307d…d57ad2`).
  Реальная запись `apply` на вторую физическую карту (512 ГиБ) на ноуте
  заблокирована АППАРАТНО: битая RAM ноутбука (см. «Новая карта 512 ГиБ…»).
  Продолжение — на другой машине.
- **Уровень 2 (реальный Switch + Hekate): ждёт Уровень 1.**
- ⚠️ Выбор `type=27` для p2 (см. §7) ещё предстоит подтвердить на реальной карте
  в Hekate/ОС — на loop-тесте этот аспект не проверяется.
- **Порядок работ (решение владельца):** СНАЧАЛА довести до ума и проверить
  **CLI** (операции ls/probe/backup/apply) на реальном железе — Уровни 1–2.
  ПОТОМ полировать **TUI** (прогресс-бары, экраны подтверждения, обработка
  ошибок — сейчас только busy-экран). TUI — тонкая обёртка над CLI; его
  полировка до подтверждения корректности операций на реальной карте
  нецелесообразна.

### Реальная карта (основная, 128 ГиБ): статус тестирования 2026-09-09

- Карта: `/dev/mmcblk0` (SD128, встроенный PCIe-ридер Realtek), 116.5 ГиБ,
  244277248 секторов. Внутренний еMMC (`type=="MMC"`) и NVMe — запрещены.
- p1: FAT32, start=32768; p2: RAW EmuMMC, start=229507072, len=14743552,
  **type=e0** (Hekate), games 0.
- `emummc.ini`: `sector=0xdae8000` = **229539840** (= p2_start + 32768 = +16 МиБ),
  raw-based (`emuMMC/RAW1/raw_based`).
- **ВЫЯВЛЕН БАГ (уровень 0 не ловит):** фактическая ФС EmuNAND
  начинается НЕ с начала p2, а с сектора из ini (+16 МиБ нулей внутри раздела).
  Старый бэкап дампил раздел с offset 0 → после переноса данные на новой карте
  оказывались на `new_p2_start + 32768`, а ini выставлялся на `new_p2_start` →
  Hekate смотрел бы в пустоту. Исправлено (см. журнал, п.6).
- Чтение `/dev/mmcblk0p*` доступно пользователю (группа `disk`); запись/монтирование — root.
- Бэкапы и образы: `~/switch-backup` (39 ГиБ) — **УДАЛЁН владельцем 2026-09-09** для
  освобождения места; на смену идёт `~/switch-backup-fixed` (39 ГиБ, фиксированный
  дамп окна, root-owned — `chown` в очереди действий).
- **«Сухая» миграция ПОДТВЕРЖДЕНА 2026-09-09:** заново созданный бэкап по фиксу
  развёрнут на loop-образ 256 ГиБ и проверен — данные в offset 0 p2,
  `sector→268436480`, sha256 побайтово совпал. Реальный `apply` на вторую карту
  остаётся за Уровнем 1 (нет карты большей ёмкости).

### Новая карта 512 ГиБ и провалы apply на ноуте (битая RAM) — 2026-09-09

- **Новая карта:** `/dev/mmcblk0` (в ридере ноута), 477.5 ГиБ (512711720960 B),
  exFAT из плеера. **f3probe `--destructive`**
  (`nix shell nixpkgs#f3 -c sudo f3probe --destructive /dev/mmcblk0`) →
  «Good news: the device is the real thing», cache 0, probe 44.96 s. CID
  клонированный (Samsung manfid 0x1b, имя `EF8S5`), но карта настоящая.
  Глубокие записи/readback с pattern на 6G/7180M/7190M/10240M/50000M (p2) и
  65536M/209715M (p1) — OK. **Карта НЕ виновата.** Текущее содержимое —
  следы неудачных apply (p1+p2 по 238.7 ГиБ), не забудем, что после f3probe
  она и так была зачищена.
- **3 провала `apply` на ноуте** с `Input/output error (os error 5)` (restore /
  copy / restore). Диагностика:
  - `dmesg`/`journalctl -k`: НИ ОДНОЙ mmc/sd ошибки на карту; зато
    `BTRFS: csum failed … corrupt 105→114` на `nvme0n1p2` (системный диск)
    ровно в моменты apply; `traps: rustc general protection fault`.
  - **`stressapptest -M 5500 -s 600` (пакет `nixpkgs#stressapptest`) → FAIL**
    («Status: FAIL — test discovered HW problems»): `miscompare` в одной узкой
    области памяти (~28 КиБ, биты залипают `0xfffd…` вместо `0xffff…`),
    CPU 3/5/6, «DIMM Unknown». Запускать желательно с количеством потоков по
    умолчанию (флаг `-W 8` отсутствует → «Unknown argument 8»).
  - **Вывод:** EIO возвращает BTRFS, когда читает участок файла бэкапа,
    повреждённый битой RAM в транзите; краши rustc (SIGSEGV/ICE/GPF) — те же
    битфлипы. Аппаратная проблема ноутбука (RAM/NVMe), **не код и не карта**.
  - **Данные бэкапа при этом ВАЛИДНЫ** (проверено сразу после падений):
    `zstd -t` — фрейм цел; `zstd -d -c … | sha256sum` = `48bb307d…d57ad2`
    (== эталон dry-миграции); полное чтение файла 3× подряд — без ошибок.
    Корру-ность была прерывистой и «ушла» — типичное поведение битой памяти.
- **Фикс сборки в песочнице Nix:** `RUST_MIN_STACK = "33554432";` в `flake.nix`
  (SIGSEGV rustc на `syn`/`libc` под нехваткой стека/памяти; на ноуте 7.4 ГиБ
  RAM · 8 ядер). Помогло; собирается и с `--cores 1`. Закоммичено `c77fa27`
  вместе с фиксом гарантированного umount в `migrate.rs`.
- **РЕШЕНИЕ ВЛАДЕЛЬЦА (2026-09-09):** миграцию доворачивать на ДРУГОМ компе —
  карта 512 ГиБ вставляется туда, репо берётся клоном, бэкап делается со
  Switch/карты, затем `apply` и полная сверка по §7. На ноуте сначала чинить
  память (переборка/замена/сервис), иначе длительные операции ненадёжны.

### Журнал bring-up e2e (что было исправлено по ходу)

1. **`migrate.rs::ensure_partitions`** — после `sfdisk` утилита раньше вызывала
   `partprobe` и сразу шла к `mkfs.vfat`; узел `/dev/loopNp1` ядро/udev создают
   асинхронно → `mkfs: unable to open ...: No such file or directory`. Теперь:
   `partprobe` + `partx -a` + `udevadm settle` + поллинг узлов p1/p2 (до 5 с).
   В обязательный набор утилит добавлены `partprobe`, `partx`.
2. **`tests/e2e_loop.rs::parse_partition_table`** — падал со «sfdisk -d не отдал
   партиций»: первое поле строки содержит префикс устройства
   (`/dev/loop6p1 : start= 2048...`), а код делал `strip_prefix("start=")` на
   всём поле → `start` = None у каждой партиции. Теперь `extract_named()` ищет
   маркер `key=` по всей строке. Плюс юнит-тест `parses_sfdisk_dump_format`
   (в бин-ре тестах, работает без root).
3. **`wait_for_partition_nodes()`** — гонка уже у самого теста: после
   `losetup -P` узлы старой карты появляются асинхронно, `mkfs.vfat`
   стартовал раньше → то же «unable to open». Теперь ждём оба узла (~5 с).
4. **Тип p2 в миграции: `0x0C` → `0x27`** (скрытый RAW, не FAT32), чтобы
   собственный `probe` после миграции не принял p2 за FAT32. Сверить на
   реальной карте (тип не влияет на чтение raw-секторов, только на отображение).
5. **Уборка**: после падений в `/var/tmp/sm-e2e` могут накапливаться
   `nix-shell.*` и висеть loop-диски → чистить `losetup -d` + `rm -rf` (см. §7).
6. **БАГ «+16 МиБ» выравнивания EmuNAND (пойман на РЕАЛЬНОЙ карте, сухой миграцией):**
   данные EmuNAND начинаются не с начала raw-раздела, а с сектора из
   `emummc.ini` (Hekate резервирует заголовок/выравнивание, здесь +32768 сек.).
   `backup.rs` дампил раздел с offset 0 → restore писал на новую карту в offset 0
   → ini правился на `new_p2_start`, а данные лежали на `new_p2_start + 32768`.
   **Фикс:** `backup.rs` теперь сначала копирует `sdmc/`, читает `emuMMC/emummc.ini`
   из копии и дампит ТОЛЬКО окно от ini-сектора (`resolve_dump_window`): offset =
   `ini.sector - emu_start`, len = `end - ini.sector`; имя файла
   `emummc.img.<ini.sector>.zst`; манифест отражает реальное окно. Без ini
   (или сектор вне раздела) — дамп всего раздела как раньше. `migrate.rs`
   не менялся (restore в offset 0 + ini = новый p2_start теперь сходятся).
   3 юнит-теста (`dump_window_*`); всего 15 unit + 2 e2e, clippy 0.

## 0.2 Ключевые команды

- Сборка/запуск TUI: `nix run .` / `nix run . -- ls`
- Dev-shell: `nix develop`
- Rust-сборка без nix: `cargo build` (нужен `rust` через nix-shell/rustup)
- Проверка стиля: `cargo clippy`, тесты: `cargo test`
- Юнит-тесты требуют ЛИШЬ чистого Rust; НЕ трогают реальные диски.
- E2E-тест на loop-дисках: `sudo cargo test --test e2e_loop`
  (полный цикл «бэкап → миграция → сверка sha256» на виртуальных картах;
  без root тихо пропускается).
- Бэкап реальной карты: `sudo nix run . -- backup /dev/mmcblk0 --out ~/switch-backup`
- «Сухая» миграция на loop-образ (без реальной второй карты):
  `truncate -s 256G ~/newcard.img && sudo losetup -f -P --show ~/newcard.img`
  `sudo env SM_ALLOW_LOOP=1 nix run . -- --allow-loop apply ~/switch-backup /dev/loopN`
  Проверка выравнивания: `dd if=/dev/loopNp2 bs=512 count=1 | od -A x -t x1z`
  → строка НЕ из нулей (файл), первый байт `00 08 00 00` = FAT-заголовок EmuNAND.
  Уборка после: `sudo losetup -d /dev/loopN && rm -f ~/newcard.img`.
- **Отработанная процедура запуска e2e** (NixOS, `/tmp` = tmpfs):
  1. Собрать бин-рь теста обычным пользователем:
     `nix develop -c cargo test --test e2e_loop --no-run`
     → путь `Executable tests/e2e_loop.rs (target/debug/deps/e2e_loop-<hash>)`.
     Хэш в имени = метаданные компиляции (тулчейн), а НЕ код: после правок
     исходников он НЕ меняется — собирать всё равно нужно (`Compiling ...`).
  2. Запустить под root без записи в target:
     `sudo TMPDIR=/var/tmp/sm-e2e nix develop -c target/debug/deps/e2e_loop-<hash> --nocapture`
     (TMPDIR обязателен: образы ~8.5 ГиБ разреженные, в tmpfs не влезут).
  3. После прогона почистить хвосты:
     `sudo bash -c 'for l in /dev/loop0 /dev/loop1 /dev/loop2 /dev/loop3 /dev/loop4 /dev/loop5 /dev/loop6 /dev/loop7; do losetup -d "$l" 2>/dev/null; done'`
     `sudo rm -rf /var/tmp/sm-e2e/nix-shell.* /var/tmp/sm-e2e/nix-develop-*`
     Проверка: `losetup -a` пуст, `ls /var/tmp/sm-e2e` пуст. Упавшие прогоны
     НЕ успевают отцепить loop — хвосты копятся после каждой паники.
- Тест-режим loop-устройств: флаг `--allow-loop` или переменная `SM_ALLOW_LOOP=1`
  (для разработки/тестов; в обычном режиме loop-устройства запрещены Safety Guard).

## 1. Технологический стек

- **Rust 2021**, edition 2021.
- **ratatui 0.28** + **crossterm 0.28** — TUI.
- **zstd 0.13** — сжатие образов EmuMMC «на лету».
- **serde/serde_json** — JSON-манифест бэкапа.
- **thiserror** — ошибки; **indicatif** — прогресс-бары.
- **Внешние системные утилиты** (вызываются по PATH, не `/usr/bin/`): `sfdisk`,
  `wipefs`, `partprobe`, `partx`, `udevadm`, `mkfs.vfat`, `mount`, `umount`,
  `blockdev`, `dd`. Все включаются в Nix-пакет через `runtimeTools` в `flake.nix`
  (`partprobe`/`partx` — из `util-linux`).

## 2. Архитектура (модули)

```
switch-migration/
├── flake.nix            # Nix flake: nix run / nix develop / nix build
├── Cargo.toml, Cargo.lock
├── agent.md             # этот файл
├── README.md            # пользовательская документация
├── tests/
│   └── e2e_loop.rs      # E2E «бэкап → миграция» на loop-устройствах (root)
└── src/
    ├── main.rs          # точка входа + CLI-режимы (ls/probe/backup/apply) + TUI loop
    ├── disk/
    │   ├── mod.rs       # SECTOR_SIZE, re-export BlockDevice/DiskFilter
    │   ├── detect.rs    # [ЯДРО 1] обнаружение блочных устройств + Safety Guard
    │   └── partition.rs # [ЯДРО 2] парсинг MBR, классификация разделов,
    │                    #          EmuGeometry (расчёт смещений секторов)
    ├── emummc/
    │   ├── mod.rs
    │   ├── ini.rs       # [ЯДРО 3] чтение/корректировка emummc.ini (sector=)
    │   └── imaging.rs   # дамп RAW-раздела в .img.zst + восстановление
    ├── fat32/
    │   ├── mod.rs
    │   ├── games.rs     # сканер игр (Nintendo/Contents/registered)
    │   └── archive.rs   # Fix Archive Bit (ioctl FS_IOC_GETFLAGS/SETFLAGS)
    ├── manifest.rs      # JSON-манифест бэкапа
    ├── operations/
    │   ├── mod.rs
    │   ├── inspect.rs   # анализ карты (обёртка над партициями + сканер)
    │   ├── backup.rs    # операция «Бэкап»
    │   └── migrate.rs   # операция «Миграция» + auto-fix sector offset
    ├── tui/
    │   ├── mod.rs
    │   ├── app.rs       # состояние/экраны/фоновые worker-потоки
    │   └── ui.rs        # рендер каждого экрана
    └── utils/
        ├── mod.rs
        ├── commands.rs  # обёртки над CLI-утилитами (run_capture, stdin)
        └── human.rs     # человекочитаемые размеры
```

## 3. Поток данных (Data Flow)

### Режим «Бэкап» (клик 1)
1. TUI → список безопасных устройств (`DiskFilter::safe_devices`).
2. Выбор карты → `operations::inspect::inspect_disk` в фоновом потоке:
   - `parse_mbr` → таблица; ищем разделы FAT32 и «не-FAT32» (RAW EmuMMC).
   - RAW-раздел → `EmuGeometry { fat_start, emu_start, emu_len, emu_bytes }`.
   - FAT-раздел монтируется RO (`mount -o ro`), сканируется
     `Nintendo/Contents/registered` → список игр.
3. `operations::backup::run_backup`:
   - дамп RAW-раздела через `dump_emummc` (zstd, блок 4 МБ)
     → `emummc.img.<sector>.zst`;
   - копирование sdmc-файлов в `sdmc/`;
   - генерация `manifest.json` (`BackupManifest`).
4. Итоговая папка бэкапа: `manifest.json` + `emummc.img.*.zst` + `sdmc/`.

### Режим «Миграция» (клик 2)
1. Выбор целевой (новой) карты в `MigrateTarget`.
2. `operations::migrate::run_migrate`:
   - `wipefs -a` + `sfdisk` (MBR: p1 FAT32 0x0C boot, p2 RAW 0x27);
   - `mkfs.vfat -F 32 -S 512 -s 64` (кластер 32 КБ) на p1;
   - `restore_emummc` — разворачивание `.img.zst` в p2;
   - mount p1 (RW), копирование `sdmc/`;
   - **auto-fix sector offset:** `EmummcIni::load` + `set_sector_and_save(new_emu_start)`;
   - `fix_archive_bit` на новой FAT32;
   - unmount + `sync_disk`.
3. Ключ к миграции — **новый стартовый сектор RAW-раздела** (p2), который
   отличается от старого (другая ёмкость карты → другая геометрия FAT32).

## 4. Ключевые алгоритмы

### 4.1 Обнаружение дисков (Safety Guard)
- Читаем `/sys/class/block/<name>/removable`. `1` → кандидат.
- Имена-разделы отсекаются (`sda1`, `mmcblk0p1`, `nvme0n1p1`, но НЕ `loop0`).
- `classify` возвращает `Removable` (можно) или `Forbidden` (всё прочее —
  внутренние SATA/NVMe/eMMC). Работа с не-removable СТРОГО запрещена.
- **Тест-режим:** при `--allow-loop` / `SM_ALLOW_LOOP=1` loop-устройства
  (`losetup` на файлах-образах) классифицируются как `Removable`. Это
  единственная возможность протестировать цикл без реальной SD.
- **Имена разделов:** `partition_path()` учитывает соглашения (`sda1` vs
  `mmcblk0p1`/`loop0p1`/`nvme0n1p1`) — сначала ищет в `/sys/class/block`,
  иначе эвристика по последней цифре имени. Без этого loop/mmcblk читались бы
  как `/dev/loop02` / `/dev/mmcblk02` (баг исправлен).

### 4.2 Расчёт Sector Offset
- Источник истины — реальный стартовый сектор раздела из MBR и `/sys/class/block`.
- Старый sector берётся из `emummc.ini`, новый — выводится из разметки новой
  карты: `p2_start = 2048 + p1_len` (после создания разделов).
- Формат записи в `emummc.ini` сохраняется (hex `0x...` или decimal), как в оригинале.

### 4.3 Пишем RAW-образ
- Дамп считывает РАЗДЕЛ EmuMMC (реальный блок `/dev/sdXN`), но НЕ всегда с его
  offset 0: окно дампа определяется из `emummc.ini` (`resolve_dump_window`),
  т.к. Hekate может резервировать заголовок/выравнивание и данные EmuNAND
  начинаются с сектора из ini (+16 МиБ на реальной карте). Без ini — весь раздел.
- Восстановление пишет в раздел p2 новой карты с offset 0 — данные после бэкапа
  из ini-сектора ложатся ровно на начало p2, ini правится на `new_p2_start`,
  инвариант сходится (баг из журнала п.6 закрыт).
- `emummc.ini` правится отдельно, чтобы Hekate знал новый `sector`.

## 5. GNU/Внешние зависимости

| Команда       | Назначение                    | Пакет Nix        |
|---------------|-------------------------------|------------------|
| `sfdisk`      | разметка MBR                  | `util-linux`     |
| `wipefs`      | затирание старых меток        | `util-linux`     |
| `partprobe`   | перечитать таблицу            | `util-linux`     |
| `partx`       | перечитать/обновить партиции  | `util-linux`     |
| `udevadm`     | дождаться создания узлов      | systemd (в системе) |
| `mkfs.vfat`   | FAT32 512B/32K-кластер        | `dosfstools`     |
| `mount/umount`| монтирование sdmc             | `util-linux`     |
| `blockdev`    | размеры устройств             | `util-linux`     |
| `which`       | проверка наличия утилит       | `which`          |

Все — в составе `runtimeTools` в `flake.nix`, доступны через PATH в `nix run .`.
Для ручного запуска: `sudo` не требуется только для чтения; запись/разметка
требуют root. В TUI используется `mount`/`mkfs.vfat` — запускайте под root
или с настроенным `polkit`/sudo.

## 6. Известные ограничения и TODO

- **НЕ ТЕСТИРОВАНО на реальном железе Switch (только dry-apply на loop).**
  Реальная карта: детект/probe/бэкап работают; сигнатура EmuNAND +16 МиБ учтена
  (журнал п.6). Реальный `apply` на вторую карту и запуск в Hekate — впереди.
- `MigrateTarget` показывает список устройств; поток «назад» и сообщение об
  ошибке реализованы, но UX не доведён до «красивого» (нет живого прогресса
  баров EmuMMC в TUI — только busy-экран).
- Часть операций монтирования делается «по требованию»; если раздел уже
  смонтирован системой (например, файловым менеджером), нужно сначала его
  размонтировать.
- `fat32::archive::fix_archive_bit` использует прямой `ioctl`; на экзотических
  ФС (не FAT32) может быть небезопасно — вызывается только на FAT32-разделе
  новой карты.
- Сканирование игр по title_id — корректно, но не показывает имена игр
  (маппинг title ID → имя не реализован; это можно сделать через
  HBL/API позже).
- GUID/GPT-карты не распознаются (`parse_mbr` ожидает MBR-сигнатуру). SD-карты
  Switch по умолчанию MBR — приемлемо.

## 7. Тестирование

### Уровень 0 — loop-диски (безопасно, на любом ПК)
**ПРОЙДЕН 2026-09-09** (`full_migration_loop ok`). Полный цикл без реальной
SD-карты, на файлах-образах:

```bash
sudo cargo test --test e2e_loop        # автоматический E2E
# или вручную:
truncate -s 512M old.img && truncate -s 8G new.img
losetup -f -P --show old.img           # → /dev/loopN
SM_ALLOW_LOOP=1 nix run . -- --allow-loop probe /dev/loopN
```

E2E-тест: размечает «старую» карту (p1 FAT16 + p2 RAW 0x27 с псевдо-содержимым),
подключает loop, прогоняет `ls` → `probe` → `backup` → `apply`, затем сверяет:
таблицу разделов новой карты, побайтовое совпадение RAW-секции, исправление
sector в `emummc.ini`, перенос `Nintendo/`, повторную самодетекцию EmuMMC.

### Уровень 1 — реальные, но неважные карты
**СОСТОЯНИЕ: dry-путь ПОДТВЕРЖДЁН (2026-09-09).** Основная карта 128 ГиБ прошла
`ls` → `probe` → `backup` → `apply` (последний на loop-образе 256 ГиБ) с
побайтовой сверкой и корректным сектором ini. Реальная карта 512 ГиБ уже
есть, но запись `apply` на ноуте упёрлась в АППАРАТНУЮ проблему — битую RAM
(см. «Новая карта 512 ГиБ…»); переносится на другой комп. Read-only шаги:
`ls`, `probe`. Команды:

```bash
nix run . -- ls                                  # детект removable (БЕЗ sudo)
sudo nix run . -- probe /dev/sdX                 # подтвердить EmuMMC
sudo nix run . -- backup /dev/sdX --out ~/switch-backup-fixed
la -la ~/switch-backup-fixed                     # manifest.json + emummc.img.N
sudo nix run . -- apply ~/switch-backup-fixed /dev/sdY # ТОЛЬКО на ненужную карту
```

Проверка нового бэкапа ОБЯЗАТЕЛЬНА (манифест отражает РЕАЛЬНОЕ окно, а не
раздел):
- `jq '.emummc' ~/switch-backup-fixed/manifest.json` → `start_sector`
  == sector из `emummc.ini` источника (229539840), `num_sectors` ==
  `(start_p2 + len_p2 - start_ini)` (14710784 на реальной карте);
- `zstd -dc emummc.img.<start_ini>.zst | od -A x -t x1z -N512` → первый байт
  НЕ нулевой (`00 08 00 00` = FAT-заголовок EmuNAND).

Сверка до вставки в Switch:
- `sfdisk -d /dev/sdY` — p1 bootable c at 2048, p2 type 27;
- `fsck.fat -v /dev/sdY1` — «0 верных, 0 исправлено»;
- `sha256sum` образа из бэкапа == `dd if=/dev/sdY2 bs=1M | sha256sum`
  (сравниваемый объём = длина зоны дампа из манифеста, секторов × 512);
- `emummc.ini` → sector равен старту p2; `lsattr -d /mnt/Nintendo` — archive bit.

### Уровень 2 — реальный Switch
После успеха на запасных картах: миграция старой→новой, вставить в Switch:
Hekate видит EmuMMC, запуск EmuNAND, игры запускаются без «переустановки»,
сохранения на месте, перезагрузка в payload работает.

- "Бит" на реальной карте: p2 имеет **type=e0**, мы при миграции пишем **0x27**.
  Чтение raw-секторов не зависит от типа, но в Hekate/ОС p2 будет виден как 27.
  РЕШИТЬ: оставить 27 (наш анализатор не примет p2 за FAT32) или перейти на e0
  (максимальное соответствие оригиналу). Зафиксировано как открытый вопрос.

### Очередь действий (что дальше, 2026-09-09)

1. ✅ Удалён старый бэкап `~/switch-backup` (39 ГиБ) — освободить место.
2. ✅ Повторный бэкап по фику выполнен:
   `sudo nix run . -- backup /dev/mmcblk0 --out ~/switch-backup-fixed`
   → образ `emummc.img.229539840.zst` (2.5 ГиБ), sdmc/ (37 ГиБ).
3. ✅ Манифест верен: `start_sector=229539840` (ini), `num_sectors=14710784`
   (окно до конца раздела), `size_bytes=7531921408`. Первый сектор образа —
   данные EmuNAND (`00 08 00 00...`), не нули.
4. ✅ «Сухая» миграция на свежий loop-образ 256 ГиБ (создан `truncate -s 256G`,
   подключён как `/dev/loop0`):
   `sudo env SM_ALLOW_LOOP=1 nix run . -- --allow-loop apply ~/switch-backup-fixed /dev/loop0`
   → `emummc.ini: sector 229539840 -> 268436480`, `исправлен: true`, archive bits 0.
5. ✅ Проверки выравнивания: `dd if=/dev/loop0p2 bs=512 count=1` → данные в
   offset 0 (паттерн `00 08 00 00...`), p2 start=268436480 type=27.
   **SHA256 совпал побайтово** (`48bb307d...d57ad2`): распакованный образ ==
   содержимое loop0p2 (7 ГиБ).
6. ✅ Уборка: `sudo losetup -d /dev/loop0 && rm -f ~/newcard.img` (44 ГиБ freed),
   сделано владельцем. Fинальный safety net — `~/switch-backup-fixed` (~39 ГиБ).
7. ⬜ Решить type p2: `0x27` (сейчас) vs `0xe0` (как у Hekate на реальной карте).
8. ⬜ **Реальный apply перенесён на другой комп** (на ноуте — битая RAM,
   см. «Новая карта 512 ГиБ…»): карта 512 ГиБ + клон репо на чистой машине →
   `backup` со Switch/карты → `apply` → полный контроль списком из §7 →
   затем Уровень 2 (Switch/Hekate).
9. ⬜ Косметика/необязательно: «0 B ГБ» в сводке игр; `fat32.num_sectors=0`
   в манифесте; `chown` файлов бэкапа (сейчас root:root).
10. ⬜ Полировка TUI после подтверждения CLI на реальном железе (см. §0.1).

## 8. Договорённости

- Все операции с блочными устройствами — только после явного подтверждения
  в TUI и прохождения Safety Guard.
- НИКОГДА не изменять целевой диск, если он совпадает с источником.
- Перед пересборкой: `cargo test`, `cargo clippy`. Для Nix: `nix flake check`.
- Комментарии/выводы — русские; идентификаторы — английские термины.

## 9. Как расширять

- Новый экран TUI: добавить вариант в `Screen`, роутинг в `handle_key`,
  рендер в `tui/ui.rs`.
- Новый шаг миграции: расширить `operations/migrate.rs` и `MigrateResult`.
- Поддержка GPT: добавить ветку в `partition.rs` (уже есть каркас `scheme`).
- Прогресс-бары: заменить `ProgressHandle::none()` на реальные индикаторы и
  прокинуть их статистику через канал `WorkerMsg::Info`.