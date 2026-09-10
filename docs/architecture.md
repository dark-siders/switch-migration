# Архитектура switch-migrator

## 1. Общая схема

```
                 ┌───────────────────────────────┐
                 │         /dev/sdX или mmcblkX   │
                 │   (removable, SD-ридер)   │
                 └──────────────┬────────────────┘
                                │
   ┌────────────────────────────▼──────────────────────────┐
   │                    disk::detect                     │
   │  /sys/class/block → {name, size, removable, model}   │
   │  classify(): removable==1  → Removable               │
   │              иначе         → Forbidden               │
   └────────────────────────────┬──────────────────────────┘
                                │
   ┌────────────────────────────▼──────────────────────────┐
   │                   disk::partition                    │
   │  parse_mbr(): 4 записи MBR → Partition[]             │
   │  PartKind::{Fat32, Unsupported}                      │
   │  → EmuGeometry { fat_start, emu_start, emu_len, ... }│
   └────────────────────────────┬──────────────────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
┌───────────────┐      ┌────────────────┐      ┌────────────────┐
│  fat32::games │      │ emummc::ini    │      │ emummc::imaging │
│  сканер игр   │      │ сектор в .ini  │      │ дамп/восст.     │
│  Nintendo/    │      │ (клик 2 fix)   │      │ (zstd)          │
│  Contents/    │      └────────────────┘      └────────────────┘
│  registered   │
└───────────────┘
        │
        ▼
┌────────────────┐
│  manifest.rs   │  JSON: геометрия, игры, дата, id
│  manifest.json │
└────────────────┘
        │
        ▼
┌───────────────────────────────┐
│      operations::backup       │  Клик 1: бэкап
│      operations::migrate      │  Клик 2: миграция
└───────────────────────────────┘
        │
        ▼
┌───────────────────────────────┐
│           tui::app/ui         │  ratatui-интерфейс
│   экраны + worker-потоки      │
└───────────────────────────────┘
```

## 2. Модули и их ответственность

### 2.1 `disk::detect` — обнаружение и Safety Guard
- Читает `/sys/class/block/*`: `removable`, `size`, `device/model`.
- `is_partition_name`: отсекает `sda1`, `mmcblk0p1`, `nvme0n1p1`; НЕ путает
  `loop0`, `sdb`.
- `classify`: `removable==1` → `Removable` (разрешено). Дополнительно SD во
  встроенных кардридерах (`removable==0`, `device/type=="SD"`) разрешаются;
  внутренние eMMC (`device/type=="MMC"`) остаются `Forbidden`. В тест-режиме
  (`--allow-loop` / `SM_ALLOW_LOOP=1`) loop-устройства также считаются
  `Removable` — это единственный способ гонять цикл без реальной SD.
- `partition_path` строит путь раздела с учётом соглашений имён: `sda1`
  (цифра после имени) vs `mmcblk0p1` / `loop0p1` / `nvme0n1p1` (после имени
  с цифрой идёт `p`). Сначала точный поиск в `/sys/class/block`, иначе
  эвристика по последней цифре.
- `DiskFilter::validate_for_probe` — единая точка входа CLI и TUI.

### 2.2 `disk::partition` — разбор разделов
- `parse_mbr` читает сектор 0, проверяет сигнатуру `0x55AA`, парсит 4 записи
  (start, length, type_byte).
- `classify_type`: `0x0B/0x0C/0x1C/0x07` → `Fat32`, прочее → `Unsupported`.
- RAW-раздел EmuMMC определяется как «первый не-FAT32 раздел с start > 0»
  (стандарт карт Hekate: p1 FAT32, p2 RAW).
- `EmuGeometry` хранит итоговую геометрию для `emummc.ini` и образа.

### 2.3 `emummc::ini` — конфиг Hekate
- Читает `[emummc]`, извлекает `sector` (hex `0x…` или decimal).
- `set_sector_and_save` сохраняет исходный стиль записи (hex/decimal) и
  форматирование секции (комментарии, отступы).

### 2.4 `emummc::imaging` — образы
- `dump_emummc`: блоки 4 МБ → `zstd::write::Encoder(level 3)` → `.img.zst`.
- `restore_emummc`: декодирование → запись в раздел p2 новой карты (offset 0).
- `sync_disk`: fsync блочного устройства до безопасного извлечения.

### 2.4a `operations::backup` — окно дампа (`resolve_dump_window`)
- Окно RAW-дампа НЕ всегда «весь раздел с offset 0»: `resolve_dump_window`
  читает `emuMMC/emummc.ini` из скопированного `sdmc/` и, если `sector`
  лежит в границах p2, дампит от него до конца раздела (`data_start,
  end−data_start`); иначе — раздел целиком.
- **Причина:** Hekate резервирует заголовок/выравнивание внутри raw-раздела —
  на реальной карте данные EmuNAND начинаются на +32768 секторов (+16 МиБ)
  от старта p2. Слепой дамп «раздела» перенёс бы этот сдвиг на новую карту и
  рассогласовал исправленный `sector=` (баг пойман на реальной карте, журнал
  в `agent.md`, п.6).

### 2.5 `operations::migrate` — авто-фикс sector offset
1. `wipefs -a` — стереть метки.
2. `sfdisk` (stdin-скрипт): MBR, p1 FAT32 c boot-flag, p2 RAW (тип `0x27` —
   скрытый/не-FAT32, чтобы повторный inspect сам распознавал EmuMMC; реальная
   карта Hekate использует `0xe0` — применён 0x27 в реальном apply, финальное
   подтверждение за Уровнем 2). **Размер p2 адаптивный** — под образ EmuNAND
   (`manifest.emummc.size_bytes`) + 0.5 ГиБ запаса, округление до 1 МиБ;
   всё остальное забирает FAT32.
3. `partprobe`; `mkfs.vfat -F 32 -S 512 -s 64` (кластер 32 КБ).
4. `restore_emummc` в p2; mount p1 → копирование `sdmc/`.
5. `EmummcIni::set_sector_and_save(p2_start)` — **ключевой шаг** (данные после
   бэкапа окном из ini ложатся ровно на старт p2, значение согласовано).
6. `fix_archive_bit`; unmount; `sync_disk`.

Весь перенос (дамп/восстановление EmuMMC и копирование FAT32) отдаёт прогресс
в процентах через `utils::progress::Progress` (`ProgressSink`): stderr для CLI,
callback-замыкание для TUI (ratatui Gauge).

> ⚠️ **Fix Archive Bit на практике (2026-09-10).** Текущая реализация в
> `src/fat32/archive.rs` использует `FS_IOC_GETFLAGS/SETFLAGS` с
> `EXT2_FLAG_ARCHIVE=0x20` — эти ioctl для vfat не работают (для FAT нужны
> `FAT_IOCTL_GET_ATTRIBUTES/SET_ATTRIBUTES` из `linux/msdos_fs.h`), поэтому
> после apply биты на карте не выставлены. На реальном Switch (Level 2)
> пришлось разово прогнать hekate `fix_archive_bit`. Дополнительно: биты
> были кривые уже на исходной 128 ГиБ (до фикса в launch — надписи без
> картинок). Итог: **после любого apply на Switch нужен hekate-фикс**;
> код `archive.rs` решено не править (решение владельца). См. также
> «Переразметка» в `agent.md`.

> **Камень на десктопах:** демон автоподключения (`devmon`/udevil, `systemctl
> --user`) перемонтирует только что размеченный p1 и `mkfs.vfat` падает с
> «mounted filesystem». Перед `apply` останавливайте его
> (`systemctl --user stop devmon`), после — возвращайте. На случайном
> нарушении — `udisksctl unmount` смонтированной sdd1. (Зафиксировано на
> реальном apply 2026-09-10.)

## 3. Почему сектор фиксится, а не копируется «как есть»

Исходная карта: FAT32 занимает часть диска, RAW-раздел идёт сразу после и
начинается с сектора `S_old`. Новая карта больше → FAT32 индексируется
адаптивно (p2 — ровно под образ EmuNAND, FAT32 — остаток) → RAW начинается
с сектора `S_new ≠ S_old`. Копия разделов при `dd` перенесла бы
неверный `sector=`, и Hekate не нашёл бы EmuMMC. Поэтому инструмент:
- дампит **реальное окно данных** из `emummc.ini` (offset внутри
  p2 = `ini.sector − p2.start`), а не «весь раздел»;
- пишет образ в начало раздела p2 новой карты (offset 0);
- пересчитывает абсолютный стартовый сектор p2 и правит `emummc.ini`.

## 4. Модель данных (manifest.json)

```json
{
  "magic": "SM-MIGRATOR-1",
  "id": "<uuid>",
  "created_at": "<epoch>",
  "source_size_bytes": 660602880000,
  "emummc": {
    "start_sector": 311754752,
    "num_sectors": 966270976,
    "size_bytes": 494730738688
  },
  "fat32": {
    "start_sector": 2048,
    "num_sectors": 0,
    "size_bytes": 165872812800
  },
  "games": [
    { "title_id": "0100F2C0115B6000", "path": "…/registered/0100F2C0115B6000", "size_bytes": 4822001 }
  ],
  "game_count": 1,
  "image_zst_bytes": 123
}
```

`size_bytes` в `fat32` выводятся как `source_size - emu_size` (секторы считаются
утилитой при реальной миграции).

## 5. Безопасность

- Safety Guard на уровне обнаружения: несъёмные устройства (внутренние
  SATA/NVMe/eMMC) не видны в списке; SD в кардридере распознаётся по типу.
- Повторная проверка `is_safe()` перед разметкой в `run_migrate`.
- Отказ, если целевая карта == исходной (проверка в `App::start_migrate`).
- `manifest.validate()` перед разворачиванием.
- Несколько слоёв `fsync`/`sync_disk` перед сообщением «Готово».`