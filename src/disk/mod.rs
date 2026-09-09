// Модуль работы с блочными устройствами: обнаружение, безопасность, партиции.
pub mod detect;
pub mod partition;

pub use detect::{partition_path, set_allow_loop, BlockDevice, DiskFilter};

/// Базовый размер сектора (байт). Весь расчёт смещений идёт от него.
pub const SECTOR_SIZE: u64 = 512;
