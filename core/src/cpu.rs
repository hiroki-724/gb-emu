//! SM83 CPU。
//!
//! メモリアクセス 1 回 = 1 M サイクルとして扱い、アクセスのたびにバス側で
//! PPU・タイマーなどの周辺回路を進める（[`CpuBus`] の実装が担当する）。
//! そのため命令ごとの「サイクル数表」は持たず、アクセスの並びがそのままタイミングになる。

/// CPU から見たバス。
///
/// 1 M サイクルは「周辺回路が進む（`tick`）→ その後でメモリにアクセスする（`load` / `store`）」
/// の順で扱う。`read` / `write` はこの 2 つをまとめたもの。
pub trait CpuBus {
    /// 周辺回路を 1 M サイクル進める（メモリアクセスを伴わない内部サイクルにも使う）
    fn tick(&mut self);
    /// 時間を進めずに読む（そのサイクルの `tick` は済んでいる前提）
    fn load(&mut self, addr: u16) -> u8;
    /// 時間を進めずに書く（そのサイクルの `tick` は済んでいる前提）
    fn store(&mut self, addr: u16, value: u8);
    /// 要求が出ていて、かつ許可されている割り込み（IE & IF の下位 5bit）
    fn pending_interrupts(&self) -> u8;
    /// IF の該当ビットを下ろす
    fn clear_interrupt(&mut self, mask: u8);

    /// 1 M サイクル進めてから読む
    #[inline]
    fn read(&mut self, addr: u16) -> u8 {
        self.tick();
        self.load(addr)
    }

    /// 1 M サイクル進めてから書く
    #[inline]
    fn write(&mut self, addr: u16, value: u8) {
        self.tick();
        self.store(addr, value);
    }
}

const FLAG_Z: u8 = 0x80;
const FLAG_N: u8 = 0x40;
const FLAG_H: u8 = 0x20;
const FLAG_C: u8 = 0x10;

#[derive(Clone, Debug)]
pub struct Cpu {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    /// 割り込みマスタ許可フラグ
    pub ime: bool,
    /// EI の直後。次の 1 命令を実行してから IME が立つ
    ei_delay: bool,
    pub halted: bool,
    /// HALT バグ: 次のオペコードフェッチで PC が進まない
    halt_bug: bool,
    /// 未定義オペコードを実行して固まった状態（実機でもリセットまで復帰しない）
    pub locked: bool,
    /// `LD B,B`（0x40）を実行したら立つ。テストROMが終了の合図として使う「ソフトウェアブレークポイント」
    pub breakpoint_hit: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// DMG のブートROM終了直後（PC=0x0100）のレジスタ状態で作る
    pub fn new() -> Self {
        Cpu {
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100,
            ime: false,
            ei_delay: false,
            halted: false,
            halt_bug: false,
            locked: false,
            breakpoint_hit: false,
        }
    }

    #[inline]
    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }
    #[inline]
    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }
    #[inline]
    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }
    #[inline]
    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }
    #[inline]
    fn set_bc(&mut self, v: u16) {
        [self.b, self.c] = v.to_be_bytes();
    }
    #[inline]
    fn set_de(&mut self, v: u16) {
        [self.d, self.e] = v.to_be_bytes();
    }
    #[inline]
    fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }

    #[inline]
    fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }

    #[inline]
    fn set_flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = (if z { FLAG_Z } else { 0 })
            | (if n { FLAG_N } else { 0 })
            | (if h { FLAG_H } else { 0 })
            | (if c { FLAG_C } else { 0 });
    }

    /// 1 命令（または割り込み受け付け・HALT 中の 1 サイクル）を実行する。
    ///
    /// 割り込みを受け付けるかどうかは、次の命令のフェッチサイクルの中（周辺回路がそのサイクルぶん
    /// 進んだ後）で決まる。HALT 中も毎サイクル同じ判定をしていて、NOP を並べて待つのと
    /// 同じタイミングで復帰する。
    pub fn step<B: CpuBus>(&mut self, bus: &mut B) {
        // フェッチサイクルの時間を進める
        bus.tick();
        if self.locked {
            return;
        }

        let pending = bus.pending_interrupts();
        if self.halted {
            // HALT は IE & IF が立つと IME に関係なく解除される
            if pending == 0 {
                return;
            }
            self.halted = false;
        }

        if self.ime && pending != 0 {
            self.dispatch_interrupt(bus);
            return;
        }

        // EI の効果は「次の命令の実行後」に現れる。ここで立てておくと、
        // 次の step の割り込み判定から有効になる（直後の命令が DI ならそこで取り消される）
        if self.ei_delay {
            self.ei_delay = false;
            self.ime = true;
        }

        let opcode = self.fetch_opcode(bus);
        self.execute(bus, opcode);
    }

    /// 割り込み受け付け（5 M サイクル）。
    /// 1 サイクル目は「読んだオペコードを捨てる」フェッチサイクルで、`step` 側ですでに進めてある
    fn dispatch_interrupt<B: CpuBus>(&mut self, bus: &mut B) {
        self.ime = false;
        bus.tick();
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, (self.pc >> 8) as u8);
        // PC 上位バイトの push が IE（0xFFFF）を書き換えることがあるため、
        // どのベクタに飛ぶかは上位バイトを書いた後で決まる。該当なしになったら 0x0000 へ飛ぶ
        let pending = bus.pending_interrupts();
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, self.pc as u8);
        if pending != 0 {
            let bit = pending.trailing_zeros() as u16;
            bus.clear_interrupt(1 << bit);
            self.pc = 0x0040 + bit * 8;
        } else {
            self.pc = 0x0000;
        }
        bus.tick();
    }

    /// オペコードを読む。このサイクルの時間は `step` の先頭ですでに進めてある
    #[inline]
    fn fetch_opcode<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.load(self.pc);
        if self.halt_bug {
            // HALT バグ: HALT 直後の 1 バイトは PC が進まず、2 回読まれる
            self.halt_bug = false;
        } else {
            self.pc = self.pc.wrapping_add(1);
        }
        v
    }

    /// 命令のオペランド（即値）を 1 バイト読む
    #[inline]
    fn fetch8<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    #[inline]
    fn fetch16<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch8(bus);
        let hi = self.fetch8(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// 8bit レジスタ表: 0=B 1=C 2=D 3=E 4=H 5=L 6=(HL) 7=A
    #[inline]
    fn r8<B: CpuBus>(&mut self, bus: &mut B, idx: u8) -> u8 {
        match idx {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => bus.read(self.hl()),
            _ => self.a,
        }
    }

    #[inline]
    fn set_r8<B: CpuBus>(&mut self, bus: &mut B, idx: u8, v: u8) {
        match idx {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            6 => bus.write(self.hl(), v),
            _ => self.a = v,
        }
    }

    /// 16bit レジスタ表（SP 版）: 0=BC 1=DE 2=HL 3=SP
    #[inline]
    fn rp(&self, idx: u8) -> u16 {
        match idx {
            0 => self.bc(),
            1 => self.de(),
            2 => self.hl(),
            _ => self.sp,
        }
    }

    #[inline]
    fn set_rp(&mut self, idx: u8, v: u16) {
        match idx {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_hl(v),
            _ => self.sp = v,
        }
    }

    /// 条件表: 0=NZ 1=Z 2=NC 3=C
    #[inline]
    fn cond(&self, idx: u8) -> bool {
        match idx {
            0 => !self.flag(FLAG_Z),
            1 => self.flag(FLAG_Z),
            2 => !self.flag(FLAG_C),
            _ => self.flag(FLAG_C),
        }
    }

    fn push16<B: CpuBus>(&mut self, bus: &mut B, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        bus.write(self.sp, v as u8);
    }

    fn pop16<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = bus.read(self.sp);
        self.sp = self.sp.wrapping_add(1);
        let hi = bus.read(self.sp);
        self.sp = self.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }

    /// 算術論理演算: 0=ADD 1=ADC 2=SUB 3=SBC 4=AND 5=XOR 6=OR 7=CP
    fn alu(&mut self, op: u8, v: u8) {
        let a = self.a;
        match op {
            0 | 1 => {
                let carry = (op == 1 && self.flag(FLAG_C)) as u8;
                let sum = a as u16 + v as u16 + carry as u16;
                let half = (a & 0x0F) + (v & 0x0F) + carry > 0x0F;
                self.a = sum as u8;
                self.set_flags(self.a == 0, false, half, sum > 0xFF);
            }
            2 | 3 | 7 => {
                let carry = (op == 3 && self.flag(FLAG_C)) as u8;
                let diff = (a as i16) - (v as i16) - (carry as i16);
                let half = (a & 0x0F) < (v & 0x0F) + carry;
                let result = diff as u8;
                self.set_flags(result == 0, true, half, diff < 0);
                if op != 7 {
                    self.a = result;
                }
            }
            4 => {
                self.a = a & v;
                self.set_flags(self.a == 0, false, true, false);
            }
            5 => {
                self.a = a ^ v;
                self.set_flags(self.a == 0, false, false, false);
            }
            _ => {
                self.a = a | v;
                self.set_flags(self.a == 0, false, false, false);
            }
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.f = (self.f & FLAG_C)
            | (if r == 0 { FLAG_Z } else { 0 })
            | (if v & 0x0F == 0x0F { FLAG_H } else { 0 });
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.f = (self.f & FLAG_C)
            | FLAG_N
            | (if r == 0 { FLAG_Z } else { 0 })
            | (if v & 0x0F == 0x00 { FLAG_H } else { 0 });
        r
    }

    fn add_hl(&mut self, v: u16) {
        let hl = self.hl();
        let sum = hl as u32 + v as u32;
        let half = (hl & 0x0FFF) + (v & 0x0FFF) > 0x0FFF;
        self.set_hl(sum as u16);
        self.f = (self.f & FLAG_Z)
            | (if half { FLAG_H } else { 0 })
            | (if sum > 0xFFFF { FLAG_C } else { 0 });
    }

    /// SP + 符号付き 8bit。フラグは下位バイトの符号なし加算として決まる
    fn add_sp_offset(&mut self, offset: u8) -> u16 {
        let sp = self.sp;
        let half = (sp & 0x000F) + (offset as u16 & 0x000F) > 0x000F;
        let carry = (sp & 0x00FF) + (offset as u16) > 0x00FF;
        self.set_flags(false, false, half, carry);
        sp.wrapping_add(offset as i8 as i16 as u16)
    }

    fn daa(&mut self) {
        let mut a = self.a;
        let mut carry = self.flag(FLAG_C);
        if !self.flag(FLAG_N) {
            if carry || a > 0x99 {
                a = a.wrapping_add(0x60);
                carry = true;
            }
            if self.flag(FLAG_H) || (a & 0x0F) > 0x09 {
                a = a.wrapping_add(0x06);
            }
        } else {
            if carry {
                a = a.wrapping_sub(0x60);
            }
            if self.flag(FLAG_H) {
                a = a.wrapping_sub(0x06);
            }
        }
        self.a = a;
        self.f = (self.f & FLAG_N)
            | (if a == 0 { FLAG_Z } else { 0 })
            | (if carry { FLAG_C } else { 0 });
    }

    /// CB プレフィックスの回転・シフト: 0=RLC 1=RRC 2=RL 3=RR 4=SLA 5=SRA 6=SWAP 7=SRL
    fn rotate_shift(&mut self, op: u8, v: u8) -> u8 {
        let carry_in = self.flag(FLAG_C) as u8;
        let (r, carry) = match op {
            0 => (v.rotate_left(1), v & 0x80 != 0),
            1 => (v.rotate_right(1), v & 0x01 != 0),
            2 => ((v << 1) | carry_in, v & 0x80 != 0),
            3 => ((v >> 1) | (carry_in << 7), v & 0x01 != 0),
            4 => (v << 1, v & 0x80 != 0),
            5 => ((v >> 1) | (v & 0x80), v & 0x01 != 0),
            6 => (v.rotate_left(4), false),
            _ => (v >> 1, v & 0x01 != 0),
        };
        self.set_flags(r == 0, false, false, carry);
        r
    }

    fn execute<B: CpuBus>(&mut self, bus: &mut B, opcode: u8) {
        let x = opcode >> 6;
        let y = (opcode >> 3) & 0x07;
        let z = opcode & 0x07;
        let p = y >> 1;
        let q = y & 0x01;

        match x {
            0 => match z {
                0 => match y {
                    0 => {} // NOP
                    1 => {
                        // LD (nn),SP
                        let addr = self.fetch16(bus);
                        bus.write(addr, self.sp as u8);
                        bus.write(addr.wrapping_add(1), (self.sp >> 8) as u8);
                    }
                    2 => {
                        // STOP: DMG ではボタン入力待ちの低消費電力モード。
                        // 2 バイト命令として読み飛ばすだけにしている（市販ソフトではほぼ使われない）
                        let _ = self.fetch8(bus);
                    }
                    3 => {
                        // JR e
                        let offset = self.fetch8(bus) as i8;
                        bus.tick();
                        self.pc = self.pc.wrapping_add(offset as i16 as u16);
                    }
                    _ => {
                        // JR cc,e
                        let offset = self.fetch8(bus) as i8;
                        if self.cond(y - 4) {
                            bus.tick();
                            self.pc = self.pc.wrapping_add(offset as i16 as u16);
                        }
                    }
                },
                1 => {
                    if q == 0 {
                        // LD rr,nn
                        let v = self.fetch16(bus);
                        self.set_rp(p, v);
                    } else {
                        // ADD HL,rr
                        bus.tick();
                        self.add_hl(self.rp(p));
                    }
                }
                2 => {
                    let addr = match p {
                        0 => self.bc(),
                        1 => self.de(),
                        _ => self.hl(),
                    };
                    if q == 0 {
                        bus.write(addr, self.a);
                    } else {
                        self.a = bus.read(addr);
                    }
                    match p {
                        2 => self.set_hl(addr.wrapping_add(1)),
                        3 => self.set_hl(addr.wrapping_sub(1)),
                        _ => {}
                    }
                }
                3 => {
                    // INC rr / DEC rr
                    bus.tick();
                    let v = self.rp(p);
                    self.set_rp(
                        p,
                        if q == 0 {
                            v.wrapping_add(1)
                        } else {
                            v.wrapping_sub(1)
                        },
                    );
                }
                4 => {
                    let v = self.r8(bus, y);
                    let r = self.inc8(v);
                    self.set_r8(bus, y, r);
                }
                5 => {
                    let v = self.r8(bus, y);
                    let r = self.dec8(v);
                    self.set_r8(bus, y, r);
                }
                6 => {
                    // LD r,n
                    let v = self.fetch8(bus);
                    self.set_r8(bus, y, v);
                }
                _ => match y {
                    0 => {
                        // RLCA
                        self.a = self.a.rotate_left(1);
                        self.set_flags(false, false, false, self.a & 0x01 != 0);
                    }
                    1 => {
                        // RRCA
                        self.a = self.a.rotate_right(1);
                        self.set_flags(false, false, false, self.a & 0x80 != 0);
                    }
                    2 => {
                        // RLA
                        let carry = self.a & 0x80 != 0;
                        self.a = (self.a << 1) | self.flag(FLAG_C) as u8;
                        self.set_flags(false, false, false, carry);
                    }
                    3 => {
                        // RRA
                        let carry = self.a & 0x01 != 0;
                        self.a = (self.a >> 1) | ((self.flag(FLAG_C) as u8) << 7);
                        self.set_flags(false, false, false, carry);
                    }
                    4 => self.daa(),
                    5 => {
                        // CPL
                        self.a = !self.a;
                        self.f |= FLAG_N | FLAG_H;
                    }
                    6 => {
                        // SCF
                        self.f = (self.f & FLAG_Z) | FLAG_C;
                    }
                    _ => {
                        // CCF
                        self.f = (self.f & (FLAG_Z | FLAG_C)) ^ FLAG_C;
                    }
                },
            },
            1 => {
                if opcode == 0x76 {
                    self.halt(bus);
                } else {
                    // LD r,r'
                    let v = self.r8(bus, z);
                    self.set_r8(bus, y, v);
                    if opcode == 0x40 {
                        self.breakpoint_hit = true;
                    }
                }
            }
            2 => {
                let v = self.r8(bus, z);
                self.alu(y, v);
            }
            _ => match z {
                0 => match y {
                    0..=3 => {
                        // RET cc
                        bus.tick();
                        if self.cond(y) {
                            self.pc = self.pop16(bus);
                            bus.tick();
                        }
                    }
                    4 => {
                        // LDH (n),A
                        let n = self.fetch8(bus);
                        bus.write(0xFF00 | n as u16, self.a);
                    }
                    5 => {
                        // ADD SP,e
                        let offset = self.fetch8(bus);
                        bus.tick();
                        bus.tick();
                        self.sp = self.add_sp_offset(offset);
                    }
                    6 => {
                        // LDH A,(n)
                        let n = self.fetch8(bus);
                        self.a = bus.read(0xFF00 | n as u16);
                    }
                    _ => {
                        // LD HL,SP+e
                        let offset = self.fetch8(bus);
                        bus.tick();
                        let v = self.add_sp_offset(offset);
                        self.set_hl(v);
                    }
                },
                1 => {
                    if q == 0 {
                        // POP rr
                        let v = self.pop16(bus);
                        match p {
                            0 => self.set_bc(v),
                            1 => self.set_de(v),
                            2 => self.set_hl(v),
                            _ => {
                                self.a = (v >> 8) as u8;
                                self.f = v as u8 & 0xF0;
                            }
                        }
                    } else {
                        match p {
                            0 => {
                                // RET
                                self.pc = self.pop16(bus);
                                bus.tick();
                            }
                            1 => {
                                // RETI（EI と違い、IME は遅延なしで立つ）
                                self.pc = self.pop16(bus);
                                bus.tick();
                                self.ime = true;
                            }
                            2 => self.pc = self.hl(), // JP HL
                            _ => {
                                // LD SP,HL
                                bus.tick();
                                self.sp = self.hl();
                            }
                        }
                    }
                }
                2 => match y {
                    0..=3 => {
                        // JP cc,nn
                        let addr = self.fetch16(bus);
                        if self.cond(y) {
                            bus.tick();
                            self.pc = addr;
                        }
                    }
                    4 => bus.write(0xFF00 | self.c as u16, self.a), // LD (C),A
                    5 => {
                        // LD (nn),A
                        let addr = self.fetch16(bus);
                        bus.write(addr, self.a);
                    }
                    6 => self.a = bus.read(0xFF00 | self.c as u16), // LD A,(C)
                    _ => {
                        // LD A,(nn)
                        let addr = self.fetch16(bus);
                        self.a = bus.read(addr);
                    }
                },
                3 => match y {
                    0 => {
                        // JP nn
                        let addr = self.fetch16(bus);
                        bus.tick();
                        self.pc = addr;
                    }
                    1 => self.execute_cb(bus),
                    6 => {
                        // DI: 即座に効く。直前の EI の遅延も取り消す
                        self.ime = false;
                        self.ei_delay = false;
                    }
                    7 => {
                        // EI: 次の命令を実行した後に IME が立つ
                        if !self.ime {
                            self.ei_delay = true;
                        }
                    }
                    _ => self.locked = true, // 0xD3 0xDB 0xE3 0xEB: 未定義
                },
                4 => match y {
                    0..=3 => {
                        // CALL cc,nn
                        let addr = self.fetch16(bus);
                        if self.cond(y) {
                            bus.tick();
                            self.push16(bus, self.pc);
                            self.pc = addr;
                        }
                    }
                    _ => self.locked = true, // 0xE4 0xEC 0xF4 0xFC: 未定義
                },
                5 => {
                    if q == 0 {
                        // PUSH rr
                        let v = match p {
                            0 => self.bc(),
                            1 => self.de(),
                            2 => self.hl(),
                            _ => self.af(),
                        };
                        bus.tick();
                        self.push16(bus, v);
                    } else if p == 0 {
                        // CALL nn
                        let addr = self.fetch16(bus);
                        bus.tick();
                        self.push16(bus, self.pc);
                        self.pc = addr;
                    } else {
                        self.locked = true; // 0xDD 0xED 0xFD: 未定義
                    }
                }
                6 => {
                    // ALU A,n
                    let v = self.fetch8(bus);
                    self.alu(y, v);
                }
                _ => {
                    // RST
                    bus.tick();
                    self.push16(bus, self.pc);
                    self.pc = (y as u16) * 8;
                }
            },
        }
    }

    fn execute_cb<B: CpuBus>(&mut self, bus: &mut B) {
        let opcode = self.fetch8(bus);
        let y = (opcode >> 3) & 0x07;
        let z = opcode & 0x07;
        let v = self.r8(bus, z);
        match opcode >> 6 {
            0 => {
                let r = self.rotate_shift(y, v);
                self.set_r8(bus, z, r);
            }
            1 => {
                // BIT y,r（書き戻さない）
                self.f = (self.f & FLAG_C) | FLAG_H | (if v & (1 << y) == 0 { FLAG_Z } else { 0 });
            }
            2 => self.set_r8(bus, z, v & !(1 << y)), // RES
            _ => self.set_r8(bus, z, v | (1 << y)),  // SET
        }
    }

    fn halt<B: CpuBus>(&mut self, bus: &mut B) {
        if !self.ime && bus.pending_interrupts() != 0 {
            // HALT バグ: IME=0 で割り込みがすでに保留中だと HALT に入らず、
            // 次のオペコードの読み出しで PC が進まない
            self.halt_bug = true;
        } else {
            // IME=1 で保留中の場合も一度 HALT に入るが、次のサイクルですぐ起きて割り込みに入る
            self.halted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64KB のフラットなメモリだけを持つテスト用バス
    struct FlatBus {
        mem: Vec<u8>,
        cycles: u64,
    }

    impl FlatBus {
        fn with_program(program: &[u8]) -> Self {
            let mut mem = vec![0u8; 0x10000];
            mem[0x0100..0x0100 + program.len()].copy_from_slice(program);
            FlatBus { mem, cycles: 0 }
        }
    }

    impl CpuBus for FlatBus {
        fn tick(&mut self) {
            self.cycles += 1;
        }
        fn load(&mut self, addr: u16) -> u8 {
            self.mem[addr as usize]
        }
        fn store(&mut self, addr: u16, value: u8) {
            self.mem[addr as usize] = value;
        }
        fn pending_interrupts(&self) -> u8 {
            self.mem[0xFFFF] & self.mem[0xFF0F] & 0x1F
        }
        fn clear_interrupt(&mut self, mask: u8) {
            self.mem[0xFF0F] &= !mask;
        }
    }

    fn run(program: &[u8], steps: usize) -> (Cpu, FlatBus) {
        let mut bus = FlatBus::with_program(program);
        let mut cpu = Cpu::new();
        for _ in 0..steps {
            cpu.step(&mut bus);
        }
        (cpu, bus)
    }

    #[test]
    fn add_sets_half_carry_and_carry() {
        // LD A,0x8F / ADD A,0x81
        let (cpu, _) = run(&[0x3E, 0x8F, 0xC6, 0x81], 2);
        assert_eq!(cpu.a, 0x10);
        assert_eq!(cpu.f, FLAG_H | FLAG_C);
    }

    #[test]
    fn sub_to_zero_sets_z_and_n() {
        // LD A,0x42 / SUB 0x42
        let (cpu, _) = run(&[0x3E, 0x42, 0xD6, 0x42], 2);
        assert_eq!(cpu.a, 0x00);
        assert_eq!(cpu.f, FLAG_Z | FLAG_N);
    }

    #[test]
    fn daa_after_bcd_add() {
        // LD A,0x45 / ADD A,0x38 / DAA → 0x83
        let (cpu, _) = run(&[0x3E, 0x45, 0xC6, 0x38, 0x27], 3);
        assert_eq!(cpu.a, 0x83);
        assert_eq!(cpu.f & FLAG_C, 0);
    }

    #[test]
    fn pop_af_masks_low_nibble() {
        // LD BC,0x12FF / PUSH BC / POP AF
        let (cpu, _) = run(&[0x01, 0xFF, 0x12, 0xC5, 0xF1], 3);
        assert_eq!(cpu.a, 0x12);
        assert_eq!(cpu.f, 0xF0);
    }

    #[test]
    fn instruction_cycle_counts() {
        // (プログラム, 実行する命令数, 期待 M サイクル数)
        let cases: &[(&[u8], usize, u64)] = &[
            (&[0x00], 1, 1),             // NOP
            (&[0x01, 0x34, 0x12], 1, 3), // LD BC,nn
            (&[0x08, 0x00, 0xC0], 1, 5), // LD (nn),SP
            (&[0x09], 1, 2),             // ADD HL,BC
            (&[0x18, 0x00], 1, 3),       // JR e
            (&[0x34], 1, 3),             // INC (HL)
            (&[0x36, 0x12], 1, 3),       // LD (HL),n
            (&[0xC3, 0x00, 0x02], 1, 4), // JP nn
            (&[0xC5], 1, 4),             // PUSH BC
            (&[0xC1], 1, 3),             // POP BC
            (&[0xCD, 0x00, 0x02], 1, 6), // CALL nn
            (&[0xC9], 1, 4),             // RET
            (&[0xC7], 1, 4),             // RST 00
            (&[0xE8, 0x01], 1, 4),       // ADD SP,e
            (&[0xF8, 0x01], 1, 3),       // LD HL,SP+e
            (&[0xF9], 1, 2),             // LD SP,HL
            (&[0xCB, 0x00], 1, 2),       // RLC B
            (&[0xCB, 0x06], 1, 4),       // RLC (HL)
            (&[0xCB, 0x46], 1, 3),       // BIT 0,(HL)
            (&[0xEA, 0x00, 0xC0], 1, 4), // LD (nn),A
            (&[0xF0, 0x80], 1, 3),       // LDH A,(n)
        ];
        for (program, steps, expected) in cases {
            let (_, bus) = run(program, *steps);
            assert_eq!(bus.cycles, *expected, "opcode {:#04x}", program[0]);
        }
    }

    #[test]
    fn conditional_branches_take_extra_cycles_only_when_taken() {
        // 初期状態は Z=1（F=0xB0）
        let (_, bus) = run(&[0x28, 0x00], 1); // JR Z（成立）
        assert_eq!(bus.cycles, 3);
        let (_, bus) = run(&[0x20, 0x00], 1); // JR NZ（不成立）
        assert_eq!(bus.cycles, 2);
        let (_, bus) = run(&[0xC8], 1); // RET Z（成立）
        assert_eq!(bus.cycles, 5);
        let (_, bus) = run(&[0xC0], 1); // RET NZ（不成立）
        assert_eq!(bus.cycles, 2);
        let (_, bus) = run(&[0xCC, 0x00, 0x02], 1); // CALL Z（成立）
        assert_eq!(bus.cycles, 6);
        let (_, bus) = run(&[0xC4, 0x00, 0x02], 1); // CALL NZ（不成立）
        assert_eq!(bus.cycles, 3);
    }

    #[test]
    fn ei_takes_effect_after_one_instruction() {
        // EI / NOP / NOP、VBlank 割り込みを保留にしておく
        let mut bus = FlatBus::with_program(&[0xFB, 0x00, 0x00]);
        bus.mem[0xFFFF] = 0x01;
        bus.mem[0xFF0F] = 0x01;
        let mut cpu = Cpu::new();
        cpu.step(&mut bus); // EI
        assert!(!cpu.ime);
        cpu.step(&mut bus); // NOP（ここまでは割り込まれない）
        assert_eq!(cpu.pc, 0x0102);
        cpu.step(&mut bus); // 割り込み受け付け
        assert_eq!(cpu.pc, 0x0040);
        assert!(!cpu.ime);
        assert_eq!(bus.mem[0xFF0F] & 0x01, 0);
    }

    #[test]
    fn di_cancels_pending_ei() {
        let mut bus = FlatBus::with_program(&[0xFB, 0xF3, 0x00, 0x00]);
        bus.mem[0xFFFF] = 0x01;
        bus.mem[0xFF0F] = 0x01;
        let mut cpu = Cpu::new();
        for _ in 0..4 {
            cpu.step(&mut bus);
        }
        assert_eq!(cpu.pc, 0x0104);
        assert!(!cpu.ime);
    }

    #[test]
    fn halt_bug_reads_next_byte_twice() {
        // HALT / INC A（IME=0 で割り込み保留中 → INC A が 2 回実行される）
        let mut bus = FlatBus::with_program(&[0x76, 0x3C, 0x00]);
        bus.mem[0xFFFF] = 0x01;
        bus.mem[0xFF0F] = 0x01;
        let mut cpu = Cpu::new();
        cpu.a = 0;
        for _ in 0..3 {
            cpu.step(&mut bus);
        }
        assert_eq!(cpu.a, 2);
        assert_eq!(cpu.pc, 0x0102);
    }

    #[test]
    fn undefined_opcode_locks_cpu() {
        let (cpu, _) = run(&[0xD3, 0x00], 3);
        assert!(cpu.locked);
        assert_eq!(cpu.pc, 0x0101);
    }
}
