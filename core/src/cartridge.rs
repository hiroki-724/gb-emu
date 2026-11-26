//! カートリッジと MBC（メモリバンクコントローラ）。
//!
//! 対応: ROM のみ / MBC1（マルチカート含む）/ MBC2 / MBC3（RTC 付き）/ MBC5

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MbcKind {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CartridgeError {
    /// ROM が小さすぎてヘッダが読めない
    TooSmall(usize),
    /// 未対応のカートリッジ種別（ヘッダ 0x0147 の値）
    UnsupportedType(u8),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CartridgeError::TooSmall(len) => {
                write!(f, "ROM が小さすぎます（{len} バイト）")
            }
            CartridgeError::UnsupportedType(t) => {
                write!(f, "未対応のカートリッジ種別です（0x{t:02X}）")
            }
        }
    }
}

impl std::error::Error for CartridgeError {}

/// MBC3 のリアルタイムクロック
#[derive(Clone, Debug, Default)]
struct Rtc {
    /// 動いている時計: 秒, 分, 時, 日(下位8bit), 日(上位1bit)+停止+桁あふれ
    regs: [u8; 5],
    /// ラッチされた値（CPU から読めるのはこちら）
    latched: [u8; 5],
    /// 1 秒未満の端数（M サイクル単位）
    subsecond: u32,
    /// ラッチ手順（0 → 1 の書き込み）の途中かどうか
    latch_armed: bool,
}

/// 1 秒あたりの M サイクル数（4194304 Hz / 4）
const M_CYCLES_PER_SECOND: u32 = 1_048_576;

impl Rtc {
    fn tick(&mut self) {
        if self.regs[4] & 0x40 != 0 {
            return; // 停止中
        }
        self.subsecond += 1;
        if self.subsecond >= M_CYCLES_PER_SECOND {
            self.subsecond = 0;
            self.advance_second();
        }
    }

    fn advance_second(&mut self) {
        // 範囲外の値を書き込まれた場合、実機と同じく「ちょうど 60 になったとき」だけ桁上がりする
        self.regs[0] = (self.regs[0] + 1) & 0x3F;
        if self.regs[0] != 60 {
            return;
        }
        self.regs[0] = 0;
        self.regs[1] = (self.regs[1] + 1) & 0x3F;
        if self.regs[1] != 60 {
            return;
        }
        self.regs[1] = 0;
        self.regs[2] = (self.regs[2] + 1) & 0x1F;
        if self.regs[2] != 24 {
            return;
        }
        self.regs[2] = 0;
        let (days, carry) = self.regs[3].overflowing_add(1);
        self.regs[3] = days;
        if carry {
            if self.regs[4] & 0x01 != 0 {
                // 512 日目であふれ: 日カウンタは 0 に戻り、桁あふれフラグが立つ
                self.regs[4] = (self.regs[4] & !0x01) | 0x80;
            } else {
                self.regs[4] |= 0x01;
            }
        }
    }

    fn read(&self, reg: u8) -> u8 {
        let mask = [0x3F, 0x3F, 0x1F, 0xFF, 0xC1][reg as usize];
        self.latched[reg as usize] & mask
    }

    fn write(&mut self, reg: u8, value: u8) {
        let mask = [0x3F, 0x3F, 0x1F, 0xFF, 0xC1][reg as usize];
        self.regs[reg as usize] = value & mask;
        if reg == 0 {
            // 秒を書くと 1 秒未満の端数もリセットされる
            self.subsecond = 0;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    kind: MbcKind,
    has_battery: bool,
    has_rtc: bool,
    /// MBC1 マルチカート（MBC1M）: BANK2 が 4bit ずれた位置につながっている
    multicart: bool,
    /// MBC5 の振動カート: RAM バンクの bit3 はモーター用で、バンク選択には使われない
    rumble: bool,

    ram_enabled: bool,
    /// ROM バンクレジスタ（MBC1 の BANK1 / MBC2・3 の ROMB / MBC5 の ROMB0+ROMB1）
    rom_bank: u16,
    /// MBC1 の BANK2 / MBC3・5 の RAM バンク（MBC3 では 0x08〜0x0C が RTC レジスタ選択）
    ram_bank: u8,
    /// MBC1 のモード選択
    mode: bool,
    rtc: Rtc,

    // レジスタから計算したオフセット（書き込み時に更新する）
    rom_offset_low: usize,
    rom_offset_high: usize,
    ram_offset: usize,
    /// セーブデータに未保存の変更があるか
    ram_dirty: bool,
}

impl Cartridge {
    pub fn new(mut rom: Vec<u8>) -> Result<Self, CartridgeError> {
        if rom.len() < 0x150 {
            return Err(CartridgeError::TooSmall(rom.len()));
        }
        let type_code = rom[0x147];
        let (kind, has_ram, has_battery, has_rtc, rumble) = match type_code {
            0x00 => (MbcKind::None, false, false, false, false),
            0x01 => (MbcKind::Mbc1, false, false, false, false),
            0x02 => (MbcKind::Mbc1, true, false, false, false),
            0x03 => (MbcKind::Mbc1, true, true, false, false),
            0x05 => (MbcKind::Mbc2, true, false, false, false),
            0x06 => (MbcKind::Mbc2, true, true, false, false),
            0x08 => (MbcKind::None, true, false, false, false),
            0x09 => (MbcKind::None, true, true, false, false),
            0x0F => (MbcKind::Mbc3, false, true, true, false),
            0x10 => (MbcKind::Mbc3, true, true, true, false),
            0x11 => (MbcKind::Mbc3, false, false, false, false),
            0x12 => (MbcKind::Mbc3, true, false, false, false),
            0x13 => (MbcKind::Mbc3, true, true, false, false),
            0x19 => (MbcKind::Mbc5, false, false, false, false),
            0x1A => (MbcKind::Mbc5, true, false, false, false),
            0x1B => (MbcKind::Mbc5, true, true, false, false),
            0x1C => (MbcKind::Mbc5, false, false, false, true),
            0x1D => (MbcKind::Mbc5, true, false, false, true),
            0x1E => (MbcKind::Mbc5, true, true, false, true),
            other => return Err(CartridgeError::UnsupportedType(other)),
        };

        // バンク計算を単純にするため、ROM は 32KB 以上の 2 のべき乗に切り上げる（足りない分は 0xFF）
        let rom_len = rom.len().max(0x8000).next_power_of_two();
        rom.resize(rom_len, 0xFF);

        let ram_len = if kind == MbcKind::Mbc2 {
            512 // MBC2 は 4bit × 512 の RAM を内蔵
        } else if has_ram {
            match rom[0x149] {
                0x01 => 0x800,
                0x02 => 0x2000,
                0x03 => 0x8000,
                0x04 => 0x2_0000,
                0x05 => 0x1_0000,
                _ => 0,
            }
        } else {
            0
        };

        // MBC1 マルチカートの判定: 1MB の ROM で、0x10 番バンク（= 2 本目のゲームの先頭）にも
        // 先頭と同じロゴデータが入っている。ヘッダだけでは通常の MBC1 と区別できないための経験則。
        // 空の領域どうしが偶然一致しただけの場合を除くため、ロゴらしい中身があることも確かめる
        let multicart = kind == MbcKind::Mbc1 && rom.len() == 0x10_0000 && {
            let logo = &rom[0x0104..0x0134];
            let mut seen = [false; 256];
            logo.iter().for_each(|&b| seen[b as usize] = true);
            let distinct = seen.iter().filter(|&&s| s).count();
            distinct >= 8 && logo == &rom[0x4_0104..0x4_0134]
        };

        let mut cart = Cartridge {
            rom,
            ram: vec![0xFF; ram_len],
            kind,
            has_battery,
            has_rtc,
            multicart,
            rumble,
            ram_enabled: false,
            rom_bank: 1,
            ram_bank: 0,
            mode: false,
            rtc: Rtc::default(),
            rom_offset_low: 0,
            rom_offset_high: 0x4000,
            ram_offset: 0,
            ram_dirty: false,
        };
        cart.update_offsets();
        Ok(cart)
    }

    pub fn kind(&self) -> MbcKind {
        self.kind
    }

    pub fn has_battery(&self) -> bool {
        self.has_battery
    }

    pub fn has_rtc(&self) -> bool {
        self.has_rtc
    }

    /// ヘッダに書かれたタイトル（ASCII として読める範囲）
    pub fn title(&self) -> String {
        self.rom[0x134..0x144]
            .iter()
            .take_while(|&&b| b != 0)
            .filter(|b| b.is_ascii_graphic() || **b == b' ')
            .map(|&b| b as char)
            .collect()
    }

    /// バッテリーバックアップ対象の RAM。セーブ機能がないカートリッジでは None
    pub fn save_ram(&self) -> Option<&[u8]> {
        if self.has_battery && !self.ram.is_empty() {
            Some(&self.ram)
        } else {
            None
        }
    }

    /// セーブデータを読み込む。長さが違う場合は入る分だけコピーする
    pub fn load_save_ram(&mut self, data: &[u8]) {
        let len = data.len().min(self.ram.len());
        self.ram[..len].copy_from_slice(&data[..len]);
        self.ram_dirty = false;
    }

    /// 前回の呼び出し以降に RAM が書き換えられたかを返し、フラグを下ろす
    pub fn take_ram_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ram_dirty)
    }

    /// 1 M サイクル進める（RTC 付きカートリッジのみ意味がある）
    #[inline]
    pub fn tick(&mut self) {
        if self.has_rtc {
            self.rtc.tick();
        }
    }

    /// 電源が切れていた間の経過時間ぶん RTC を進める（フロントエンド用）
    pub fn advance_rtc_seconds(&mut self, seconds: u64) {
        if !self.has_rtc || self.rtc.regs[4] & 0x40 != 0 {
            return;
        }
        // 512 日 = 44,236,800 秒で一周する。巨大な値でも回りすぎないよう上限を付ける
        for _ in 0..seconds.min(2 * 512 * 86_400) {
            self.rtc.advance_second();
        }
    }

    /// RTC の状態（セーブ用）: 動いている時計 5 バイト + ラッチ値 5 バイト
    pub fn rtc_state(&self) -> Option<[u8; 10]> {
        if !self.has_rtc {
            return None;
        }
        let mut out = [0u8; 10];
        out[..5].copy_from_slice(&self.rtc.regs);
        out[5..].copy_from_slice(&self.rtc.latched);
        Some(out)
    }

    pub fn load_rtc_state(&mut self, state: &[u8]) {
        if self.has_rtc && state.len() >= 10 {
            self.rtc.regs.copy_from_slice(&state[..5]);
            self.rtc.latched.copy_from_slice(&state[5..10]);
        }
    }

    fn update_offsets(&mut self) {
        let rom_mask = self.rom.len() - 1;
        let (low_bank, high_bank) = match self.kind {
            MbcKind::None => (0, 1),
            MbcKind::Mbc1 => {
                let bank2 = self.ram_bank as usize & 0x03;
                let bank1 = self.rom_bank as usize;
                let (upper, lower) = if self.multicart {
                    (bank2 << 4, bank1 & 0x0F)
                } else {
                    (bank2 << 5, bank1)
                };
                (if self.mode { upper } else { 0 }, upper | lower)
            }
            MbcKind::Mbc2 | MbcKind::Mbc3 | MbcKind::Mbc5 => (0, self.rom_bank as usize),
        };
        self.rom_offset_low = (low_bank * 0x4000) & rom_mask;
        self.rom_offset_high = (high_bank * 0x4000) & rom_mask;

        let ram_bank = match self.kind {
            MbcKind::None | MbcKind::Mbc2 => 0,
            MbcKind::Mbc1 => {
                if self.mode {
                    self.ram_bank as usize & 0x03
                } else {
                    0
                }
            }
            MbcKind::Mbc3 => self.ram_bank as usize & 0x07,
            MbcKind::Mbc5 => self.ram_bank as usize & if self.rumble { 0x07 } else { 0x0F },
        };
        self.ram_offset = if self.ram.is_empty() {
            0
        } else {
            (ram_bank * 0x2000) % self.ram.len()
        };
    }

    /// 0x0000〜0x7FFF の読み出し
    #[inline]
    pub fn read_rom(&self, addr: u16) -> u8 {
        if addr < 0x4000 {
            self.rom[self.rom_offset_low + addr as usize]
        } else {
            self.rom[self.rom_offset_high + (addr as usize & 0x3FFF)]
        }
    }

    /// 0x0000〜0x7FFF への書き込み（MBC のレジスタ）
    pub fn write_rom(&mut self, addr: u16, value: u8) {
        match self.kind {
            MbcKind::None => {}
            MbcKind::Mbc1 => match addr >> 13 {
                0 => self.ram_enabled = value & 0x0F == 0x0A,
                1 => {
                    // 5bit。0 を書くと 1 になる（この判定は ROM サイズでマスクする前の 5bit に対して行われる）
                    let bank = value as u16 & 0x1F;
                    self.rom_bank = if bank == 0 { 1 } else { bank };
                }
                2 => self.ram_bank = value & 0x03,
                _ => self.mode = value & 0x01 != 0,
            },
            MbcKind::Mbc2 => {
                if addr < 0x4000 {
                    // アドレスの bit8 でレジスタが決まる（0: RAM 許可、1: ROM バンク）
                    if addr & 0x0100 == 0 {
                        self.ram_enabled = value & 0x0F == 0x0A;
                    } else {
                        let bank = value as u16 & 0x0F;
                        self.rom_bank = if bank == 0 { 1 } else { bank };
                    }
                }
            }
            MbcKind::Mbc3 => match addr >> 13 {
                0 => self.ram_enabled = value & 0x0F == 0x0A,
                1 => {
                    let bank = value as u16 & 0x7F;
                    self.rom_bank = if bank == 0 { 1 } else { bank };
                }
                2 => self.ram_bank = value & 0x0F,
                _ => {
                    // 0 → 1 の順に書くと現在時刻がラッチされる
                    if value == 0x00 {
                        self.rtc.latch_armed = true;
                    } else {
                        if value == 0x01 && self.rtc.latch_armed {
                            self.rtc.latched = self.rtc.regs;
                        }
                        self.rtc.latch_armed = false;
                    }
                }
            },
            MbcKind::Mbc5 => match addr >> 12 {
                // Pan Docs では下位 4bit が 0xA なら有効とあるが、MBC5 の RAMG は 8bit 全体を
                // 比較する（Gekkio "Complete Technical Reference"）ので、0x0A ちょうどのみ有効にする
                0 | 1 => self.ram_enabled = value == 0x0A,
                2 => self.rom_bank = (self.rom_bank & 0x100) | value as u16,
                3 => self.rom_bank = (self.rom_bank & 0x0FF) | ((value as u16 & 0x01) << 8),
                4 | 5 => self.ram_bank = value & 0x0F,
                _ => {}
            },
        }
        self.update_offsets();
    }

    /// 0xA000〜0xBFFF の読み出し
    pub fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled {
            return 0xFF;
        }
        match self.kind {
            MbcKind::Mbc2 => 0xF0 | (self.ram[addr as usize & 0x1FF] & 0x0F),
            MbcKind::Mbc3 if self.ram_bank >= 0x08 => {
                if self.has_rtc && self.ram_bank <= 0x0C {
                    self.rtc.read(self.ram_bank - 0x08)
                } else {
                    0xFF
                }
            }
            _ => {
                if self.ram.is_empty() {
                    0xFF
                } else {
                    let index = (self.ram_offset + (addr as usize & 0x1FFF)) % self.ram.len();
                    self.ram[index]
                }
            }
        }
    }

    /// 0xA000〜0xBFFF への書き込み
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if !self.ram_enabled {
            return;
        }
        match self.kind {
            MbcKind::Mbc2 => {
                self.ram[addr as usize & 0x1FF] = value & 0x0F;
                self.ram_dirty = true;
            }
            MbcKind::Mbc3 if self.ram_bank >= 0x08 => {
                if self.has_rtc && self.ram_bank <= 0x0C {
                    self.rtc.write(self.ram_bank - 0x08, value);
                }
            }
            _ => {
                if !self.ram.is_empty() {
                    let index = (self.ram_offset + (addr as usize & 0x1FFF)) % self.ram.len();
                    self.ram[index] = value;
                    self.ram_dirty = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 各バンクの先頭にバンク番号を書いた ROM を作る
    fn make_rom(type_code: u8, banks: usize, ram_code: u8) -> Vec<u8> {
        let mut rom = vec![0u8; banks * 0x4000];
        for bank in 0..banks {
            rom[bank * 0x4000] = bank as u8;
            rom[bank * 0x4000 + 1] = (bank >> 8) as u8;
        }
        rom[0x147] = type_code;
        rom[0x149] = ram_code;
        rom
    }

    #[test]
    fn rejects_tiny_rom() {
        assert_eq!(
            Cartridge::new(vec![0; 16]).unwrap_err(),
            CartridgeError::TooSmall(16)
        );
    }

    #[test]
    fn mbc1_bank_zero_maps_to_one() {
        let mut cart = Cartridge::new(make_rom(0x01, 64, 0)).unwrap();
        cart.write_rom(0x2000, 0x00);
        assert_eq!(cart.read_rom(0x4000), 1);
        cart.write_rom(0x2000, 0x1F);
        assert_eq!(cart.read_rom(0x4000), 0x1F);
        cart.write_rom(0x4000, 0x01);
        assert_eq!(cart.read_rom(0x4000), 0x3F);
        // モード 1 では 0x0000〜 も BANK2 で切り替わる
        cart.write_rom(0x6000, 0x01);
        assert_eq!(cart.read_rom(0x0000), 0x20);
    }

    #[test]
    fn mbc1_ram_needs_enable() {
        let mut cart = Cartridge::new(make_rom(0x03, 4, 0x02)).unwrap();
        cart.write_ram(0xA000, 0x12);
        assert_eq!(cart.read_ram(0xA000), 0xFF);
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x12);
        assert_eq!(cart.read_ram(0xA000), 0x12);
        assert!(cart.take_ram_dirty());
        assert_eq!(cart.save_ram().unwrap()[0], 0x12);
    }

    #[test]
    fn mbc2_ram_is_4bit_and_mirrored() {
        let mut cart = Cartridge::new(make_rom(0x06, 4, 0)).unwrap();
        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0xFF);
        assert_eq!(cart.read_ram(0xA200), 0xFF);
        cart.write_ram(0xA001, 0x35);
        assert_eq!(cart.read_ram(0xA001), 0xF5);
        // bit8 が 1 のアドレスは ROM バンク選択
        cart.write_rom(0x0100, 0x03);
        assert_eq!(cart.read_rom(0x4000), 3);
    }

    #[test]
    fn mbc5_supports_bank_zero_and_9bit_bank() {
        let mut cart = Cartridge::new(make_rom(0x19, 512, 0)).unwrap();
        cart.write_rom(0x2000, 0x00);
        assert_eq!(cart.read_rom(0x4000), 0);
        cart.write_rom(0x2000, 0x34);
        cart.write_rom(0x3000, 0x01);
        assert_eq!(cart.read_rom(0x4000), 0x34);
        assert_eq!(cart.read_rom(0x4001), 0x01);
    }

    #[test]
    fn mbc3_rtc_counts_and_latches() {
        let mut cart = Cartridge::new(make_rom(0x10, 4, 0x02)).unwrap();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08); // 秒レジスタ
        for _ in 0..M_CYCLES_PER_SECOND * 3 {
            cart.tick();
        }
        assert_eq!(cart.read_ram(0xA000), 0, "ラッチするまで見えない");
        cart.write_rom(0x6000, 0x00);
        cart.write_rom(0x6000, 0x01);
        assert_eq!(cart.read_ram(0xA000), 3);
    }
}
