//! ゲームボーイ（DMG）エミュレータ本体。
//!
//! 外部クレートにも OS の入出力にも依存しない。フロントエンドとのやり取りは
//! - 画面: 160×144 のフレームバッファ（各ピクセル 0〜3 の濃淡）
//! - 入力: ボタンの押下状態
//! - 音: ステレオのサンプル列
//!
//! だけで行う。

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod interrupts;
pub mod joypad;
pub mod ppu;
pub mod serial;
pub mod timer;

pub use cartridge::{Cartridge, CartridgeError, MbcKind};
pub use joypad::Button;
pub use ppu::{FRAMEBUFFER_SIZE, SCREEN_HEIGHT, SCREEN_WIDTH};

use bus::Bus;
use cpu::Cpu;

/// 1 フレームの長さ（M サイクル）。70224 ドット ÷ 4
pub const M_CYCLES_PER_FRAME: u64 = 17_556;
/// CPU クロック（Hz）
pub const CLOCK_HZ: u32 = 4_194_304;
/// 1 秒あたりのフレーム数（約 59.73）
pub const FRAMES_PER_SECOND: f64 = CLOCK_HZ as f64 / 70_224.0;

#[derive(Clone)]
pub struct GameBoy {
    pub cpu: Cpu,
    pub bus: Bus,
}

impl GameBoy {
    /// ROM イメージからエミュレータを作る。ブートROM終了直後の状態から始まる
    pub fn new(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let cart = Cartridge::new(rom)?;
        Ok(GameBoy {
            cpu: Cpu::new(),
            bus: Bus::new(cart),
        })
    }

    /// CPU を 1 命令ぶん進める
    #[inline]
    pub fn step(&mut self) {
        self.cpu.step(&mut self.bus);
    }

    /// 次の VBlank まで（LCD が消えているときは 1 フレーム相当の時間だけ）進める
    pub fn run_frame(&mut self) {
        // フレーム境界と命令境界のずれを吸収するための少しの余裕
        const SLACK: u64 = 16;
        let start = self.bus.cycles();
        self.bus.ppu.frame_ready = false;
        while !self.bus.ppu.frame_ready && self.bus.cycles() - start < M_CYCLES_PER_FRAME + SLACK {
            self.cpu.step(&mut self.bus);
        }
    }

    /// 完成済みの画面（160×144、各要素は 0 = 最も明るい 〜 3 = 最も暗い）
    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.bus.ppu.framebuffer()
    }

    pub fn set_button(&mut self, button: Button, pressed: bool) {
        if self.bus.joypad.set_button(button, pressed) {
            self.bus.request_interrupt(interrupts::JOYPAD);
        }
    }

    /// 全ボタンの状態をまとめて設定する（ビット配置は [`Button`] の値と同じ、1 = 押下）
    pub fn set_buttons(&mut self, pressed: u8) {
        if self.bus.joypad.set_state(pressed) {
            self.bus.request_interrupt(interrupts::JOYPAD);
        }
    }

    /// シリアルポートに送信されたバイト列を取り出す
    pub fn take_serial_output(&mut self) -> Vec<u8> {
        self.bus.serial.take_output()
    }

    /// 音声の出力サンプルレート（Hz）を設定する。設定するまで音声は生成されない。
    /// 0 以下を渡すと生成を止める
    pub fn set_audio_sample_rate(&mut self, hz: f64) {
        self.bus.apu.set_sample_rate(hz);
    }

    /// 溜まっている音声サンプル（左右交互の f32、-1.0〜1.0）を取り出す
    pub fn take_audio_samples(&mut self) -> Vec<f32> {
        self.bus.apu.take_samples()
    }

    /// 起動からの経過 M サイクル数
    pub fn cycles(&self) -> u64 {
        self.bus.cycles()
    }

    /// 時間を進めずにメモリを読む
    pub fn peek(&self, addr: u16) -> u8 {
        self.bus.peek(addr)
    }

    /// 描画中かどうかに関係なく VRAM（0x8000〜0x9FFF）を読む（デバッグ・テスト判定用）
    pub fn peek_vram(&self, addr: u16) -> u8 {
        self.bus.ppu.peek_vram(addr)
    }

    /// `LD B,B` が実行されていたら true を返し、フラグを下ろす
    pub fn take_breakpoint(&mut self) -> bool {
        std::mem::take(&mut self.cpu.breakpoint_hit)
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.bus.cart
    }

    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.bus.cart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x0100 から始まる小さなプログラムを持つ ROM を作る
    fn rom_with_program(program: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x100 + program.len()].copy_from_slice(program);
        rom
    }

    #[test]
    fn serial_output_is_captured() {
        // LD A,'O' / LDH (01),A / LD A,0x81 / LDH (02),A / JR -2
        let rom = rom_with_program(&[0x3E, b'O', 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, 0x18, 0xFE]);
        let mut gb = GameBoy::new(rom).unwrap();
        for _ in 0..10 {
            gb.step();
        }
        assert_eq!(gb.take_serial_output(), b"O");
    }

    #[test]
    fn serial_transfer_completes_and_requests_interrupt() {
        let rom = rom_with_program(&[0x3E, 0x12, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, 0x18, 0xFE]);
        let mut gb = GameBoy::new(rom).unwrap();
        // 8 ビット × 128 M サイクル = 1024 M サイクルあれば必ず終わる
        while gb.cycles() < 1100 {
            gb.step();
        }
        assert_eq!(gb.peek(0xFF01), 0xFF);
        assert_eq!(gb.peek(0xFF02) & 0x80, 0);
        assert_ne!(gb.peek(0xFF0F) & interrupts::SERIAL, 0);
    }

    #[test]
    fn run_frame_advances_about_one_frame() {
        let rom = rom_with_program(&[0x18, 0xFE]); // JR -2
        let mut gb = GameBoy::new(rom).unwrap();
        gb.run_frame();
        let start = gb.cycles();
        gb.run_frame();
        let elapsed = gb.cycles() - start;
        assert!((M_CYCLES_PER_FRAME - 4..=M_CYCLES_PER_FRAME + 4).contains(&elapsed));
    }

    #[test]
    fn oam_dma_copies_160_bytes_and_blocks_oam_meanwhile() {
        // HL=0xC000 から 160 バイトを 0xA5 で埋めたあと DMA を起動する
        let rom = rom_with_program(&[
            0x21, 0x00, 0xC0, // LD HL,0xC000
            0x06, 0xA0, // LD B,160
            0x3E, 0xA5, // LD A,0xA5
            0x22, // loop: LD (HL+),A
            0x05, // DEC B
            0x20, 0xFC, // JR NZ,loop
            0x3E, 0xC0, // LD A,0xC0
            0xE0, 0x46, // LDH (46),A
            0x18, 0xFE, // JR -2
        ]);
        let mut gb = GameBoy::new(rom).unwrap();
        // LCD が ON だと OAM がモードでブロックされるので消しておく
        gb.bus.ppu.write_register(0xFF40, 0x00);
        while gb.cpu.pc != 0x010F {
            gb.step();
        }
        gb.step();
        gb.step();
        assert_eq!(gb.peek(0xFE00), 0xFF, "転送中は 0xFF が読める");
        for _ in 0..200 {
            gb.step();
        }
        assert_eq!(gb.peek(0xFE00), 0xA5);
        assert_eq!(gb.peek(0xFE9F), 0xA5);
    }
}
