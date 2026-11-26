//! APU（サウンド）。
//!
//! いまはレジスタの読み書きだけを持つ仮実装（音は出ない）。M7 で本実装に置き換える。

/// 各レジスタを読んだときに 1 として見えるビット（0xFF10〜0xFF2F）
const READ_MASKS: [u8; 0x20] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10〜NR14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // (未使用), NR21〜NR24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30〜NR34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // (未使用), NR41〜NR44
    0x00, 0x00, 0x70, // NR50〜NR52
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // 未使用
];

#[derive(Clone, Debug)]
pub struct Apu {
    regs: [u8; 0x20],
    wave_ram: [u8; 0x10],
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    /// DMG のブートROM終了直後の状態で作る
    pub fn new() -> Self {
        let mut regs = [0u8; 0x20];
        let initial: [(usize, u8); 21] = [
            (0x00, 0x80),
            (0x01, 0xBF),
            (0x02, 0xF3),
            (0x03, 0xFF),
            (0x04, 0xBF),
            (0x06, 0x3F),
            (0x07, 0x00),
            (0x08, 0xFF),
            (0x09, 0xBF),
            (0x0A, 0x7F),
            (0x0B, 0xFF),
            (0x0C, 0x9F),
            (0x0D, 0xFF),
            (0x0E, 0xBF),
            (0x10, 0xFF),
            (0x11, 0x00),
            (0x12, 0x00),
            (0x13, 0xBF),
            (0x14, 0x77),
            (0x15, 0xF3),
            (0x16, 0xF1),
        ];
        for (index, value) in initial {
            regs[index] = value;
        }
        Apu {
            regs,
            wave_ram: [0; 0x10],
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF10..=0xFF2F => {
                let index = (addr - 0xFF10) as usize;
                self.regs[index] | READ_MASKS[index]
            }
            0xFF30..=0xFF3F => self.wave_ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF10..=0xFF2F => self.regs[(addr - 0xFF10) as usize] = value,
            0xFF30..=0xFF3F => self.wave_ram[(addr - 0xFF30) as usize] = value,
            _ => {}
        }
    }
}
