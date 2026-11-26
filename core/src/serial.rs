//! シリアル通信（SB / SC）。
//!
//! 通信相手はいない前提。内部クロックで転送を始めると 8 ビット分のクロック後に
//! 0xFF を受信して完了し、シリアル割り込みを要求する。
//! 送信しようとしたバイトは `output` に溜める（テストROMの文字出力に使う）。

#[derive(Clone, Debug, Default)]
pub struct Serial {
    sb: u8,
    sc: u8,
    bits_left: u8,
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Serial {
            sb: 0x00,
            sc: 0x7E,
            bits_left: 0,
            output: Vec::new(),
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF01 => self.sb,
            // DMG では bit1〜6 は未使用で 1 が読める
            _ => self.sc | 0x7E,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF01 => self.sb = value,
            _ => {
                self.sc = value & 0x81;
                if value & 0x81 == 0x81 {
                    // 内部クロックで転送開始
                    self.bits_left = 8;
                    self.output.push(self.sb);
                } else {
                    // 外部クロックは相手がいないので永久に進まない
                    self.bits_left = 0;
                }
            }
        }
    }

    /// シリアルクロック（8192 Hz）の立ち下がりごとに呼ぶ。転送完了で true（割り込み要求）
    pub fn clock(&mut self) -> bool {
        if self.bits_left == 0 {
            return false;
        }
        // 相手がいないので入力は常に 1
        self.sb = (self.sb << 1) | 1;
        self.bits_left -= 1;
        if self.bits_left == 0 {
            self.sc &= !0x80;
            return true;
        }
        false
    }

    /// これまでに送信されたバイト列を取り出す
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }
}
