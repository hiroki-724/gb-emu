//! APU（サウンド）。
//!
//! 4 チャンネル（矩形波 ×2、波形メモリ、ノイズ）とミキサー。
//! 長さカウンタ・エンベロープ・スイープは 512Hz の「フレームシーケンサ」で進み、
//! その時計はタイマーの内部カウンタ（DIV）から取っている。
//!
//! 音はステレオの f32 サンプル列として取り出す（[`Apu::take_samples`]）。
//! 出力サンプルレートを設定するまでは音声の生成を行わない（テストや画面なし実行を軽くするため）。
//!
//! 細かい挙動は gbdev wiki の "Gameboy sound hardware" と、Blargg の dmg_sound テストに合わせている。

/// 各レジスタを読んだときに 1 として見えるビット（0xFF10〜0xFF26）
const READ_MASKS: [u8; 0x17] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10〜NR14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // (未使用), NR21〜NR24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30〜NR34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // (未使用), NR41〜NR44
    0x00, 0x00, 0x70, // NR50〜NR52
];

/// 矩形波のデューティ比ごとの波形（左のビットから順に出力する）
const DUTY_PATTERNS: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

/// ノイズの分周比（M サイクル単位）。実機の T サイクル単位の値 8,16,32,… を 4 で割ったもの
const NOISE_DIVISORS: [u32; 8] = [2, 4, 8, 12, 16, 20, 24, 28];

/// 1 秒あたりの M サイクル数
const M_CYCLES_PER_SECOND: f64 = 1_048_576.0;

/// 音量エンベロープ（NRx2）
#[derive(Clone, Debug, Default)]
struct Envelope {
    initial: u8,
    increase: bool,
    period: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    fn write(&mut self, value: u8) {
        self.initial = value >> 4;
        self.increase = value & 0x08 != 0;
        self.period = value & 0x07;
    }

    /// 上位 5bit がすべて 0 だと DAC が切れ、チャンネルも止まる
    fn dac_enabled(&self) -> bool {
        self.initial != 0 || self.increase
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        // 周期 0 は 8 として扱われる
        self.timer = if self.period == 0 { 8 } else { self.period };
    }

    /// 64Hz の時計。音量が変わったら true
    fn clock(&mut self) -> bool {
        if self.period == 0 {
            return false;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer != 0 {
            return false;
        }
        self.timer = self.period;
        if self.increase && self.volume < 15 {
            self.volume += 1;
            true
        } else if !self.increase && self.volume > 0 {
            self.volume -= 1;
            true
        } else {
            false
        }
    }
}

/// 長さカウンタ（NRx1 と NRx4 bit6）
#[derive(Clone, Debug, Default)]
struct Length {
    counter: u16,
    enabled: bool,
}

impl Length {
    /// 256Hz の時計。カウンタが 0 になってチャンネルを止めるべきとき true
    fn clock(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            return self.counter == 0;
        }
        false
    }

    /// NRx4 への書き込み（長さ許可ビットとトリガー）の共通処理。
    /// チャンネルを止めるべきとき true を返す。
    ///
    /// `next_step_skips_length`: フレームシーケンサの次のステップが長さを進めないステップか。
    /// そのタイミングで長さを許可すると、カウンタが余分に 1 回進む（実機の挙動）
    fn write_control(&mut self, value: u8, max: u16, next_step_skips_length: bool) -> bool {
        let was_enabled = self.enabled;
        self.enabled = value & 0x40 != 0;
        let trigger = value & 0x80 != 0;
        let mut disable = false;

        if next_step_skips_length && !was_enabled && self.enabled && self.counter > 0 {
            self.counter -= 1;
            if self.counter == 0 && !trigger {
                disable = true;
            }
        }
        if trigger && self.counter == 0 {
            self.counter = max;
            if next_step_skips_length && self.enabled {
                self.counter -= 1;
            }
        }
        disable
    }
}

/// 矩形波チャンネル（チャンネル 1・2）。スイープはチャンネル 1 だけが使う
#[derive(Clone, Debug, Default)]
struct Square {
    enabled: bool,
    duty: u8,
    duty_position: u8,
    frequency: u16,
    /// 次に波形を 1 段進めるまでの M サイクル数
    timer: u16,
    length: Length,
    envelope: Envelope,

    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    /// スイープ計算用に保持している周波数
    shadow_frequency: u16,
    /// 直近のトリガー以降、減算モードで計算したことがあるか
    negate_used: bool,
}

impl Square {
    #[inline]
    fn period(&self) -> u16 {
        // 実機では (2048 - 周波数) × 4 T サイクル = (2048 - 周波数) M サイクル
        2048 - self.frequency
    }

    /// 1 M サイクル進める。出力が変わりうるとき true
    #[inline]
    fn tick(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        self.timer -= 1;
        if self.timer == 0 {
            self.timer = self.period();
            self.duty_position = (self.duty_position + 1) & 7;
            return true;
        }
        false
    }

    fn output(&self) -> u8 {
        if self.enabled && (DUTY_PATTERNS[self.duty as usize] >> (7 - self.duty_position)) & 1 != 0
        {
            self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self, with_sweep: bool) {
        self.enabled = true;
        self.timer = self.period();
        self.envelope.trigger();
        if with_sweep {
            self.shadow_frequency = self.frequency;
            self.sweep_timer = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
            self.negate_used = false;
            // シフトが 0 でなければ、トリガーの時点ですぐに計算とあふれ判定が行われる
            if self.sweep_shift != 0 {
                self.sweep_calculate();
            }
        }
        if !self.envelope.dac_enabled() {
            self.enabled = false;
        }
    }

    /// スイープ後の周波数を計算する。2047 を超えたらチャンネルを止める
    fn sweep_calculate(&mut self) -> u16 {
        let delta = self.shadow_frequency >> self.sweep_shift;
        let new_frequency = if self.sweep_negate {
            self.negate_used = true;
            self.shadow_frequency - delta
        } else {
            self.shadow_frequency + delta
        };
        if new_frequency > 2047 {
            self.enabled = false;
        }
        new_frequency
    }

    /// 128Hz の時計
    fn sweep_clock(&mut self) {
        self.sweep_timer = self.sweep_timer.saturating_sub(1);
        if self.sweep_timer != 0 {
            return;
        }
        // 周期 0 は 8 として扱われる
        self.sweep_timer = if self.sweep_period == 0 {
            8
        } else {
            self.sweep_period
        };
        if self.sweep_enabled && self.sweep_period != 0 {
            let new_frequency = self.sweep_calculate();
            if new_frequency <= 2047 && self.sweep_shift != 0 {
                self.shadow_frequency = new_frequency;
                self.frequency = new_frequency;
                // 書き戻した値でもう一度あふれ判定だけ行う
                self.sweep_calculate();
            }
        }
    }
}

/// 波形メモリチャンネル（チャンネル 3）
#[derive(Clone, Debug, Default)]
struct Wave {
    enabled: bool,
    dac_enabled: bool,
    /// 音量コード（0 = 無音、1 = 100%、2 = 50%、3 = 25%）
    volume_code: u8,
    frequency: u16,
    length: Length,
    ram: [u8; 16],
    /// 再生位置（0〜31、4bit サンプル単位）
    position: u8,
    /// いま鳴らしているバイト
    sample_byte: u8,
    /// 次のサンプルに進むまでの残り（2 T サイクル単位、0 なら次の 1 単位で進む）
    countdown: u16,
    /// 直前の CPU アクセスの直前に、ちょうどサンプルを読んだところか。
    /// DMG では、再生中の波形 RAM はこの瞬間にしか読み書きできない
    just_read: bool,
}

impl Wave {
    /// 1 M サイクル（= 2 T サイクル × 2）進める。出力が変わりうるとき true
    #[inline]
    fn tick(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        let mut cycles_left = 2u16;
        let mut advanced = false;
        while cycles_left > self.countdown {
            cycles_left -= self.countdown + 1;
            self.countdown = self.frequency ^ 0x7FF;
            self.position = (self.position + 1) & 31;
            self.sample_byte = self.ram[(self.position >> 1) as usize];
            self.just_read = true;
            advanced = true;
        }
        if cycles_left > 0 {
            self.countdown -= cycles_left;
            self.just_read = false;
        }
        advanced
    }

    fn output(&self) -> u8 {
        if !self.enabled || self.volume_code == 0 {
            return 0;
        }
        let nibble = if self.position & 1 == 0 {
            self.sample_byte >> 4
        } else {
            self.sample_byte & 0x0F
        };
        nibble >> (self.volume_code - 1)
    }

    fn read_ram(&self, index: usize) -> u8 {
        if !self.enabled {
            self.ram[index]
        } else if self.just_read {
            // 再生中は、チャンネルがいま読んでいるバイトが見える
            self.ram[(self.position >> 1) as usize]
        } else {
            0xFF
        }
    }

    fn write_ram(&mut self, index: usize, value: u8) {
        if !self.enabled {
            self.ram[index] = value;
        } else if self.just_read {
            self.ram[(self.position >> 1) as usize] = value;
        }
    }

    fn trigger(&mut self) {
        // DMG の不具合: チャンネルがサンプルを読む直前に再トリガーすると、波形 RAM の先頭が壊れる。
        // 読もうとしていたのが先頭 4 バイトならそのバイトが先頭 1 バイトに、
        // それ以外なら読もうとしていた 4 バイトの塊が先頭 4 バイトに上書きされる
        if self.enabled && self.countdown == 0 {
            let offset = (((self.position + 1) >> 1) & 0x0F) as usize;
            if offset < 4 {
                self.ram[0] = self.ram[offset];
            } else {
                let block = offset & !3;
                self.ram.copy_within(block..block + 4, 0);
            }
        }
        self.position = 0;
        if self.dac_enabled {
            self.enabled = true;
            // 再生開始は少し遅れる。最初のサンプルは読み直されず、直前のバッファの値が鳴る
            self.countdown = (self.frequency ^ 0x7FF) + 3;
        } else {
            self.enabled = false;
        }
    }
}

/// ノイズチャンネル（チャンネル 4）
#[derive(Clone, Debug, Default)]
struct Noise {
    enabled: bool,
    length: Length,
    envelope: Envelope,
    shift: u8,
    /// true なら 7bit 周期（金属的な音）
    short_mode: bool,
    divisor_code: u8,
    lfsr: u16,
    timer: u32,
}

impl Noise {
    #[inline]
    fn period(&self) -> u32 {
        NOISE_DIVISORS[self.divisor_code as usize] << self.shift
    }

    #[inline]
    fn tick(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        self.timer -= 1;
        if self.timer != 0 {
            return false;
        }
        self.timer = self.period();
        // シフト量 14・15 では LFSR に時計が入らない
        if self.shift >= 14 {
            return false;
        }
        let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
        self.lfsr = (self.lfsr >> 1) | (bit << 14);
        if self.short_mode {
            self.lfsr = (self.lfsr & !0x40) | (bit << 6);
        }
        true
    }

    fn output(&self) -> u8 {
        if self.enabled && self.lfsr & 1 == 0 {
            self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self) {
        self.enabled = true;
        self.timer = self.period();
        self.envelope.trigger();
        self.lfsr = 0x7FFF;
        if !self.envelope.dac_enabled() {
            self.enabled = false;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Apu {
    /// NR52 bit7（全体の電源）
    powered: bool,
    /// 書き込まれたままのレジスタ値（0xFF10〜0xFF26）。読み出し用
    regs: [u8; 0x17],
    /// フレームシーケンサが次に実行するステップ（0〜7）
    frame_step: u8,
    ch1: Square,
    ch2: Square,
    ch3: Wave,
    ch4: Noise,

    // ---- 音声出力 ----
    /// 出力サンプルレート（Hz）。0 の間は音声を生成しない
    sample_rate: f64,
    /// 次の出力サンプルまでの位相（M サイクル単位で 1 秒 = 1,048,576）
    sample_phase: f64,
    /// 現在のミキサー出力（左, 右）。チャンネルの状態が変わったときだけ計算し直す
    mix: (f32, f32),
    mix_dirty: bool,
    /// 出力サンプル 1 個ぶんの期間の平均を取るための累積
    sum: (f32, f32),
    sum_count: u32,
    /// 直流成分を取り除くハイパスフィルタ（実機のコンデンサに相当）の状態
    capacitor: (f32, f32),
    capacitor_charge: f32,
    samples: Vec<f32>,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    /// DMG のブートROM終了直後の状態で作る
    pub fn new() -> Self {
        let mut apu = Apu {
            powered: true,
            regs: [0; 0x17],
            frame_step: 0,
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            sample_rate: 0.0,
            sample_phase: 0.0,
            mix: (0.0, 0.0),
            mix_dirty: true,
            sum: (0.0, 0.0),
            sum_count: 0,
            capacitor: (0.0, 0.0),
            capacitor_charge: 0.0,
            samples: Vec::new(),
        };
        // ブートROMが起動音を鳴らした後の状態。チャンネル 1 は「鳴っている」扱いのまま
        // 音量が 0 まで下がっている
        let initial: [(usize, u8); 9] = [
            (0x00, 0x80), // NR10
            (0x01, 0xBF), // NR11
            (0x02, 0xF3), // NR12
            (0x03, 0xC1), // NR13
            (0x04, 0x87), // NR14
            (0x0A, 0x7F), // NR30
            (0x14, 0x77), // NR50
            (0x15, 0xF3), // NR51
            (0x16, 0xF1), // NR52
        ];
        for (index, value) in initial {
            apu.regs[index] = value;
        }
        apu.ch1.duty = 2;
        apu.ch1.frequency = 0x7C1;
        apu.ch1.timer = apu.ch1.period();
        apu.ch1.envelope.write(0xF3);
        apu.ch1.enabled = true;
        apu.ch4.lfsr = 0x7FFF;
        apu
    }

    // ---- フレームシーケンサ ----

    /// フレームシーケンサの次のステップが、長さカウンタを進めないステップかどうか
    #[inline]
    fn next_step_skips_length(&self) -> bool {
        self.frame_step & 1 == 1
    }

    /// タイマーの内部カウンタの該当ビットが立ち下がったとき（512Hz）に呼ぶ
    pub fn frame_sequencer_clock(&mut self) {
        if !self.powered {
            return;
        }
        let step = self.frame_step;
        self.frame_step = (step + 1) & 7;

        if step & 1 == 0 {
            // 256Hz: 長さカウンタ
            if self.ch1.length.clock() {
                self.ch1.enabled = false;
            }
            if self.ch2.length.clock() {
                self.ch2.enabled = false;
            }
            if self.ch3.length.clock() {
                self.ch3.enabled = false;
            }
            if self.ch4.length.clock() {
                self.ch4.enabled = false;
            }
        }
        if step == 2 || step == 6 {
            // 128Hz: スイープ
            self.ch1.sweep_clock();
        }
        if step == 7 {
            // 64Hz: 音量エンベロープ
            self.ch1.envelope.clock();
            self.ch2.envelope.clock();
            self.ch4.envelope.clock();
        }
        self.mix_dirty = true;
    }

    // ---- 時間を進める ----

    /// 1 M サイクル進める
    #[inline]
    pub fn tick(&mut self) {
        if self.powered {
            let changed = self.ch1.tick() | self.ch2.tick() | self.ch3.tick() | self.ch4.tick();
            self.mix_dirty |= changed;
        }
        if self.sample_rate > 0.0 {
            self.accumulate();
        }
    }

    /// 現在のミキサー出力を累積し、出力サンプルの時刻が来たら 1 個書き出す
    fn accumulate(&mut self) {
        if self.mix_dirty {
            self.mix_dirty = false;
            self.mix = self.mix_output();
        }
        self.sum.0 += self.mix.0;
        self.sum.1 += self.mix.1;
        self.sum_count += 1;

        self.sample_phase += self.sample_rate;
        if self.sample_phase < M_CYCLES_PER_SECOND {
            return;
        }
        self.sample_phase -= M_CYCLES_PER_SECOND;

        // 期間内の平均（簡易的な折り返し雑音対策）を取り、直流成分を取り除く
        let count = self.sum_count as f32;
        let input = (self.sum.0 / count, self.sum.1 / count);
        self.sum = (0.0, 0.0);
        self.sum_count = 0;
        let left = input.0 - self.capacitor.0;
        let right = input.1 - self.capacitor.1;
        self.capacitor.0 = input.0 - left * self.capacitor_charge;
        self.capacitor.1 = input.1 - right * self.capacitor_charge;

        // 取り出されないまま溜まり続けないよう、約 1 秒ぶんで打ち切る
        if self.samples.len() < self.sample_rate as usize * 2 {
            self.samples.push(left);
            self.samples.push(right);
        }
    }

    /// 4 チャンネルを NR50・NR51 に従って混ぜる。結果は -1.0〜1.0
    fn mix_output(&self) -> (f32, f32) {
        if !self.powered {
            return (0.0, 0.0);
        }
        // DAC はデジタル値 0〜15 を -1.0〜1.0 に変換する。DAC が切れているチャンネルは 0.0
        let dac = |enabled: bool, digital: u8| {
            if enabled {
                digital as f32 / 7.5 - 1.0
            } else {
                0.0
            }
        };
        let channels = [
            dac(self.ch1.envelope.dac_enabled(), self.ch1.output()),
            dac(self.ch2.envelope.dac_enabled(), self.ch2.output()),
            dac(self.ch3.dac_enabled, self.ch3.output()),
            dac(self.ch4.envelope.dac_enabled(), self.ch4.output()),
        ];
        let panning = self.regs[0x15];
        let mut left = 0.0;
        let mut right = 0.0;
        for (index, level) in channels.iter().enumerate() {
            if panning & (0x10 << index) != 0 {
                left += level;
            }
            if panning & (0x01 << index) != 0 {
                right += level;
            }
        }
        let volume = self.regs[0x14];
        let left_volume = ((volume >> 4) & 7) as f32 + 1.0;
        let right_volume = (volume & 7) as f32 + 1.0;
        // 4 チャンネル ÷ 4、音量 1〜8 ÷ 8
        (left * left_volume / 32.0, right * right_volume / 32.0)
    }

    /// 出力サンプルレート（Hz）を設定する。0 以下で音声の生成を止める。
    /// 再生側のバッファの増減に合わせて細かく変えてよい
    pub fn set_sample_rate(&mut self, hz: f64) {
        if hz > 0.0 {
            self.sample_rate = hz;
            // 実機（DMG）のコンデンサの放電率を出力サンプル間隔に換算したもの
            self.capacitor_charge = 0.999958_f64.powf(4_194_304.0 / hz) as f32;
        } else {
            self.sample_rate = 0.0;
            self.samples.clear();
        }
    }

    /// 溜まっているサンプル（左右交互、-1.0〜1.0）を取り出す
    pub fn take_samples(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    // ---- レジスタ ----

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF26 => {
                0x70 | (self.powered as u8) << 7
                    | (self.ch4.enabled as u8) << 3
                    | (self.ch3.enabled as u8) << 2
                    | (self.ch2.enabled as u8) << 1
                    | self.ch1.enabled as u8
            }
            0xFF10..=0xFF25 => {
                let index = (addr - 0xFF10) as usize;
                self.regs[index] | READ_MASKS[index]
            }
            0xFF30..=0xFF3F => self.ch3.read_ram((addr - 0xFF30) as usize),
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF30..=0xFF3F => {
                self.ch3.write_ram((addr - 0xFF30) as usize, value);
                return;
            }
            0xFF26 => {
                self.write_power(value & 0x80 != 0);
                return;
            }
            0xFF10..=0xFF25 => {}
            _ => return,
        }

        if !self.powered {
            // 電源が切れている間、レジスタへの書き込みは無視される。
            // ただし DMG では長さカウンタだけは書き込める
            match addr {
                0xFF11 => self.ch1.length.counter = 64 - (value & 0x3F) as u16,
                0xFF16 => self.ch2.length.counter = 64 - (value & 0x3F) as u16,
                0xFF1B => self.ch3.length.counter = 256 - value as u16,
                0xFF20 => self.ch4.length.counter = 64 - (value & 0x3F) as u16,
                _ => {}
            }
            return;
        }

        self.regs[(addr - 0xFF10) as usize] = value;
        self.mix_dirty = true;
        let skips_length = self.next_step_skips_length();
        match addr {
            // ---- チャンネル 1 ----
            0xFF10 => {
                let ch = &mut self.ch1;
                ch.sweep_period = (value >> 4) & 7;
                ch.sweep_shift = value & 7;
                let negate = value & 0x08 != 0;
                // 減算モードで一度でも計算した後に加算モードへ戻すと、チャンネルが止まる
                if ch.sweep_negate && !negate && ch.negate_used {
                    ch.enabled = false;
                }
                ch.sweep_negate = negate;
            }
            0xFF11 => {
                self.ch1.duty = value >> 6;
                self.ch1.length.counter = 64 - (value & 0x3F) as u16;
            }
            0xFF12 => {
                self.ch1.envelope.write(value);
                if !self.ch1.envelope.dac_enabled() {
                    self.ch1.enabled = false;
                }
            }
            0xFF13 => self.ch1.frequency = (self.ch1.frequency & 0x700) | value as u16,
            0xFF14 => {
                let ch = &mut self.ch1;
                ch.frequency = (ch.frequency & 0x0FF) | ((value as u16 & 7) << 8);
                if ch.length.write_control(value, 64, skips_length) {
                    ch.enabled = false;
                }
                if value & 0x80 != 0 {
                    ch.trigger(true);
                }
            }

            // ---- チャンネル 2 ----
            0xFF16 => {
                self.ch2.duty = value >> 6;
                self.ch2.length.counter = 64 - (value & 0x3F) as u16;
            }
            0xFF17 => {
                self.ch2.envelope.write(value);
                if !self.ch2.envelope.dac_enabled() {
                    self.ch2.enabled = false;
                }
            }
            0xFF18 => self.ch2.frequency = (self.ch2.frequency & 0x700) | value as u16,
            0xFF19 => {
                let ch = &mut self.ch2;
                ch.frequency = (ch.frequency & 0x0FF) | ((value as u16 & 7) << 8);
                if ch.length.write_control(value, 64, skips_length) {
                    ch.enabled = false;
                }
                if value & 0x80 != 0 {
                    ch.trigger(false);
                }
            }

            // ---- チャンネル 3 ----
            0xFF1A => {
                self.ch3.dac_enabled = value & 0x80 != 0;
                if !self.ch3.dac_enabled {
                    self.ch3.enabled = false;
                }
            }
            0xFF1B => self.ch3.length.counter = 256 - value as u16,
            0xFF1C => self.ch3.volume_code = (value >> 5) & 3,
            0xFF1D => self.ch3.frequency = (self.ch3.frequency & 0x700) | value as u16,
            0xFF1E => {
                let ch = &mut self.ch3;
                ch.frequency = (ch.frequency & 0x0FF) | ((value as u16 & 7) << 8);
                if ch.length.write_control(value, 256, skips_length) {
                    ch.enabled = false;
                }
                if value & 0x80 != 0 {
                    ch.trigger();
                }
            }

            // ---- チャンネル 4 ----
            0xFF20 => self.ch4.length.counter = 64 - (value & 0x3F) as u16,
            0xFF21 => {
                self.ch4.envelope.write(value);
                if !self.ch4.envelope.dac_enabled() {
                    self.ch4.enabled = false;
                }
            }
            0xFF22 => {
                self.ch4.shift = value >> 4;
                self.ch4.short_mode = value & 0x08 != 0;
                self.ch4.divisor_code = value & 7;
            }
            0xFF23 => {
                let ch = &mut self.ch4;
                if ch.length.write_control(value, 64, skips_length) {
                    ch.enabled = false;
                }
                if value & 0x80 != 0 {
                    ch.trigger();
                }
            }

            // NR50・NR51 は値を保存するだけ（ミキサーが参照する）
            _ => {}
        }
    }

    /// NR52 bit7（全体の電源）の切り替え
    fn write_power(&mut self, on: bool) {
        if on == self.powered {
            return;
        }
        self.powered = on;
        self.mix_dirty = true;
        if on {
            // 入れた直後: フレームシーケンサは最初のステップから、波形は先頭から始まる
            self.frame_step = 0;
            self.ch1.duty_position = 0;
            self.ch2.duty_position = 0;
            self.ch3.sample_byte = 0;
        } else {
            // 切ると NR10〜NR51 がすべて 0 になる。
            // DMG では長さカウンタの値だけは残る。波形 RAM も消えない
            self.regs = [0; 0x17];
            let lengths = [
                self.ch1.length.counter,
                self.ch2.length.counter,
                self.ch3.length.counter,
                self.ch4.length.counter,
            ];
            let ram = self.ch3.ram;
            self.ch1 = Square::default();
            self.ch2 = Square::default();
            self.ch3 = Wave {
                ram,
                ..Wave::default()
            };
            self.ch4 = Noise::default();
            self.ch1.length.counter = lengths[0];
            self.ch2.length.counter = lengths[1];
            self.ch3.length.counter = lengths[2];
            self.ch4.length.counter = lengths[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 電源を入れ直して、初期状態の影響がない APU を作る
    fn fresh() -> Apu {
        let mut apu = Apu::new();
        apu.write(0xFF26, 0x00);
        apu.write(0xFF26, 0x80);
        apu
    }

    #[test]
    fn registers_read_back_with_unused_bits_set() {
        let mut apu = fresh();
        apu.write(0xFF10, 0x00);
        assert_eq!(apu.read(0xFF10), 0x80);
        apu.write(0xFF11, 0x00);
        assert_eq!(apu.read(0xFF11), 0x3F);
        apu.write(0xFF13, 0x00);
        assert_eq!(apu.read(0xFF13), 0xFF, "周波数は読めない");
        apu.write(0xFF1C, 0x00);
        assert_eq!(apu.read(0xFF1C), 0x9F);
        assert_eq!(apu.read(0xFF15), 0xFF, "未使用レジスタ");
        assert_eq!(apu.read(0xFF26), 0xF0);
    }

    #[test]
    fn power_off_clears_registers_and_blocks_writes() {
        let mut apu = fresh();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF25, 0xFF);
        apu.write(0xFF30, 0x12);
        apu.write(0xFF26, 0x00);
        assert_eq!(apu.read(0xFF12), 0x00);
        assert_eq!(apu.read(0xFF25), 0x00);
        assert_eq!(apu.read(0xFF26), 0x70);
        apu.write(0xFF12, 0xF0);
        assert_eq!(apu.read(0xFF12), 0x00, "電源が切れている間は書けない");
        assert_eq!(apu.read(0xFF30), 0x12, "波形 RAM は消えない");
    }

    #[test]
    fn trigger_sets_status_and_length_expiry_clears_it() {
        let mut apu = fresh();
        apu.write(0xFF17, 0xF0); // CH2: 音量 15
        apu.write(0xFF16, 0x3E); // 長さ 2
        apu.write(0xFF19, 0xC0); // トリガー + 長さ有効
        assert_eq!(apu.read(0xFF26) & 0x02, 0x02);
        apu.frame_sequencer_clock(); // ステップ 0: 長さ 2 → 1
        assert_eq!(apu.read(0xFF26) & 0x02, 0x02);
        apu.frame_sequencer_clock(); // ステップ 1: 長さは進まない
        apu.frame_sequencer_clock(); // ステップ 2: 長さ 1 → 0
        assert_eq!(apu.read(0xFF26) & 0x02, 0x00);
    }

    #[test]
    fn trigger_with_dac_off_does_not_enable_channel() {
        let mut apu = fresh();
        apu.write(0xFF17, 0x00);
        apu.write(0xFF19, 0x80);
        assert_eq!(apu.read(0xFF26) & 0x02, 0x00);
    }

    #[test]
    fn sweep_overflow_disables_channel_on_trigger() {
        let mut apu = fresh();
        apu.write(0xFF12, 0xF0);
        apu.write(0xFF10, 0x01); // シフト 1、加算
        apu.write(0xFF13, 0xFF);
        apu.write(0xFF14, 0x87); // 周波数 0x7FF でトリガー → 計算結果が 2047 を超える
        assert_eq!(apu.read(0xFF26) & 0x01, 0x00);
    }

    #[test]
    fn enabling_length_in_first_half_of_period_clocks_it_once() {
        let mut apu = fresh();
        apu.write(0xFF17, 0xF0);
        apu.write(0xFF16, 0x3F); // 長さ 1
        apu.write(0xFF19, 0x80); // トリガー（長さは無効のまま）
        apu.frame_sequencer_clock(); // ステップ 0 を実行 → 次はステップ 1（長さを進めない）
        apu.write(0xFF19, 0x40); // 長さを有効にした瞬間に余分に 1 回進み、0 になって止まる
        assert_eq!(apu.read(0xFF26) & 0x02, 0x00);
    }

    #[test]
    fn square_wave_is_generated_at_the_programmed_pitch() {
        let mut apu = fresh();
        apu.set_sample_rate(48_000.0);
        apu.write(0xFF24, 0x77);
        apu.write(0xFF25, 0x22); // CH2 を左右に
        apu.write(0xFF16, 0x80); // デューティ 50%
        apu.write(0xFF17, 0xF0);
        // 周波数レジスタ 1792 → 131072 / (2048 - 1792) = 512 Hz
        apu.write(0xFF18, 0x00);
        apu.write(0xFF19, 0x87);
        for _ in 0..1_048_576 {
            apu.tick();
        }
        let samples = apu.take_samples();
        assert!(
            (samples.len() as i64 - 96_000).abs() <= 2,
            "1 秒ぶんのステレオ"
        );
        // 左チャンネルの「負 → 正」の横切り回数が周波数になる
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!((crossings as i64 - 512).abs() <= 2, "{crossings} Hz");
        assert!(left.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn no_samples_are_generated_until_a_rate_is_set() {
        let mut apu = fresh();
        for _ in 0..10_000 {
            apu.tick();
        }
        assert!(apu.take_samples().is_empty());
    }

    #[test]
    fn wave_ram_is_only_visible_at_the_moment_of_a_sample_read_while_playing() {
        let mut apu = fresh();
        for i in 0..16u16 {
            apu.write(0xFF30 + i, 0x10 + i as u8);
        }
        apu.write(0xFF1A, 0x80);
        apu.write(0xFF1D, 0xFD); // 周期は 2 T サイクル × 3。M サイクル（× 2）とは 1 回おきに揃う
        apu.write(0xFF1E, 0x87);
        assert_eq!(apu.read(0xFF30), 0xFF, "再生開始直後は読めない");
        // M サイクルの終わりちょうどにサンプルを読んだ回だけ、そのバイトが見える
        let mut seen = Vec::new();
        for _ in 0..8 {
            apu.tick();
            seen.push(apu.read(0xFF30));
        }
        assert!(seen.iter().any(|&v| v != 0xFF), "{seen:02X?}");
        assert!(seen.iter().all(|&v| v == 0xFF || (0x10..0x20).contains(&v)));
    }
}
