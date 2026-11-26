//! 割り込み要求ビット（IF / IE レジスタのビット位置）。優先度はビットが小さいほど高い。

pub const VBLANK: u8 = 0x01;
pub const STAT: u8 = 0x02;
pub const TIMER: u8 = 0x04;
pub const SERIAL: u8 = 0x08;
pub const JOYPAD: u8 = 0x10;
