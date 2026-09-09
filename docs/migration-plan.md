# План миграции (пошаговый алгоритм)

Пошаговое описание того, что делает инструмент — клик 1 (бэкап) и клик 2
(миграция). Порядок шагов важен для безопасности данных.

## Клик 1. Бэкап старой карты

**Исходно:** в системе одна съёмная SD-карта (`/dev/sdb`), пользователь её выбрал.

1. **Обнаружение** — `DiskFilter::safe_devices()` возвращает `BlockDevice`.
   Признак разрешения: `/sys/class/block/sdb/removable == 1`.

2. **Чтение MBR** — `disk::partition::parse_mbr("/dev/sdb")`:
   - сектор 0 → проверка `0x55AA`;
   - разбор 4 записей (start, length, type);
   - классификация: p1 → `Fat32`, p2 → `Unsupported` (это и есть EmuMMC).

3. **Геометрия** — строим `EmuGeometry`:
   - `fat_start = p1.start` (обычно 2048);
   - `emu_start = p2.start`; `emu_len = p2.length`; `emu_bytes = p2.size_bytes()`.

4. **Сканер игр**:
   - монтируем `/dev/sdb1` RO во временную папку;
   - `scan_registered` читает `Nintendo/Contents/registered/<16-hex>/`;
   - подсчёт `size_bytes` каждого каталога;
   - демонтируем.

5. **Дамп EmuMMC** — `dump_emummc("/dev/sdb2", 0, emu_len, out, progress)`:
   - открываем блок-устройство p2, читаем с offset 0 (это начало раздела);
   - блоки по 4 МБ пишем в `zstd` (level 3);
   - результат: `emummc.img.<emu_start>.zst` (пустые секторы ≈ не занимают места).

6. **Копирование sdmc** — рекурсивно `sdmc/` (bootloader, Atmosphere, Nintendo,
   hakate configs) с mount'ом p1 RO.

7. **Манифест** — `BackupManifest::new(...)`, сохраняем `manifest.json`.

**Итог бэкапа:**
```
~/switch-backup/
├── manifest.json
├── emummc.img.311754752.zst
└── sdmc/
    ├── bootloader/...
    ├── atmosphere/...
    ├── Nintendo/...
    └── emummc/emummc.ini
```

## Клик 2. Миграция на новую карту

**Исходно:** старая карта извлечена, вставлена новая `/dev/sdc` (больше объём),
доступен бэкап из клика 1.

1. **Проверки безопасности**:
   - `BlockDevice::is_safe()` для `/dev/sdc`;
   - `sdc != sdb` (пути не совпадают);
   - `manifest.validate()` — магия `SM-MIGRATOR-1`.

2. **Проверка утилит** — `sfdisk`, `wipefs`, `partprobe`, `mkfs.vfat`,
   `mount`, `umount`, `blockdev` присутствуют в PATH.

3. **Стирание разметки** — `wipefs --all /dev/sdc` (затирает MBR и метки ФС).

4. **Разметка MBR** через `sfdisk` (stdin-скрипт):
   ```
   label: dos
   device: /dev/sdc
   /dev/sdc1 : start=2048, size<P1>, type=c, bootable
   /dev/sdc2 : start=<P2>,  size<P2>, type=27
   ```
   где:
   - `P1 = (total - 2048)/2` — «данные» (аналогично исходной карте);
   - `P2 = total - P1_START - P1` — RAW EmuMMC;
   - `P2_START = 2048 + P1` — **новое значение `sector=` для `emummc.ini`**;
   - тип p2 — `0x27` (скрытый RAW), НЕ `0x0C`: иначе анализатор снова сочтёт
     p2 за FAT32 и не найдет EmuMMC при повторном анализе после миграции.

5. **`partprobe`** — ядро перечитывает таблицу.

6. **Форматирование FAT32**:
   `mkfs.vfat -F 32 -S 512 -s 64 -n SDCARD /dev/sdc1`
   (сектор 512, кластер 64×512 = 32 КБ — рекомендовано для Switch).

7. **Разворачивание образа EmuMMC**:
   `restore_emummc("emummc.img.*.zst", "/dev/sdc2", 0)` —
   пишем в начало раздела p2, offset 0 (внутренняя геометрия сохраняется 1:1).

8. **Копирование sdmc**: mount `/dev/sdc1` RW → копируем `sdmc/*`.

9. **Авто-фикс sector offset**:
   - `EmummcIni::load("/mnt/sdc1/emummc/emummc.ini")`;
   - `set_sector_and_save(P2_START)` — переписывает `sector=` в том же стиле
     (hex/decimal), как было;
   - при отсутствии файла — только предупреждение (EmuMMC файловая или
     поштучная — не трогаем).

10. **Fix Archive Bit**: `fix_archive_bit("/mnt/sdc1")` — рекурсивно выставляет
    attribute `0x20` (archive) всем каталогам FAT32 (требование Hekate/Atmosphere
    при чтении каталогов после Linux).

11. **Финализация**:
    - `umount /mnt/sdc1`;
    - `sync_disk("/dev/sdc")` — fsync до извлечения;
    - вывод итого: `новый sector=<P2_START>`, `emummc.ini исправлен=true`,
      `archive bits: <N>`.

## Расчёт секторов (формулы)

```
SECTOR = 512 байт

P1_START = 2048
P1_LEN   = (TOTAL_SECTORS - P1_START) / 2
P2_START = P1_START + P1_LEN
P2_LEN   = TOTAL_SECTORS - P2_START

# В emummc.ini:
sector = P2_START            # десятичная или 0x… (сохраняем стиль исходного)
```

## Диаграмма состояний

```
          Enter           Enter            1
Welcome ───────► Devices ───────► Inspect ────► Backup
                                      │               │
                                      │  3            │ done
                                      ▼               ▼
                                 MigrateTarget   BackupDone (пауза)
                                      │ Enter          │ 3
                                      ▼               ▼
                                   Migrate      MigrateTarget2 (цель)
                                      │
                                      ▼
                                  MigrateDone ────► Devices
```

(В текущем MVP экран «MigrateTarget» уже реализован; «Мигрировать на другую
карту» работает после создания бэкапа.)

## Edge cases

| Случай                                | Реакция                                        |
|---------------------------------------|------------------------------------------------|
| Карта без EmuMMC (только FAT32)       | Бэкап FAT32, раздел EmuMMC пропускается        |
| Нет файла `emummc.ini`                | Только предупреждение, миграция продолжается   |
| «Новая» карта совпадает со старой     | Отказ до разметки                              |
| `removable == 0`                      | Устройство не появляется в списке              |
| OOM при чтении гигантского зstd       | Копируем блоками, без аллокации всего образа   |
| Утile-таблица уже смонтирована        | Требуется размонтирование перед операцией      |