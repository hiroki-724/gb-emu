//! PPU（画面描画）。
//!
//! 1 ドット（= 1 T サイクル）単位でモードと LY を進め、モード 3 に入る時点で
//! その行をまとめて描画する（スキャンライン方式）。
//! 画面は 160×144、各ピクセルは 0（最も明るい）〜3（最も暗い）の濃淡値。
//!
//! # タイミングのモデル
//!
//! 1 行は 456 ドット。行内のドット位置を「モード 2 が始まる位置を 0」として数える。
//!
//! | ドット | 起きること |
//! |---:|:--|
//! | 0 | モード 2 開始、LYC との比較が有効になる、OAM 書き込み禁止 |
//! | 76 | VRAM が読めなくなる。OAM への書き込みだけ一時的に通る（DMG） |
//! | 80 | モード 3 開始、VRAM と OAM が読み書きとも禁止 |
//! | 252 + 延長 | モード 0 開始、禁止解除。延長 = SCX の端数 + スプライト + ウィンドウ |
//! | 455 | LY が次の行の値になる、OAM 読み出し禁止、LYC 一致フラグはいったん 0 |
//!
//! CPU の 1 M サイクル（4 ドット）は、ドット位置が 4 の倍数のところから始まる。
//! つまりモードの切り替えは M サイクルの最初のドットで起き、LY の更新は最後のドットで起きる。
//! この位相は LCD を ON にした時点で決まり、DMG では常に同じになる
//! （mooneye-test-suite の lcdon_timing / hblank_ly_scx_timing / intr_2_* から求めた）。

use crate::interrupts;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

const DOTS_PER_LINE: u16 = 456;
/// VBlank が始まる行
const VBLANK_LINE: u8 = 144;
/// フレームの最終行
const LAST_LINE: u8 = 153;
/// モード 3 の準備が始まるドット。VRAM が読めなくなり、OAM への書き込みは一時的に通る
const MODE3_PREPARE_DOT: u16 = 76;
/// モード 3 に入るドット
const MODE3_START_DOT: u16 = 80;
/// モード 3（描画）の最短の長さ
const MODE3_BASE_DOTS: u16 = 172;
/// LY が次の行の値に切り替わるドット
const LY_UPDATE_DOT: u16 = 455;

const LCDC_BG_ENABLE: u8 = 0x01;
const LCDC_OBJ_ENABLE: u8 = 0x02;
const LCDC_OBJ_SIZE: u8 = 0x04;
const LCDC_BG_MAP: u8 = 0x08;
const LCDC_TILE_DATA: u8 = 0x10;
const LCDC_WINDOW_ENABLE: u8 = 0x20;
const LCDC_WINDOW_MAP: u8 = 0x40;
const LCDC_LCD_ENABLE: u8 = 0x80;

const MODE_HBLANK: u8 = 0;
const MODE_VBLANK: u8 = 1;
const MODE_OAM: u8 = 2;
const MODE_DRAW: u8 = 3;

/// その行に表示するスプライト
#[derive(Clone, Copy, Default)]
struct LineSprite {
    x: u8,
    /// スプライト内の何行目を描くか（上下反転を適用済み）
    row: u8,
    tile: u8,
    attr: u8,
}

#[derive(Clone)]
pub struct Ppu {
    vram: Box<[u8; 0x2000]>,
    oam: [u8; 0xA0],

    lcdc: u8,
    /// STAT の割り込み許可ビット（bit3〜6）
    stat_enable: u8,
    scy: u8,
    scx: u8,
    ly: u8,
    lyc: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,

    /// 内部の行カウンタ（0〜153）。LY レジスタとは行末と 153 行目でずれる
    line: u8,
    /// 行内のドット位置（0〜455）。直前に処理し終えたドットを指す
    dot: u16,
    /// この行で次に何かが起きるドット（456 = 行の折り返し）
    next_event: u16,
    /// STAT の下位 2bit として読めるモード
    mode: u8,
    /// STAT 割り込みの要因になっているモード。モード 2 は行頭の一瞬だけ要因になり、
    /// モード 3 は要因にならないので、読めるモードとは一致しない
    irq_mode: Option<u8>,
    /// LYC と比較される LY の値。None の間は比較が止まっている（LY の切り替わり直後）
    compare_ly: Option<u8>,
    /// STAT bit2（LYC 一致フラグ）。LCD が消えている間は直前の値を保つ
    lyc_flag: bool,
    /// LYC 一致による割り込み要因。比較が止まっている間も直前の状態を保つ
    lyc_irq: bool,
    /// STAT 割り込み線の直前の状態（立ち上がりでだけ割り込みが出る）
    stat_line: bool,
    /// この行でモード 0 に入るドット
    mode3_end: u16,
    oam_read_blocked: bool,
    oam_write_blocked: bool,
    vram_read_blocked: bool,
    vram_write_blocked: bool,
    /// LCD を ON にした直後の最初の行（モード 2 がなく、モード 3 の準備期間もない）
    first_line_after_enable: bool,
    /// LCD を ON にした直後のフレームは実機でも表示されない
    skip_frame: bool,

    /// ウィンドウ用の内部行カウンタ
    window_line: u8,
    /// このフレームで LY == WY が成立したか
    wy_triggered: bool,

    /// 描画中のフレーム
    back: Box<[u8; FRAMEBUFFER_SIZE]>,
    /// 完成したフレーム（フロントエンドに見せるのはこちら）
    front: Box<[u8; FRAMEBUFFER_SIZE]>,
    /// VBlank に入ってフレームが完成したら立つ
    pub frame_ready: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    /// DMG のブートROM終了直後の状態で作る
    pub fn new() -> Self {
        let mut ppu = Ppu {
            vram: Box::new([0; 0x2000]),
            oam: [0; 0xA0],
            lcdc: 0x91,
            stat_enable: 0,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            // ブートROM終了時は VBlank の最終行（LY は 0 と読める）。
            // 行内の正確な位置は未確認で、M サイクルとの位相（4 で割って 3 余る）だけ合わせてある
            line: LAST_LINE,
            dot: 399,
            next_event: LY_UPDATE_DOT,
            mode: MODE_VBLANK,
            irq_mode: Some(MODE_VBLANK),
            compare_ly: Some(0),
            lyc_flag: true,
            lyc_irq: true,
            stat_line: false,
            mode3_end: u16::MAX,
            oam_read_blocked: false,
            oam_write_blocked: false,
            vram_read_blocked: false,
            vram_write_blocked: false,
            first_line_after_enable: false,
            skip_frame: false,
            window_line: 0,
            wy_triggered: false,
            back: Box::new([0; FRAMEBUFFER_SIZE]),
            front: Box::new([0; FRAMEBUFFER_SIZE]),
            frame_ready: false,
        };
        ppu.update_stat();
        ppu
    }

    /// 完成済みフレーム（160×144、各要素 0〜3）
    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.front
    }

    #[inline]
    fn lcd_enabled(&self) -> bool {
        self.lcdc & LCDC_LCD_ENABLE != 0
    }

    /// LYC 一致フラグと STAT 割り込み線を更新し、立ち上がりなら STAT 割り込みビットを返す。
    /// LCD が消えている間は呼ばない（実機でも比較回路と割り込み線は止まったまま値を保つ）
    fn update_stat(&mut self) -> u8 {
        match self.compare_ly {
            Some(ly) => {
                self.lyc_flag = ly == self.lyc;
                self.lyc_irq = self.lyc_flag;
            }
            None => self.lyc_flag = false,
        }
        let mode_source = match self.irq_mode {
            Some(MODE_HBLANK) => 0x08,
            Some(MODE_VBLANK) => 0x10,
            Some(MODE_OAM) => 0x20,
            _ => 0,
        };
        let line =
            self.stat_enable & mode_source != 0 || (self.stat_enable & 0x40 != 0 && self.lyc_irq);
        let rising = line && !self.stat_line;
        self.stat_line = line;
        if rising {
            interrupts::STAT
        } else {
            0
        }
    }

    /// 1 M サイクル（4 ドット）進める。要求する割り込みビットを
    /// （最初のドットで出たもの, 残り 3 ドットで出たもの）に分けて返す。
    /// CPU が割り込みを受け付けるかどうかは最初のドットの直後の状態で決まるため、区別が必要
    pub fn tick(&mut self) -> (u8, u8) {
        if !self.lcd_enabled() {
            return (0, 0);
        }
        if self.dot + 4 < self.next_event {
            // この 4 ドットの間は何も起きない
            self.dot += 4;
            return (0, 0);
        }
        let early = self.step_dot();
        let late = self.step_dot() | self.step_dot() | self.step_dot();
        (early, late)
    }

    fn step_dot(&mut self) -> u8 {
        self.dot += 1;
        if self.dot < self.next_event {
            return 0;
        }
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            self.line = if self.line == LAST_LINE {
                0
            } else {
                self.line + 1
            };
            self.first_line_after_enable = false;
        }

        let mut irq = 0;
        if self.line < VBLANK_LINE {
            match self.dot {
                0 => irq |= self.start_visible_line(),
                MODE3_PREPARE_DOT => {
                    if !self.first_line_after_enable {
                        self.oam_write_blocked = false;
                        self.vram_read_blocked = true;
                    }
                }
                MODE3_START_DOT => self.start_drawing(),
                LY_UPDATE_DOT => {
                    self.ly = self.line + 1;
                    self.compare_ly = None;
                    // 次が表示行なら、モード 2 より 1 ドット早く OAM が読めなくなる
                    self.oam_read_blocked = self.line + 1 < VBLANK_LINE;
                }
                dot if dot == self.mode3_end => {
                    self.mode = MODE_HBLANK;
                    self.irq_mode = Some(MODE_HBLANK);
                    self.oam_read_blocked = false;
                    self.oam_write_blocked = false;
                    self.vram_read_blocked = false;
                    self.vram_write_blocked = false;
                }
                _ => {}
            }
        } else {
            match (self.line, self.dot) {
                (VBLANK_LINE, 0) => irq |= self.start_vblank(),
                // 最終行: LY=153 は 1 M サイクルしか読めず、すぐ 0 になる。
                // LYC との比較は 153 →（停止）→ 0 の順に切り替わる
                (LAST_LINE, 0) => {}
                (LAST_LINE, 2) => {
                    self.ly = 0;
                    self.compare_ly = Some(LAST_LINE);
                }
                (LAST_LINE, 4) => self.compare_ly = None,
                (LAST_LINE, 8) => self.compare_ly = Some(0),
                (LAST_LINE, LY_UPDATE_DOT) => {
                    // LY は既に 0。DMG では VBlank の最後の 1 M サイクルだけモード 0 と読める
                    self.mode = MODE_HBLANK;
                    self.oam_read_blocked = true;
                }
                (_, 0) => self.compare_ly = Some(self.line),
                (_, LY_UPDATE_DOT) => {
                    self.ly = self.line + 1;
                    self.compare_ly = None;
                }
                _ => {}
            }
        }
        self.next_event = self.next_event_after_current_dot();
        irq | self.update_stat()
    }

    /// 現在の行で、いまのドットより後に何かが起きる最初のドット
    fn next_event_after_current_dot(&self) -> u16 {
        let dot = self.dot;
        if dot >= LY_UPDATE_DOT {
            return DOTS_PER_LINE;
        }
        if self.line < VBLANK_LINE {
            if dot < MODE3_PREPARE_DOT {
                MODE3_PREPARE_DOT
            } else if dot < MODE3_START_DOT {
                MODE3_START_DOT
            } else if dot < self.mode3_end {
                self.mode3_end
            } else {
                LY_UPDATE_DOT
            }
        } else if self.line == LAST_LINE {
            match dot {
                0..=1 => 2,
                2..=3 => 4,
                4..=7 => 8,
                _ => LY_UPDATE_DOT,
            }
        } else {
            LY_UPDATE_DOT
        }
    }

    /// 表示行の先頭（ドット 0）: モード 2 に入る
    fn start_visible_line(&mut self) -> u8 {
        if self.line == 0 {
            self.window_line = 0;
            self.wy_triggered = false;
            self.skip_frame = false;
        }
        self.mode = MODE_OAM;
        self.compare_ly = Some(self.line);
        self.oam_read_blocked = true;
        self.oam_write_blocked = true;
        if self.line == self.wy {
            self.wy_triggered = true;
        }
        // モード 2 の割り込み要因は行頭の一瞬だけ立つ
        self.irq_mode = Some(MODE_OAM);
        let irq = self.update_stat();
        self.irq_mode = None;
        irq
    }

    /// ドット 80: モード 3 に入り、この行を描画する
    fn start_drawing(&mut self) {
        self.mode = MODE_DRAW;
        self.irq_mode = None;
        self.oam_read_blocked = true;
        self.oam_write_blocked = true;
        self.vram_read_blocked = true;
        self.vram_write_blocked = true;
        if self.line == self.wy {
            self.wy_triggered = true;
        }

        let mut sprites = [LineSprite::default(); 10];
        let count = if self.lcdc & LCDC_OBJ_ENABLE != 0 {
            self.collect_sprites(&mut sprites)
        } else {
            0
        };
        let sprites = &mut sprites[..count];
        // DMG の優先順位: X が小さいほど手前、同じなら OAM の若い順。
        // 安定ソートなので、同じ X のものは OAM 順が保たれる
        sprites.sort_by_key(|sprite| sprite.x);

        // DMG では LCDC bit0 を落とすと背景とウィンドウの両方が消える
        let window = self.lcdc & (LCDC_BG_ENABLE | LCDC_WINDOW_ENABLE)
            == (LCDC_BG_ENABLE | LCDC_WINDOW_ENABLE)
            && self.wy_triggered
            && self.wx <= 166;

        self.mode3_end = MODE3_START_DOT + self.mode3_length(sprites, window);
        if !self.skip_frame {
            self.render_line(sprites, window);
        }
        if window {
            self.window_line += 1;
        }
    }

    /// VBlank の開始（144 行目のドット 0）
    fn start_vblank(&mut self) -> u8 {
        self.mode = MODE_VBLANK;
        self.compare_ly = Some(VBLANK_LINE);
        if !self.skip_frame {
            self.front.copy_from_slice(&self.back[..]);
        }
        self.frame_ready = true;
        // VBlank に入る瞬間、モード 2 の割り込み要因も一瞬だけ立つ（vblank_stat_intr）
        self.irq_mode = Some(MODE_OAM);
        let irq = self.update_stat();
        self.irq_mode = Some(MODE_VBLANK);
        irq | interrupts::VBLANK
    }

    /// モード 3 の長さ。スクロールの端数・ウィンドウ・スプライトで延びる。
    /// スプライトぶんは Pan Docs の "OBJ penalty algorithm" による
    fn mode3_length(&self, sprites: &[LineSprite], window: bool) -> u16 {
        let mut dots = MODE3_BASE_DOTS + (self.scx & 0x07) as u16;
        if window {
            dots += 6;
        }

        // スプライトの左端が乗っている背景（またはウィンドウ）タイルごとに、最初の 1 個だけ
        // 「背景タイルの取得が終わるのを待つ」ぶんが加わる
        let mut considered = [false; 128];
        let window_start = self.wx as i16 - 7;
        for sprite in sprites {
            if sprite.x >= 168 {
                continue; // 画面の右外にあるものは取得されない
            }
            dots += 6;
            let screen_x = sprite.x as i16 - 8;
            let (tile, offset) = if sprite.x == 0 {
                // X=0 は SCX に関係なく、タイルの左端に乗っている扱いになる
                (127, 0)
            } else if window && screen_x >= window_start {
                let position = (screen_x - window_start) as usize;
                (64 + position / 8, position % 8)
            } else {
                let position = sprite.x as usize + self.scx as usize;
                (position / 8, position % 8)
            };
            if !considered[tile] {
                considered[tile] = true;
                dots += 5u16.saturating_sub(offset as u16);
            }
        }
        dots
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        if self.vram_read_blocked {
            return 0xFF; // 描画中は CPU から読めない
        }
        self.vram[addr as usize & 0x1FFF]
    }

    pub fn write_vram(&mut self, addr: u16, value: u8) {
        if self.vram_write_blocked {
            return;
        }
        self.vram[addr as usize & 0x1FFF] = value;
    }

    pub fn read_oam(&self, addr: u16) -> u8 {
        if self.oam_read_blocked {
            return 0xFF;
        }
        self.oam[addr as usize & 0xFF]
    }

    pub fn write_oam(&mut self, addr: u16, value: u8) {
        if self.oam_write_blocked {
            return;
        }
        self.oam[addr as usize & 0xFF] = value;
    }

    /// モードによるブロックを受けずに VRAM を読む（OAM DMA の転送元・デバッグ用）
    pub fn peek_vram(&self, addr: u16) -> u8 {
        self.vram[addr as usize & 0x1FFF]
    }

    /// OAM DMA からの書き込み（モードによるブロックを受けない）
    pub fn dma_write_oam(&mut self, index: usize, value: u8) {
        self.oam[index] = value;
    }

    pub fn read_register(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => 0x80 | self.stat_enable | ((self.lyc_flag as u8) << 2) | self.mode,
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            _ => 0xFF,
        }
    }

    /// レジスタへの書き込み。要求する割り込みビットを返す
    pub fn write_register(&mut self, addr: u16, value: u8) -> u8 {
        match addr {
            0xFF40 => {
                let was_enabled = self.lcd_enabled();
                self.lcdc = value;
                if was_enabled && !self.lcd_enabled() {
                    self.turn_off();
                } else if !was_enabled && self.lcd_enabled() {
                    self.turn_on();
                }
            }
            0xFF41 => {
                let mut irq = 0;
                if self.lcd_enabled() {
                    // DMG の不具合: STAT に書き込んだ瞬間だけ全ての許可ビットが 1 として扱われ、
                    // どれかの要因が成立していれば割り込みが出る
                    self.stat_enable = 0x78;
                    irq = self.update_stat();
                }
                self.stat_enable = value & 0x78;
                if self.lcd_enabled() {
                    irq |= self.update_stat();
                }
                return irq;
            }
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF44 => {} // LY は読み取り専用
            0xFF45 => self.lyc = value,
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wy = value,
            0xFF4B => self.wx = value,
            _ => {}
        }
        if self.lcd_enabled() {
            self.update_stat()
        } else {
            0
        }
    }

    fn turn_off(&mut self) {
        self.line = 0;
        self.dot = 0;
        self.ly = 0;
        self.mode = MODE_HBLANK;
        self.irq_mode = None;
        // 再び ON にしたとき LY=0 との比較から始まる。消えている間は比較回路が止まるので、
        // LYC 一致フラグと割り込み線は直前の状態を保つ（stat_lyc_onoff）
        self.compare_ly = Some(0);
        self.oam_read_blocked = false;
        self.oam_write_blocked = false;
        self.vram_read_blocked = false;
        self.vram_write_blocked = false;
        self.window_line = 0;
        self.wy_triggered = false;
        // LCD が消えている間は真っ白
        self.back.fill(0);
        self.front.fill(0);
    }

    fn turn_on(&mut self) {
        // ON 直後の最初の行はモード 2 を経ず、モード 0 のままモード 3 に入る。
        // モード 3 に入るのは ON から 77 ドット後（通常の行より 3 ドットぶん先に進んだ位置から始まる）
        self.line = 0;
        self.dot = 3;
        self.ly = 0;
        self.mode = MODE_HBLANK;
        self.irq_mode = None;
        self.compare_ly = Some(0);
        self.mode3_end = u16::MAX;
        self.next_event = self.next_event_after_current_dot();
        self.first_line_after_enable = true;
        self.skip_frame = true;
        self.window_line = 0;
        self.wy_triggered = self.wy == 0;
    }

    // ---- 描画 ----

    #[inline]
    fn tile_row(&self, tile: u8, row: u8, unsigned_addressing: bool) -> (u8, u8) {
        // LCDC bit4 = 0 のとき、タイル 0〜127 は 0x9000 から、128〜255 は 0x8800 から取る
        let base = if unsigned_addressing || tile >= 0x80 {
            tile as usize * 16
        } else {
            0x1000 + tile as usize * 16
        };
        let addr = base + row as usize * 2;
        (self.vram[addr], self.vram[addr + 1])
    }

    fn collect_sprites(&self, out: &mut [LineSprite; 10]) -> usize {
        let height = if self.lcdc & LCDC_OBJ_SIZE != 0 {
            16
        } else {
            8
        };
        let ly = self.line as i16;
        let mut count = 0;
        for entry in self.oam.as_chunks::<4>().0 {
            let top = entry[0] as i16 - 16;
            if ly < top || ly >= top + height {
                continue;
            }
            let attr = entry[3];
            let mut row = (ly - top) as u8;
            if attr & 0x40 != 0 {
                row = height as u8 - 1 - row;
            }
            let tile = if height == 16 {
                // 8×16 では上半分が偶数タイル、下半分が奇数タイル
                (entry[2] & 0xFE) | (row >> 3)
            } else {
                entry[2]
            };
            out[count] = LineSprite {
                x: entry[1],
                row: row & 0x07,
                tile,
                attr,
            };
            count += 1;
            if count == out.len() {
                break; // 1 行に表示できるのは 10 個まで
            }
        }
        count
    }

    fn render_line(&mut self, sprites: &[LineSprite], window: bool) {
        let ly = self.line;
        // 背景とウィンドウの色番号（パレット適用前）。スプライトとの優先順位判定に使う
        let mut bg_colors = [0u8; SCREEN_WIDTH];

        if self.lcdc & LCDC_BG_ENABLE != 0 {
            let unsigned = self.lcdc & LCDC_TILE_DATA != 0;

            let map_base = if self.lcdc & LCDC_BG_MAP != 0 {
                0x1C00
            } else {
                0x1800
            };
            let y = self.scy.wrapping_add(ly);
            let map_row = map_base + (y as usize / 8) * 32;
            for (x, color) in bg_colors.iter_mut().enumerate() {
                let px = self.scx.wrapping_add(x as u8);
                let tile = self.vram[map_row + px as usize / 8];
                let (lo, hi) = self.tile_row(tile, y & 0x07, unsigned);
                let bit = 7 - (px & 0x07);
                *color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            }

            if window {
                let map_base = if self.lcdc & LCDC_WINDOW_MAP != 0 {
                    0x1C00
                } else {
                    0x1800
                };
                let wy = self.window_line;
                let map_row = map_base + (wy as usize / 8) * 32;
                let start = self.wx as i16 - 7;
                for x in start.max(0)..SCREEN_WIDTH as i16 {
                    let px = (x - start) as u8;
                    let tile = self.vram[map_row + px as usize / 8];
                    let (lo, hi) = self.tile_row(tile, wy & 0x07, unsigned);
                    let bit = 7 - (px & 0x07);
                    bg_colors[x as usize] = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                }
            }
        }

        let mut shades = [0u8; SCREEN_WIDTH];
        for (shade, &color) in shades.iter_mut().zip(bg_colors.iter()) {
            *shade = (self.bgp >> (color * 2)) & 0x03;
        }

        // スプライトは優先度の高い順（X の小さい順）に並んでいる
        let mut drawn = [false; SCREEN_WIDTH];
        for sprite in sprites {
            let (lo, hi) = self.tile_row(sprite.tile, sprite.row, true);
            let palette = if sprite.attr & 0x10 != 0 {
                self.obp1
            } else {
                self.obp0
            };
            for i in 0..8u8 {
                let x = sprite.x as i16 - 8 + i as i16;
                if !(0..SCREEN_WIDTH as i16).contains(&x) {
                    continue;
                }
                let x = x as usize;
                if drawn[x] {
                    continue; // より優先度の高いスプライトが既にこのピクセルを取っている
                }
                let bit = if sprite.attr & 0x20 != 0 { i } else { 7 - i };
                let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                if color == 0 {
                    continue; // 色 0 は透明
                }
                drawn[x] = true;
                // bit7: 背景の色 1〜3 の後ろに隠れる
                if sprite.attr & 0x80 != 0 && bg_colors[x] != 0 {
                    continue;
                }
                shades[x] = (palette >> (color * 2)) & 0x03;
            }
        }

        let offset = ly as usize * SCREEN_WIDTH;
        self.back[offset..offset + SCREEN_WIDTH].copy_from_slice(&shades);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_at_frame_start() -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(0xFF40, 0x00);
        ppu.write_register(0xFF40, 0x91);
        // LCD を ON にした直後のフレームは表示されないので 1 フレーム進めておく
        while !ppu.frame_ready {
            ppu.tick();
        }
        ppu.frame_ready = false;
        ppu
    }

    fn run_frame(ppu: &mut Ppu) {
        while !ppu.frame_ready {
            ppu.tick();
        }
        ppu.frame_ready = false;
    }

    /// LCD を ON にしてから n M サイクル後の (LY, STAT の下位 3bit)
    fn after_lcd_on(cycles: usize, lyc: u8) -> (u8, u8) {
        let mut ppu = Ppu::new();
        ppu.write_register(0xFF40, 0x00);
        ppu.write_register(0xFF45, lyc);
        ppu.write_register(0xFF40, 0x81);
        for _ in 0..cycles {
            ppu.tick();
        }
        (ppu.read_register(0xFF44), ppu.read_register(0xFF41) & 0x07)
    }

    #[test]
    fn first_line_after_lcd_on_has_no_mode2_and_starts_mode3_at_dot_77() {
        // 値は mooneye-test-suite の lcdon_timing-GS で実機確認されているもの
        assert_eq!(
            after_lcd_on(2, 0),
            (0, 0b100),
            "ON 直後はモード 0、LYC=0 が一致"
        );
        assert_eq!(after_lcd_on(19, 0), (0, 0b100), "76 ドット後はまだモード 0");
        assert_eq!(after_lcd_on(20, 0), (0, 0b111), "80 ドット後はモード 3");
        assert_eq!(after_lcd_on(62, 0), (0, 0b111));
        assert_eq!(after_lcd_on(63, 0), (0, 0b100), "252 ドット後はモード 0");
        assert_eq!(after_lcd_on(112, 0), (0, 0b100));
    }

    #[test]
    fn ly_increments_one_cycle_before_mode2_and_lyc_flag_follows_mode2() {
        assert_eq!(
            after_lcd_on(113, 1),
            (1, 0b000),
            "LY は 1 だがモード 0、一致フラグもまだ立たない"
        );
        assert_eq!(
            after_lcd_on(114, 1),
            (1, 0b110),
            "モード 2 と同時に一致フラグ"
        );
        assert_eq!(after_lcd_on(133, 1), (1, 0b110));
        assert_eq!(after_lcd_on(134, 1), (1, 0b111), "モード 2 は 80 ドット");
        assert_eq!(after_lcd_on(176, 1), (1, 0b111));
        assert_eq!(
            after_lcd_on(177, 1),
            (1, 0b100),
            "モード 3 は最短 172 ドット"
        );
        assert_eq!(after_lcd_on(227, 1), (2, 0b000));
    }

    #[test]
    fn mode3_is_extended_by_scroll_and_sprites() {
        let sprite = |x: u8| LineSprite {
            x,
            ..LineSprite::default()
        };
        let mut ppu = Ppu::new();
        assert_eq!(ppu.mode3_length(&[], false), 172);
        // 値は mooneye-test-suite の intr_2_mode0_timing_sprites と一致するもの
        assert_eq!(ppu.mode3_length(&[sprite(0)], false), 172 + 11);
        assert_eq!(ppu.mode3_length(&[sprite(4)], false), 172 + 7);
        assert_eq!(ppu.mode3_length(&[sprite(7)], false), 172 + 6);
        assert_eq!(
            ppu.mode3_length(&[sprite(168)], false),
            172,
            "画面外は数えない"
        );
        assert_eq!(ppu.mode3_length(&[sprite(0); 10], false), 172 + 11 + 9 * 6);
        assert_eq!(
            ppu.mode3_length(&[sprite(0), sprite(8)], false),
            172 + 22,
            "別のタイルに乗っていれば両方に待ち時間が付く"
        );
        ppu.write_register(0xFF43, 5);
        assert_eq!(ppu.mode3_length(&[], false), 172 + 5);
    }

    #[test]
    fn vblank_also_pulses_mode2_stat_interrupt() {
        let mut ppu = ppu_at_frame_start();
        ppu.frame_ready = false;
        ppu.write_register(0xFF41, 0x20);
        let mut at_vblank = 0;
        while !ppu.frame_ready {
            let (early, late) = ppu.tick();
            at_vblank = early | late;
        }
        assert_eq!(at_vblank, interrupts::VBLANK | interrupts::STAT);
    }

    #[test]
    fn lyc_flag_is_frozen_while_lcd_is_off() {
        let mut ppu = ppu_at_frame_start();
        // VBlank 開始直後（LY=144）で LYC を合わせてから消す
        ppu.write_register(0xFF45, 144);
        assert_eq!(ppu.read_register(0xFF41) & 0x04, 0x04);
        ppu.write_register(0xFF40, 0x00);
        assert_eq!(
            ppu.read_register(0xFF41) & 0x07,
            0x04,
            "消してもフラグは残る"
        );
        ppu.write_register(0xFF45, 1);
        assert_eq!(
            ppu.read_register(0xFF41) & 0x04,
            0x04,
            "消えている間は比較されない"
        );
        ppu.write_register(0xFF40, 0x80);
        assert_eq!(
            ppu.read_register(0xFF41) & 0x04,
            0x00,
            "ON で LY=0 と比較し直す"
        );
    }

    #[test]
    fn frame_is_70224_dots() {
        let mut ppu = ppu_at_frame_start();
        let mut ticks = 0;
        while !ppu.frame_ready {
            ppu.tick();
            ticks += 1;
        }
        assert_eq!(ticks * 4, 70224);
    }

    #[test]
    fn ly_counts_up_and_vblank_requests_interrupt() {
        let mut ppu = ppu_at_frame_start();
        let mut saw_vblank = false;
        let mut max_ly = 0;
        for _ in 0..70224 / 4 {
            let (early, late) = ppu.tick();
            if (early | late) & interrupts::VBLANK != 0 {
                saw_vblank = true;
            }
            max_ly = max_ly.max(ppu.read_register(0xFF44));
        }
        assert!(saw_vblank);
        assert_eq!(max_ly, 153);
    }

    #[test]
    fn background_tile_is_drawn_with_palette() {
        let mut ppu = ppu_at_frame_start();
        ppu.write_register(0xFF40, 0x00);
        // タイル 0 の 1 行目: 左から色 3,2,1,0,3,2,1,0
        ppu.vram[0] = 0b1010_1010;
        ppu.vram[1] = 0b1100_1100;
        ppu.write_register(0xFF47, 0b1110_0100);
        ppu.write_register(0xFF40, 0x91);
        run_frame(&mut ppu);
        run_frame(&mut ppu);
        let fb = ppu.framebuffer();
        assert_eq!(&fb[0..4], &[3, 2, 1, 0]);
    }

    #[test]
    fn sprite_is_drawn_over_background_and_respects_x_priority() {
        let mut ppu = ppu_at_frame_start();
        ppu.write_register(0xFF40, 0x00);
        // タイル 1: 全ピクセル色 1、タイル 2: 全ピクセル色 3
        for row in 0..8 {
            ppu.vram[16 + row * 2] = 0xFF;
            ppu.vram[32 + row * 2] = 0xFF;
            ppu.vram[32 + row * 2 + 1] = 0xFF;
        }
        // OAM 0: X=12（画面 x=4〜11）タイル 1 / OAM 1: X=8（画面 x=0〜7）タイル 2
        ppu.oam[0..4].copy_from_slice(&[16, 12, 1, 0]);
        ppu.oam[4..8].copy_from_slice(&[16, 8, 2, 0]);
        ppu.write_register(0xFF47, 0x00);
        ppu.write_register(0xFF48, 0b1110_0100);
        ppu.write_register(0xFF40, 0x93);
        run_frame(&mut ppu);
        run_frame(&mut ppu);
        let fb = ppu.framebuffer();
        // x=4〜7 は両方が重なるが、X が小さい OAM 1（色 3）が手前
        assert_eq!(&fb[0..12], &[3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1]);
        assert_eq!(fb[12], 0);
    }
}
