//! ジョイパッド（P1 / 0xFF00）。

/// ボタン。値は内部のビット位置（下位 4bit = 十字キー、上位 4bit = ボタン）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Button {
    Right = 0x01,
    Left = 0x02,
    Up = 0x04,
    Down = 0x08,
    A = 0x10,
    B = 0x20,
    Select = 0x40,
    Start = 0x80,
}

#[derive(Clone, Debug)]
pub struct Joypad {
    /// 押されているボタン（1 = 押下）
    pressed: u8,
    /// P1 の bit4・5（0 で選択）
    select: u8,
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}

impl Joypad {
    pub fn new() -> Self {
        Joypad {
            pressed: 0,
            // ブートROM終了時点では両方の列が選択された状態（P1 = 0xCF）
            select: 0x00,
        }
    }

    /// 入力線 4 本の状態（0 = 押されている）
    fn lines(&self) -> u8 {
        let mut lines = 0x0F;
        if self.select & 0x10 == 0 {
            lines &= !(self.pressed & 0x0F);
        }
        if self.select & 0x20 == 0 {
            lines &= !(self.pressed >> 4);
        }
        lines
    }

    pub fn read(&self) -> u8 {
        0xC0 | self.select | self.lines()
    }

    /// 入力線が 1→0 になったら true（ジョイパッド割り込み要求）
    pub fn write(&mut self, value: u8) -> bool {
        let before = self.lines();
        self.select = value & 0x30;
        before & !self.lines() != 0
    }

    /// ボタン状態を更新する。入力線が 1→0 になったら true（ジョイパッド割り込み要求）
    pub fn set_button(&mut self, button: Button, pressed: bool) -> bool {
        let before = self.lines();
        if pressed {
            self.pressed |= button as u8;
        } else {
            self.pressed &= !(button as u8);
        }
        before & !self.lines() != 0
    }

    /// 全ボタンの状態をまとめて設定する（ビット配置は [`Button`] と同じ）
    pub fn set_state(&mut self, pressed: u8) -> bool {
        let before = self.lines();
        self.pressed = pressed;
        before & !self.lines() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_row_shows_pressed_buttons_as_zero() {
        let mut pad = Joypad::new();
        pad.set_button(Button::Left, true);
        pad.set_button(Button::A, true);
        pad.write(0x20); // 十字キーの列を選択
        assert_eq!(pad.read(), 0xC0 | 0x20 | 0b1101);
        pad.write(0x10); // ボタンの列を選択
        assert_eq!(pad.read(), 0xC0 | 0x10 | 0b1110);
        pad.write(0x30); // どちらも選択しない
        assert_eq!(pad.read(), 0xFF);
        pad.write(0x00); // 両方選択すると重なって見える
        assert_eq!(pad.read(), 0xC0 | 0b1100);
    }

    #[test]
    fn press_requests_interrupt_only_when_its_row_is_selected() {
        let mut pad = Joypad::new();
        pad.write(0x20);
        assert!(!pad.set_button(Button::Start, true), "選択していない列");
        assert!(pad.set_button(Button::Down, true));
        assert!(
            !pad.set_button(Button::Down, true),
            "押しっぱなしでは出ない"
        );
        assert!(!pad.set_button(Button::Down, false), "離したときは出ない");
        // 列を切り替えた結果、線が 0 になった場合も割り込みが出る
        assert!(pad.write(0x10));
    }

    #[test]
    fn set_state_replaces_all_buttons() {
        let mut pad = Joypad::new();
        pad.write(0x10);
        assert!(pad.set_state(Button::A as u8 | Button::Start as u8));
        assert_eq!(pad.read() & 0x0F, 0b0110);
        pad.set_state(0);
        assert_eq!(pad.read() & 0x0F, 0x0F);
    }
}
