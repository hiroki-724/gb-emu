//! メモリバス。CPU からのアクセスを各デバイスに振り分け、
//! アクセス 1 回（= 1 M サイクル）ごとに周辺回路を進める。

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::cpu::CpuBus;
use crate::interrupts;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

/// シリアルクロック（8192 Hz）になる内部カウンタのビット（M サイクル単位で bit6）
const SERIAL_CLOCK_BIT: u16 = 1 << 6;
/// APU のフレームシーケンサ（512 Hz）を進める内部カウンタのビット（M サイクル単位で bit10 = DIV の bit4）
const APU_FRAME_BIT: u16 = 1 << 10;

/// OAM DMA（0xFF46）の状態
#[derive(Clone, Debug, Default)]
struct Dma {
    /// 0xFF46 に最後に書かれた値
    register: u8,
    /// 書き込まれた直後（次のサイクルは準備期間）
    requested: Option<u8>,
    /// 準備期間が終わり、次のサイクルから転送が始まる
    starting: Option<u8>,
    active: bool,
    source: u16,
    /// 転送済みバイト数（0〜160）
    index: u16,
}

#[derive(Clone)]
pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub timer: Timer,
    pub serial: Serial,
    pub joypad: Joypad,
    pub apu: Apu,
    wram: Box<[u8; 0x2000]>,
    hram: [u8; 0x7F],
    /// IF（割り込み要求、下位 5bit）
    intf: u8,
    /// CPU が割り込み判定に使う IF。M サイクルの最初のドットの直後の状態
    intf_sampled: u8,
    /// IE（割り込み許可）
    inte: u8,
    dma: Dma,
    /// 起動からの経過 M サイクル数
    cycles: u64,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Bus {
            cart,
            ppu: Ppu::new(),
            timer: Timer::new(),
            serial: Serial::new(),
            joypad: Joypad::new(),
            apu: Apu::new(),
            wram: Box::new([0; 0x2000]),
            hram: [0; 0x7F],
            intf: interrupts::VBLANK,
            intf_sampled: interrupts::VBLANK,
            inte: 0,
            dma: Dma {
                register: 0xFF,
                ..Dma::default()
            },
            cycles: 0,
        }
    }

    #[inline]
    pub fn cycles(&self) -> u64 {
        self.cycles
    }

    pub fn request_interrupt(&mut self, mask: u8) {
        self.intf |= mask & 0x1F;
    }

    /// 周辺回路を 1 M サイクル進める
    fn advance(&mut self) {
        self.cycles += 1;

        let counter_before = self.timer.counter();
        if self.timer.tick() {
            self.intf |= interrupts::TIMER;
        }
        self.clock_from_divider(counter_before);
        self.apu.tick();

        self.tick_dma();
        self.cart.tick();

        // CPU が割り込みを受け付けるかどうかは、サイクルの最初のドットの直後の状態で決まる。
        // 残りの 3 ドットで PPU が出した要求は IF には立つが、判定に使われるのは次のサイクルから
        let (early, late) = self.ppu.tick();
        self.intf |= early;
        self.intf_sampled = self.intf;
        self.intf |= late;
    }

    /// 内部カウンタのビットの立ち下がりを時計にしている回路（シリアルと APU）を進める
    fn clock_from_divider(&mut self, counter_before: u16) {
        let falling = counter_before & !self.timer.counter();
        if falling & SERIAL_CLOCK_BIT != 0 && self.serial.clock() {
            self.intf |= interrupts::SERIAL;
        }
        if falling & APU_FRAME_BIT != 0 {
            self.apu.frame_sequencer_clock();
        }
    }

    fn tick_dma(&mut self) {
        if let Some(page) = self.dma.starting.take() {
            // 転送開始。実行中の DMA があれば、ここで新しいものに切り替わる
            self.dma.active = true;
            self.dma.source = (page as u16) << 8;
            self.dma.index = 0;
        } else if self.dma.active && self.dma.index >= 160 {
            self.dma.active = false;
        }
        if let Some(page) = self.dma.requested.take() {
            self.dma.starting = Some(page);
        }
        if self.dma.active {
            let value = self.dma_source_read(self.dma.source + self.dma.index);
            self.ppu.dma_write_oam(self.dma.index as usize, value);
            self.dma.index += 1;
        }
    }

    /// DMA の転送元を読む。0xE000 以降を指定した場合は WRAM のミラーが読まれる
    fn dma_source_read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.peek_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            _ => self.wram[addr as usize & 0x1FFF],
        }
    }

    /// 時間を進めずに読む（デバッガ・テスト判定用）
    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xFDFF => self.wram[addr as usize & 0x1FFF],
            0xFE00..=0xFE9F => {
                if self.dma.active {
                    0xFF // DMA 中は CPU から OAM が見えない
                } else {
                    self.ppu.read_oam(addr)
                }
            }
            0xFEA0..=0xFEFF => 0x00, // 使用禁止領域（DMG では 0 が読める）
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[addr as usize - 0xFF80],
            0xFFFF => self.inte,
        }
    }

    fn poke(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_rom(addr, value),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, value),
            0xA000..=0xBFFF => self.cart.write_ram(addr, value),
            0xC000..=0xFDFF => self.wram[addr as usize & 0x1FFF] = value,
            0xFE00..=0xFE9F => {
                if !self.dma.active {
                    self.ppu.write_oam(addr, value);
                }
            }
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => self.write_io(addr, value),
            0xFF80..=0xFFFE => self.hram[addr as usize - 0xFF80] = value,
            0xFFFF => self.inte = value,
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => self.joypad.read(),
            0xFF01 | 0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => self.intf | 0xE0,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF46 => self.dma.register,
            0xFF40..=0xFF4B => self.ppu.read_register(addr),
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF00 => {
                if self.joypad.write(value) {
                    self.intf |= interrupts::JOYPAD;
                }
            }
            0xFF01 | 0xFF02 => self.serial.write(addr, value),
            0xFF04..=0xFF07 => {
                let counter_before = self.timer.counter();
                self.timer.write(addr, value);
                // DIV への書き込みで内部カウンタが 0 になると、シリアルと APU の時計にも立ち下がりが起きる
                self.clock_from_divider(counter_before);
            }
            0xFF0F => self.intf = value & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, value),
            0xFF46 => {
                self.dma.register = value;
                self.dma.requested = Some(value);
            }
            0xFF40..=0xFF4B => self.intf |= self.ppu.write_register(addr, value),
            _ => {}
        }
    }
}

impl CpuBus for Bus {
    #[inline]
    fn tick(&mut self) {
        self.advance();
    }

    #[inline]
    fn load(&mut self, addr: u16) -> u8 {
        self.peek(addr)
    }

    #[inline]
    fn store(&mut self, addr: u16, value: u8) {
        self.poke(addr, value);
    }

    #[inline]
    fn pending_interrupts(&self) -> u8 {
        self.intf & self.inte & 0x1F
    }

    #[inline]
    fn sampled_interrupts(&self) -> u8 {
        self.intf_sampled & self.inte & 0x1F
    }

    #[inline]
    fn clear_interrupt(&mut self, mask: u8) {
        self.intf &= !mask;
    }
}
