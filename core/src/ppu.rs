//! PPU（画面描画）。
//!
//! 1 ドット（= 1 T サイクル）単位でモードと LY を進め、モード 3 に入る時点で
//! その行をまとめて描画する（スキャンライン方式）。
//! 画面は 160×144、各ピクセルは 0（最も明るい）〜3（最も暗い）の濃淡値。

use crate::interrupts;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

const DOTS_PER_LINE: u16 = 456;
const LINES_PER_FRAME: u8 = 154;
/// モード 2（OAM サーチ）の長さ
const MODE2_DOTS: u16 = 80;
/// モード 3（描画）の最短の長さ
const MODE3_BASE_DOTS: u16 = 172;

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

    /// 内部の行カウンタ（0〜153）。LY レジスタとは 153 行目でずれる
    line: u8,
    /// 行内のドット位置（0〜455）
    dot: u16,
    mode: u8,
    /// この行でモード 0 に入るドット
    mode3_end: u16,
    /// STAT 割り込み線の直前の状態（立ち上がりでだけ割り込みが出る）
    stat_line: bool,
    /// LCD を ON にした直後の最初の行（モード 2 がなく、少し短い）
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
            // ブートROM終了時は VBlank の最終行（LY は 0 と読める）
            line: 153,
            dot: 400,
            mode: MODE_VBLANK,
            mode3_end: MODE2_DOTS + MODE3_BASE_DOTS,
            stat_line: false,
            first_line_after_enable: false,
            skip_frame: false,
            window_line: 0,
            wy_triggered: false,
            back: Box::new([0; FRAMEBUFFER_SIZE]),
            front: Box::new([0; FRAMEBUFFER_SIZE]),
            frame_ready: false,
        };
        ppu.stat_line = ppu.stat_condition();
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

    #[inline]
    fn lyc_match(&self) -> bool {
        self.ly == self.lyc
    }

    fn stat_condition(&self) -> bool {
        if !self.lcd_enabled() {
            return false;
        }
        (self.stat_enable & 0x08 != 0 && self.mode == MODE_HBLANK)
            || (self.stat_enable & 0x10 != 0 && self.mode == MODE_VBLANK)
            || (self.stat_enable & 0x20 != 0 && self.mode == MODE_OAM)
            || (self.stat_enable & 0x40 != 0 && self.lyc_match())
    }

    /// STAT 割り込み線を更新し、立ち上がりなら STAT 割り込みビットを返す
    fn update_stat_line(&mut self) -> u8 {
        let line = self.stat_condition();
        let rising = line && !self.stat_line;
        self.stat_line = line;
        if rising {
            interrupts::STAT
        } else {
            0
        }
    }

    /// 1 M サイクル（4 ドット）進める。要求する割り込みビットを返す
    pub fn tick(&mut self) -> u8 {
        if !self.lcd_enabled() {
            return 0;
        }
        let mut irq = 0;
        for _ in 0..4 {
            irq |= self.step_dot();
        }
        irq
    }

    fn step_dot(&mut self) -> u8 {
        let mut irq = 0;
        self.dot += 1;
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            self.line += 1;
            if self.line == LINES_PER_FRAME {
                self.line = 0;
            }
            self.first_line_after_enable = false;
            irq |= self.start_line();
        }

        if self.line < SCREEN_HEIGHT as u8 {
            if self.dot == MODE2_DOTS {
                self.mode = MODE_DRAW;
                self.mode3_end = MODE2_DOTS + self.mode3_length();
                self.render_line();
            } else if self.dot == self.mode3_end {
                self.mode = MODE_HBLANK;
            }
        } else if self.line == 153 && self.dot == 4 {
            // 最終行では LY が早々に 0 に戻る
            self.ly = 0;
        }

        irq | self.update_stat_line()
    }

    /// 行の先頭（ドット 0）での処理
    fn start_line(&mut self) -> u8 {
        self.ly = self.line;
        if self.line == 0 {
            self.window_line = 0;
            self.wy_triggered = false;
            self.skip_frame = false;
        }
        if self.line < SCREEN_HEIGHT as u8 {
            self.mode = MODE_OAM;
            if self.ly == self.wy {
                self.wy_triggered = true;
            }
            0
        } else if self.line == SCREEN_HEIGHT as u8 {
            self.mode = MODE_VBLANK;
            if !self.skip_frame {
                self.front.copy_from_slice(&self.back[..]);
            }
            self.frame_ready = true;
            interrupts::VBLANK
        } else {
            0
        }
    }

    /// モード 3 の長さ。スクロールの端数・スプライト・ウィンドウで延びる
    fn mode3_length(&self) -> u16 {
        // TODO(M4): スプライトとウィンドウによる延長を正確に入れる
        MODE3_BASE_DOTS + (self.scx & 0x07) as u16
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        if self.lcd_enabled() && self.mode == MODE_DRAW {
            return 0xFF; // 描画中は CPU から読めない
        }
        self.vram[addr as usize & 0x1FFF]
    }

    pub fn write_vram(&mut self, addr: u16, value: u8) {
        if self.lcd_enabled() && self.mode == MODE_DRAW {
            return;
        }
        self.vram[addr as usize & 0x1FFF] = value;
    }

    pub fn read_oam(&self, addr: u16) -> u8 {
        if self.lcd_enabled() && (self.mode == MODE_OAM || self.mode == MODE_DRAW) {
            return 0xFF;
        }
        self.oam[addr as usize & 0xFF]
    }

    pub fn write_oam(&mut self, addr: u16, value: u8) {
        if self.lcd_enabled() && (self.mode == MODE_OAM || self.mode == MODE_DRAW) {
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
            0xFF41 => {
                let mode = if self.lcd_enabled() { self.mode } else { 0 };
                let lyc = if self.lcd_enabled() && self.lyc_match() {
                    0x04
                } else {
                    0
                };
                0x80 | self.stat_enable | lyc | mode
            }
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
                // DMG の不具合: STAT に書き込んだ瞬間だけ全ての許可ビットが 1 として扱われ、
                // どれかの条件が成立していれば割り込みが出る
                let mut irq = 0;
                if self.lcd_enabled() {
                    let real = self.stat_enable;
                    self.stat_enable = 0x78;
                    irq |= self.update_stat_line();
                    self.stat_enable = real;
                }
                self.stat_enable = value & 0x78;
                return irq | self.update_stat_line();
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
        self.update_stat_line()
    }

    fn turn_off(&mut self) {
        self.line = 0;
        self.dot = 0;
        self.ly = 0;
        self.mode = MODE_HBLANK;
        self.stat_line = false;
        self.window_line = 0;
        self.wy_triggered = false;
        // LCD が消えている間は真っ白
        self.back.fill(0);
        self.front.fill(0);
    }

    fn turn_on(&mut self) {
        self.line = 0;
        self.dot = 4;
        self.ly = 0;
        // ON 直後の最初の行はモード 2 を経ず、モード 0 のままモード 3 に入る
        self.mode = MODE_HBLANK;
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

    fn render_line(&mut self) {
        if self.skip_frame {
            return;
        }
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

            let window_visible =
                self.lcdc & LCDC_WINDOW_ENABLE != 0 && self.wy_triggered && self.wx <= 166;
            if window_visible {
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
                self.window_line += 1;
            }
        }

        let mut shades = [0u8; SCREEN_WIDTH];
        for (shade, &color) in shades.iter_mut().zip(bg_colors.iter()) {
            *shade = (self.bgp >> (color * 2)) & 0x03;
        }

        if self.lcdc & LCDC_OBJ_ENABLE != 0 {
            let mut sprites = [LineSprite::default(); 10];
            let count = self.collect_sprites(&mut sprites);
            // DMG の優先順位: X が小さいほど手前、同じなら OAM の若い順。
            // 安定ソートなので、同じ X のものは OAM 順が保たれる
            let sprites = &mut sprites[..count];
            sprites.sort_by_key(|s| s.x);

            let mut drawn = [false; SCREEN_WIDTH];
            for sprite in sprites.iter() {
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
            if ppu.tick() & interrupts::VBLANK != 0 {
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
