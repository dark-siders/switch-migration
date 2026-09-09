# switch-migrator

Утилита переноса данных Nintendo Switch на новую (более крупную) SD-карту
**в два клика**: бэкап → миграция. TUI на Rust + Nix flake.

> ⚠️ Работает только с removable-накопителями (SD/USB-ридеры). Системные диски
> (`/dev/sda`, `/dev/nvme0n1`) **категорически запрещены** фильтром безопасности.

---

## Возможности

- **Авто-определение SD-карт** по флагу `removable` из `/sys` + «Защита от дурака».
- **Анализ карты**: таблица разделов, скрытый RAW-раздел EmuMMC, список игр
  (`Nintendo/Contents/registered`).
- **Дамп EmuMMC** в сжатый образ (`emummc.img.<sector>.zst`, zstd на лету) —
  пустые секторы не занимают места.
- **Миграция**: разметка MBR (p1 FAT32 кластер 32К + p2 RAW), разворачивание
  образа, копирование sdmc, **авто-фикс `emummc.ini` (сектор)** и Fix Archive Bit.
- **JSON-манифест** каждого бэкапа (геометрия, игры, дата, ID).
- **Fix Archive Bit** — сброс архивных флагов для FAT32 (частая проблема после
  работы с картой в Linux).

## Быстрый старт (NixOS / Nix)

```bash
# Запуск TUI
nix run .#x86_64-linux

# Список безопасных накопителей (CLI)
nix run .#x86_64-linux -- ls

# Анализ конкретной карты
nix run .#x86_64-linux -- probe /dev/sdb

# Полный бэкап в папку
nix run .#x86_64-linux -- backup /dev/sdb --out ~/switch-backup

# Миграция из существующего бэкапа на новую карту
nix run .#x86_64-linux -- apply ~/switch-backup /dev/sdc
```

Отладка / разработка:

```bash
nix develop          # cargo build, cargo clippy, cargo test
cargo build --release
./target/release/switch-migrator
```

Без Nix:

```bash
cargo build --release
# Системные утилиты должны быть в PATH:
#   sfdisk wipefs partprobe mkfs.vfat mount umount blockdev
sudo ./target/release/switch-migrator
```

## UX «в 2 клика»

### Клик 1 — Бэкап
1. Вставьте **старую** SD-карту (кардридер или USB-ридер).
2. TUI покажет её в списке → **Enter**.
3. Инструмент сам находит FAT32 и скрытый RAW-раздел EmuMMC, показывает игры.
4. **`1`** → создаётся слепок: `manifest.json` + `emummc.img.*.zst` + `sdmc/`.

### Клик 2 — Миграция
1. Выньте старую карту, вставьте **новую** (свободнее по объёму).
2. **`3`** → выберите новую карту в списке → **Enter**.
3. Инструмент:
   - затирает старую разметку и создаёт MBR (p1 FAT32, p2 RAW);
   - форматирует FAT32 с кластером 32 КБ;
   - разворачивает образ EmuMMC;
   - копирует файлы sdmc;
   - **автоматически пересчитывает** стартовый сектор RAW-раздела в
     `sdmc:/emummc/emummc.ini` (главное отличие от ручного копирования `dd`);
   - сбрасывает archive bits.
4. Карта готова — вставляйте в Switch.

## Архитектура

```
src/
├── main.rs              # точка входа, CLI-режимы, TUI-loop
├── disk/
│   ├── detect.rs        # обнаружение блок-устройств + Safety Guard
│   └── partition.rs     # парсинг MBR, EmuGeometry, расчёт секторов
├── emummc/
│   ├── ini.rs           # emummc.ini: чтение/запись/сдвиг сектора
│   └── imaging.rs       # дамп (zstd) и восстановление RAW-образа
├── fat32/
│   ├── games.rs         # сканер игр Nintendo
│   └── archive.rs       # Fix Archive Bit (ioctl)
├── manifest.rs          # JSON-манифест бэкапа
├── operations/
│   ├── inspect.rs       # анализ карты
│   ├── backup.rs        # операция «Бэкап»
│   └── migrate.rs       # операция «Миграция» + sector-offset fix
├── tui/
│   ├── app.rs           # состояние и потоки
│   └── ui.rs            # рендер
└── utils/
    ├── commands.rs      # обёртки над CLI-утилитами
    └── human.rs         # человекочитаемые размеры
```

Подробнее об алгоритмах см. **docs/architecture.md** и **docs/migration-plan.md**.

## Safety Guard

- Работа **только** с накопителями, у которых `/sys/class/block/<name>/removable`
  равен `1` (USB/SD). Всё остальное — системные диски, `loop`, виртуальные —
  помечается `Forbidden` и отклоняется.
- Целевая карта не должна совпадать с исходной.
- Разметка начинается только после явного нажатия в TUI.
- Для записи в блочные устройства нужен root — запускайте через `sudo`.

## Зависимости (Nix-ready)

Крайний набор утилит собран в `runtimeTools` в `flake.nix` и доступен через PATH
внутри `nix run` / `nix develop`:
`sfdisk`, `wipefs`, `partprobe`, `partx`, `parted`, `mkfs.vfat`, `mount`, `umount`,
`blockdev`, `dd`, `sync`, `zstd`, `which`. Никаких зашитых путей вроде `/usr/bin/`.

## Тестирование

### Юнит-тесты и статика (без реальных дисков)

```bash
cargo test               # 11+ юнит-тестов (детект, ini, partition_path и т.д.)
cargo clippy             # без предупреждений
nix flake check --no-build --all-systems
```

### E2E на loop-дисках (без SD, но нужен root)

> **Статус: пройден** (Level 0). Дальше — Level 1 на реальных запасных картах
> (см. ниже), затем реальный Switch. На момент последней проверки карты
> большей ёмкости у автора не было — тесты на реальном железе открыты.

Полный цикл «бэкап → миграция → сверка байтов» проганяется на виртуальных
картах-файлах (`losetup`): создаётся «старая» карта (p1 FAT16 + p2 RAW с
псевдо-содержимым), затем `apply` на «новую», после чего сверяются таблица
разделов, RAW-секция побайтово, исправление `emummc.ini`, перенос `Nintendo/`,
повторная самодетекция EmuMMC.

```bash
sudo cargo test --test e2e_loop   # без root — тихо пропускается
```

Запуск без записи в `target` под root: собрать `cargo test --test e2e_loop --no-run`,
затем `sudo TMPDIR=/var/tmp/sm-e2e nix develop -c target/debug/deps/e2e_loop-<hash> --nocapture`
(образы ~8.5 ГиБ в tmpfs не влезают; после прогона — почистить
`losetup -d` и `/var/tmp/sm-e2e`, подробности в `agent.md` §0.2).

### Тест-режим loop-устройств

Для отладки на образах-файлах (без SD) разрешается работа с loop-устройствами:

```bash
truncate -s 512M old.img
losetup -f -P --show old.img                    # → /dev/loopN
SM_ALLOW_LOOP=1 nix run . -- --allow-loop probe /dev/loopN
```

Флаг `--allow-loop` или переменная `SM_ALLOW_LOOP=1` снимает Safety Guard только
для loop-устройств. **В обычном режиме loop-устройства по-прежнему запрещены.**

### Перед боевым использованием (реальные карты)

1. Read-only: `ls` и `probe` на запасной карте.
2. `backup` (только чтение) и `apply` на **заведомо ненужную** вторую карту.
3. Проверки новой карты до Switch:
   - `sfdisk -d /dev/sdX` — p1 FAT32 bootable (start 2048), p2 `type=27`;
   - `sudo fsck.fat -v /dev/sdX1` — целостность FAT32;
   - сверить `sha256sum` распакованного образа и `/dev/sdX2`);
   - `sudo mount -o ro /dev/sdX1 /mnt && cat /mnt/emummc/emummc.ini`
     (sector == старту p2) и `lsattr -d /mnt/Nintendo` (archive-бит).
4. Только после этого — вставить в Switch: загрузка Hekate → EmuMMC →
   игры запускаются, сохранения на месте, reboot-to-payload работает.

> ⚠️ Тип раздела p2 при миграции — `0x27` (скрытый RAW), а не `0x0C`: иначе
> инструмент после миграции сам не распознает p2 как EmuMMC. Корректность нужно
> сверить на реальной карте (раздел читается по raw-секторам, тип влияет только
> на отображение в ОС).

## Лицензия

MIT.