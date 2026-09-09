# switch-migrator

Утилита переноса данных Nintendo Switch на новую (более крупную) SD-карту
**в два клика**: бэкап → миграция. TUI на Rust + Nix flake.

> ⚠️ Работает только с removable-накопителями (SD/USB-ридеры). Системные диски
> (`/dev/sda`, `/dev/nvme0n1`) **категорически запрещены** фильтром безопасности.

---

## Возможности

- **Авто-определение SD-карт** по флагу `removable` из `/sys` + «Защита от дурака».
  В т.ч. SD-карты во **встроенных PCIe-ридерах** (`removable=0`, но тип
  `device/type = "SD"`): внутренние eMMC так НЕ распознаются.
- **Анализ карты**: таблица разделов, скрытый RAW-раздел EmuMMC, список игр
  (`Nintendo/Contents/registered`).
- **Дамп EmuMMC** в сжатый образ (`emummc.img.<sector>.zst`, zstd на лету) —
  пустые секторы не занимают места. Дамп берёт **реальное окно данных** из
  `emuMMC/emummc.ini` (Hekate резервирует заголовок/выравнивание внутри
  раздела — на реальных картах данные начинаются на +16 МиБ от старта p2),
  а не «весь раздел слепо».
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

- Работа **только** с накопителями, которые считаются съёмными:
  `/sys/class/block/<name>/removable == 1` (USB/SD), плюс SD-карты во встроенных
  кардридерах (`removable == 0`, но `device/type == "SD"`). Внутренние eMMC,
  SATA/NVMe и виртуальные диски помечаются `Forbidden` и отклоняются.
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
cargo test               # 15 юнит-тестов (detect, ini, dump-window, partition_path…)
cargo clippy             # без предупреждений
nix flake check --no-build --all-systems
```

### E2E на loop-дисках (без SD, но нужен root)

> **Статус реализации:**
> - **Level 0 (loop-диски): пройден.** E2E-цикл «бэкап → миграция → сверка»
>   зелёный (2 теста-бинаря + 15 юнитов, clippy 0).
> - **Level 1: пройден на РЕАЛЬНЫХ данных, включая реальный apply.**
>   Основная карта 128 ГиБ (встроенный кардридер) прошла `ls` → `probe` →
>   `backup`; бэкап развёрнут через `apply` на **физическую карту 512 ГиБ**
>   (2026-09-10). При бэкапе учтён баг «+16 МиБ»: данные EmuNAND начинаются
>   с сектора из `emummc.ini`, а не со старта раздела → дампится именно это окно.
>   Контроль после apply: `start_sector=229539840`,
>   `num_sectors=14710784`; данные в offset 0 p2, `sector→500696064`;
>   **sha256 распакованного образа == содержимого p2 физической карты**
>   (`48bb307d…d57ad2`) — побайтовое совпадение.
> - Рабочий бэкап-эталон: `~/switch-backup-fixed` (~39 ГиБ). Карта 512 ГиБ
>   готова: осталось вставить в Switch и проверить Hekate/EmuNAND (Level 2).
>
> ⚠️ При `apply` на десктопах с демоном автоподключения (`devmon`/udevil) карта
> перемонтируется сама и ломает `mkfs.vfat` — перед `apply` остановите его:
> `systemctl --user stop devmon` (после — `systemctl --user start devmon`).

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
3. Проверки бэкапа и новой карты до Switch:
   - `jq '.emummc' <backup>/manifest.json` — `start_sector` должен равняться
     `sector` из `emummc.ini` источника, `num_sectors` = остаток раздела
     (на реальной карте 128 ГиБ: 229539840 / 14710784, а не старт p2);
   - распаковать образ и убедиться, что первый сектор не нулевой
     (`zstd -dc emummc.img.*.zst | od -A x -t x1z -N64`);
   - `sfdisk -d /dev/sdX` — p1 FAT32 bootable (start 2048), p2 `type=27`;
   - `sudo fsck.fat -v /dev/sdX1` — целостность FAT32;
   - сверить `sha256sum` распакованного образа и `/dev/sdX2`
     (объём = `num_sectors` из манифеста);
   - `sudo mount -o ro /dev/sdX1 /mnt && cat /mnt/emummc/emummc.ini`
     (sector == старту p2) и `lsattr -d /mnt/Nintendo` (archive-бит).
4. Только после этого — вставить в Switch: загрузка Hekate → EmuMMC →
   игры запускаются, сохранения на месте, reboot-to-payload работает.

> ⚠️ Тип раздела p2 при миграции — `0x27` (скрытый RAW), а не `0x0C`: иначе
> инструмент после миграции сам не распознает p2 как EmuMMC. На реальной карте
> Hekate использует тип `0xe0` — вопрос «27 vs e0» требует финального
> подтверждения при реальном запуске на Switch (чтение raw-секторов от типа
> не зависит); реальный `apply` с `0x27` на физическую карту уже прошёл
> успешно и побайтово сверен.

## Лицензия

MIT.