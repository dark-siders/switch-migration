# План миграции (пошаговый алгоритм)

Пошаговое описание того, что делает инструмент — клик 1 (бэкап) и клик 2
(миграция). Порядок шагов важен для безопасности данных.

## Клик 1. Бэкап старой карты

**Исходно:** в системе одна съёмная SD-карта (`/dev/sdb`), пользователь её выбрал.

1. **Обнаружение** — `DiskFilter::safe_devices()` возвращает `BlockDevice`.
   Признак разрешения: `/sys/class/block/sdb/removable == 1`; плюс SD-карты во
   встроенных кардридерах (`removable == 0`, но `device/type == "SD"` — не путать
   с внутренним `"MMC"`).

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

5. **Дамп EmuMMC** — `dump_emummc("/dev/sdb2", offset, len, out, progress)`:
   - окно дампа из `emuMMC/emummc.ini` (после копирования sdmc, см. п.6):
     `data_start = sector из ini`, `len = end_p2 - data_start`;
     offset внутри раздела = `data_start - p2.start`. Без ini — весь раздел
     (offset 0, len = p2.length). **Зачем:** Hekate резервирует заголовок/
     выравнивание — на реальной карте данные EmuNAND лежат на +16 МиБ от
     старта p2, и слепой дамп «всего раздела» сломал бы выравнивание после переноса;
   - блоки по 4 МБ пишем в `zstd` (level 3);
   - результат: `emummc.img.<data_start>.zst` (пустые секторы ≈ не занимают места).

6. **Копирование sdmc** — рекурсивно `sdmc/` (bootloader, Atmosphere, Nintendo,
   hakate configs) с mount'ом p1 RO. Выполняется ДО дампа (из копии читаем ini).

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

> ⚠️ На десктопах с демоном автоподключения (`devmon`/udevil) новая карта
> монтируется сама (`/run/media/…`) — сразу после разметки это ломает
> `mkfs.vfat` («mounted filesystem»). Перед миграцией:
> `systemctl --user stop devmon` и `udisksctl unmount -b /dev/sdc1` (если
> смонтирована); после готовности — `systemctl --user start devmon`.
> Зафиксировано при реальном apply на ПК (2026-09-10).

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
   - `P2_LEN` — **адаптивно**: ровно под образ EmuNAND из манифеста
     (`manifest.emummc.size_bytes`) + запас 0.5 ГиБ, округление до 1 МиБ;
- `P2_START = 2048 + P1`, где `P1 = TOTAL - P1_START - P2_LEN` — FAT32
      забирает всё остальное (на 512 ГиБ это 470 ГиБ вместо ~239);
   - `P2_START` — **новое значение `sector=` для `emummc.ini`**;
   - тип p2 — `0x27` (скрытый RAW), НЕ `0x0C`: иначе анализатор снова сочтёт
     p2 за FAT32 и не найдет EmuMMC при повторном анализе после миграции.

5. **`partprobe`** — ядро перечитывает таблицу.

6. **Форматирование FAT32**:
   `mkfs.vfat -F 32 -S 512 -s 64 -n SDCARD /dev/sdc1`
   (сектор 512, кластер 64×512 = 32 КБ — рекомендовано для Switch).

7. **Разворачивание образа EmuMMC**:
   `restore_emummc("emummc.img.*.zst", "/dev/sdc2", 0)` —
   пишем в начало раздела p2, offset 0 (образ уже начинается строго с данных
   EmuNAND → они ложатся ровно на старт p2, и `sector=` ниже согласован).

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
    > ⚠️ **Оговорка (2026-09-10):** текущий `archive.rs` юзает
    > `FS_IOC_GETFLAGS/SETFLAGS`, которые на vfat не работают (для FAT нужны
    > `FAT_IOCTL_*` из `linux/msdos_fs.h`) — на практике биты после apply НЕ
    > выставлены, и на реальном Switch (Level 2) потребовался разовый
    > `fix_archive_bit` в Hekate. Биты были кривые уже на исходной 128 ГиБ.
    > **После каждого apply на Switch — прогнать hekate `fix_archive_bit`.**
    > Код `archive.rs` решено не править (решение владельца).

11. **Финализация**:
    - `umount /mnt/sdc1`;
    - `sync_disk("/dev/sdc")` — fsync до извлечения;
    - вывод итого: `новый sector=<P2_START>`, `emummc.ini исправлен=true`,
      `archive bits: <N>`.

## Расчёт секторов (формулы — адаптивная разметка)

```
SECTOR = 512 байт
P1_START = 2048
RESERVE_BYTES = 0.5 ГиБ            # запас p2 поверх образа (для Hekate)
ALIGN_SECTORS = 2048               # кратность 1 МиБ

NEED      = ceil( manifest.emummc.size_bytes/512 + RESERVE_BYTES/512
                                                          , ALIGN_SECTORS )*ALIGN_SECTORS
P2_LEN    = min(NEED, TOTAL_SECTORS - P1_START)
P2_START  = P1_START + (TOTAL_SECTORS - P1_START - P2_LEN)
P1_LEN    = P2_START - P1_START

# В emummc.ini:
sector = P2_START            # десятичная или 0x… (сохраняем стиль исходного)
```

> ✅ **Адаптивная разметка «мин. EmuNAND» (реализовано, 2026-09-10):**
> p2 теперь занимает ровно столько, сколько нужно под образ EmuNAND
> (`manifest.emummc.size_bytes` ≈ 7.5 ГиБ) + запас 0.5 ГиБ, а не половину карты.
> На карте 512 ГиБ (реально 477.5 ГиБ) FAT32 получает 470 ГиБ вместо ~239,
> p2 = 7.51 ГиБ. Проверено на реальном Switch: DBI «SD 433/470».
> После apply на Switch — разовый `fix_archive_bit` в Hekate (см. шаг 10).
> Код `archive.rs` не правим.

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
| Раздел смонтирован демоном (`devmon`) | Остановить devmon + `udisksctl unmount` перед apply |
| Таблица уже смонтирована              | Требуется размонтирование перед операцией      |