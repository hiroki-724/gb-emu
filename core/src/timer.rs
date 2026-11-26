//! タイマー（DIV / TIMA / TMA / TAC）。
//!
//! 実機では 16bit の内部カウンタが T サイクルごとに進み、DIV はその上位 8bit。
//! このエミュレータは CPU の M サイクルに同期して動くので、内部カウンタを
//! M サイクル単位の 14bit で持つ（下位 2bit は常に CPU と揃っているため情報を失わない）。
//! TIMA は「内部カウンタの特定ビット AND タイマー許可」の立ち下がりで +1 される。
//! この立ち下がり検出のおかげで、DIV や TAC への書き込みによる余分なカウントも再現できる。

#[derive(Clone, Debug)]
pub struct Timer {
    /// 内部カウンタ（M サイクル単位、14bit）
    counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// TIMA があふれた直後。次のサイクルで TMA をリロードし、割り込みを要求する
    overflow: bool,
    /// このサイクルでリロードが起きた
    reloading: bool,
}

const COUNTER_MASK: u16 = 0x3FFF;

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Timer {
            // DMG では 0x0100 の最初の命令をフェッチするサイクルで内部カウンタが 0xABCC
            // （T サイクル単位）になる。ここはそのサイクルに入る前の値なので 1 つ手前
            counter: (0xABCC >> 2) - 1,
            tima: 0,
            tma: 0,
            tac: 0,
            overflow: false,
            reloading: false,
        }
    }

    /// 内部カウンタ（M サイクル単位）。シリアルや APU がここからクロックを取る
    #[inline]
    pub fn counter(&self) -> u16 {
        self.counter
    }

    /// TAC で選ばれた内部カウンタのビット（M サイクル単位に換算済み）
    #[inline]
    fn selected_bit(&self) -> u16 {
        match self.tac & 0x03 {
            0 => 1 << 7, // 4096 Hz
            1 => 1 << 1, // 262144 Hz
            2 => 1 << 3, // 65536 Hz
            _ => 1 << 5, // 16384 Hz
        }
    }

    /// TIMA のクロック入力（選択ビット AND 許可ビット）
    #[inline]
    fn input(&self) -> bool {
        self.tac & 0x04 != 0 && self.counter & self.selected_bit() != 0
    }

    fn increment(&mut self) {
        let (v, overflow) = self.tima.overflowing_add(1);
        self.tima = v;
        self.overflow = overflow;
    }

    /// 1 M サイクル進める。タイマー割り込みを要求するとき true を返す
    pub fn tick(&mut self) -> bool {
        self.reloading = false;
        if self.overflow {
            // あふれた次のサイクル: TIMA は 1 サイクルだけ 0 に見えた後、TMA がロードされる
            self.overflow = false;
            self.reloading = true;
            self.tima = self.tma;
            self.counter = (self.counter + 1) & COUNTER_MASK;
            return true;
        }
        let before = self.input();
        self.counter = (self.counter + 1) & COUNTER_MASK;
        if before && !self.input() {
            self.increment();
        }
        false
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.counter >> 6) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            _ => self.tac | 0xF8,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF04 => {
                // 何を書いても内部カウンタ全体が 0 になる。選択ビットが 1 だった場合は
                // 立ち下がりになるので TIMA が進む
                if self.input() {
                    self.increment();
                }
                self.counter = 0;
            }
            0xFF05 => {
                // リロードが起きたサイクルの書き込みは無視される。
                // あふれた直後（リロード待ち）のサイクルに書くと、リロードと割り込みが取り消される
                if !self.reloading {
                    self.overflow = false;
                    self.tima = value;
                }
            }
            0xFF06 => {
                self.tma = value;
                // リロードと同じサイクルに書くと、新しい値がそのまま TIMA にも入る
                if self.reloading {
                    self.tima = value;
                }
            }
            _ => {
                let before = self.input();
                self.tac = value & 0x07;
                if before && !self.input() {
                    self.increment();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Timer {
        let mut t = Timer::new();
        t.write(0xFF04, 0);
        t
    }

    #[test]
    fn div_increments_every_64_m_cycles() {
        let mut t = fresh();
        for _ in 0..63 {
            t.tick();
        }
        assert_eq!(t.read(0xFF04), 0);
        t.tick();
        assert_eq!(t.read(0xFF04), 1);
    }

    #[test]
    fn tima_fastest_rate_counts_every_4_m_cycles() {
        let mut t = fresh();
        t.write(0xFF07, 0x05);
        for _ in 0..4 {
            t.tick();
        }
        assert_eq!(t.read(0xFF05), 1);
    }

    #[test]
    fn overflow_reloads_tma_one_cycle_later_and_requests_interrupt() {
        let mut t = fresh();
        t.write(0xFF06, 0xAB);
        t.write(0xFF05, 0xFF);
        t.write(0xFF07, 0x05);
        let mut irq = false;
        for _ in 0..4 {
            irq |= t.tick();
        }
        assert_eq!(t.read(0xFF05), 0x00, "あふれた直後の 1 サイクルは 0");
        assert!(!irq);
        assert!(t.tick());
        assert_eq!(t.read(0xFF05), 0xAB);
    }

    #[test]
    fn writing_div_can_increment_tima() {
        let mut t = fresh();
        t.write(0xFF07, 0x05);
        t.tick();
        t.tick(); // 選択ビット（bit1）が 1 になった
        t.write(0xFF04, 0);
        assert_eq!(t.read(0xFF05), 1);
    }
}
