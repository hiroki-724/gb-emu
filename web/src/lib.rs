//! ブラウザ用フロントエンド（WebAssembly）。
//!
//! JavaScript 側（`web/app.js`）からは [`Emulator`] だけを使う。
//! やり取りするのは ROM のバイト列、ボタン状態、RGBA の画面、セーブデータだけ。

use gb_core::{CartridgeError, GameBoy, FRAMEBUFFER_SIZE};
use wasm_bindgen::prelude::*;

/// 画面 1 枚ぶんの RGBA のバイト数
pub const RGBA_SIZE: usize = FRAMEBUFFER_SIZE * 4;

/// 既定の配色（濃淡 0〜3 → RGB）。明るい順
const DEFAULT_PALETTE: [[u8; 3]; 4] = [
    [0xE8, 0xF0, 0xD8],
    [0xA0, 0xB8, 0x80],
    [0x50, 0x70, 0x50],
    [0x18, 0x28, 0x20],
];

#[wasm_bindgen]
pub struct Emulator {
    gb: GameBoy,
    /// 濃淡 0〜3 に対応する RGBA
    palette: [[u8; 4]; 4],
}

impl Emulator {
    /// ROM からエミュレータを作る（wasm 以外からも呼べる版）
    pub fn create(rom: &[u8]) -> Result<Emulator, CartridgeError> {
        let mut emulator = Emulator {
            gb: GameBoy::new(rom.to_vec())?,
            palette: [[0; 4]; 4],
        };
        emulator.set_palette(DEFAULT_PALETTE.as_flattened());
        Ok(emulator)
    }
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsError> {
        Emulator::create(rom).map_err(|err| JsError::new(&err.to_string()))
    }

    /// 1 フレームぶん進める
    pub fn run_frame(&mut self) {
        self.gb.run_frame();
    }

    /// 現在の画面を RGBA（160×144×4 バイト）で書き出す
    pub fn render(&self, out: &mut [u8]) {
        let (pixels, _) = out.as_chunks_mut::<4>();
        for (pixel, &shade) in pixels.iter_mut().zip(self.gb.framebuffer().iter()) {
            *pixel = self.palette[shade as usize & 3];
        }
    }

    /// 配色を変える。`rgb` は明るい順に 4 色ぶん（12 バイト）
    pub fn set_palette(&mut self, rgb: &[u8]) {
        let (colors, _) = rgb.as_chunks::<3>();
        for (entry, color) in self.palette.iter_mut().zip(colors) {
            *entry = [color[0], color[1], color[2], 0xFF];
        }
    }

    /// 押されているボタンをまとめて設定する。
    /// bit0 右 / bit1 左 / bit2 上 / bit3 下 / bit4 A / bit5 B / bit6 Select / bit7 Start
    pub fn set_buttons(&mut self, pressed: u8) {
        self.gb.set_buttons(pressed);
    }

    /// 音声の出力サンプルレート（Hz）を設定する。0 を渡すと音声の生成を止める。
    /// 再生側のバッファの増減に合わせて、毎フレーム少しずつ変えてよい
    pub fn set_sample_rate(&mut self, hz: f64) {
        self.gb.set_audio_sample_rate(hz);
    }

    /// 溜まっている音声サンプル（左右交互、-1.0〜1.0）を取り出す
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.gb.take_audio_samples()
    }

    /// ROM のヘッダに書かれたタイトル
    pub fn title(&self) -> String {
        self.gb.cartridge().title()
    }

    /// セーブデータの保存先を見分けるためのキー（タイトル + ROM のチェックサム）
    pub fn save_key(&self) -> String {
        let cart = self.gb.cartridge();
        format!("{}:{:04x}", cart.title(), cart.global_checksum())
    }

    /// セーブ機能（バッテリーバックアップ）のあるカートリッジか
    pub fn has_battery(&self) -> bool {
        self.gb.cartridge().has_battery()
    }

    /// セーブデータを取り出す。`unix_time` は現在時刻（UNIX 秒）
    pub fn save_data(&self, unix_time: f64) -> Option<Vec<u8>> {
        self.gb.cartridge().save_data(unix_time.max(0.0) as u64)
    }

    /// セーブデータを読み込む。`unix_time` は現在時刻（UNIX 秒）
    pub fn load_save_data(&mut self, data: &[u8], unix_time: f64) {
        self.gb
            .cartridge_mut()
            .load_save_data(data, unix_time.max(0.0) as u64);
    }

    /// 前回の呼び出し以降にセーブデータが書き換えられたか
    pub fn take_save_dirty(&mut self) -> bool {
        self.gb.cartridge_mut().take_ram_dirty()
    }
}

/// 実機の 1 フレームの長さ（ミリ秒）。約 16.742ms = 59.73fps
pub const FRAME_MS: f64 = 1000.0 / gb_core::FRAMES_PER_SECOND;

/// 1 回の画面更新で進めすぎないための上限（処理落ちからの復帰用）
const MAX_FRAMES_PER_REFRESH: u32 = 3;

/// 画面の更新（requestAnimationFrame）のたびに、エミュレータを何フレーム進めるかを決める。
///
/// - ほぼ 60Hz の画面では、更新 1 回につき 1 フレーム進める。59.73fps との差（0.5% 未満）は
///   体感できず、こうすると「数秒に 1 回同じ絵が 2 度出る」周期的なカクつきが起きない
/// - 90Hz・120Hz などの画面では、経過時間をためて 59.73fps になるように進める
#[wasm_bindgen]
pub struct FramePacer {
    /// 画面の更新間隔の移動平均（ミリ秒）
    refresh_interval: f64,
    /// まだフレームに換算していない経過時間（ミリ秒）
    accumulator: f64,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl FramePacer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> FramePacer {
        FramePacer {
            refresh_interval: FRAME_MS,
            accumulator: 0.0,
        }
    }

    /// 一時停止からの再開時に呼ぶ
    pub fn reset(&mut self) {
        *self = FramePacer::new();
    }

    /// 前回の更新からの経過時間（ミリ秒）を受け取り、今回進めるフレーム数を返す
    pub fn frames_for(&mut self, delta_ms: f64) -> u32 {
        let mut delta = delta_ms;
        if !(0.0..=250.0).contains(&delta) {
            // タブの切り替えなどで長く止まっていた（または時刻が巻き戻った）。
            // 追いつこうとせず仕切り直す
            delta = FRAME_MS;
            self.accumulator = 0.0;
        }
        self.refresh_interval += (delta - self.refresh_interval) * 0.05;

        if (self.refresh_interval - FRAME_MS).abs() < FRAME_MS * 0.025 {
            // 表示が遅れた回は、そのぶん多く進めて速度を保つ
            self.accumulator = 0.0;
            return ((delta / FRAME_MS).round() as u32).clamp(1, MAX_FRAMES_PER_REFRESH);
        }

        self.accumulator += delta;
        let frames = ((self.accumulator / FRAME_MS) as u32).min(MAX_FRAMES_PER_REFRESH);
        self.accumulator -= frames as f64 * FRAME_MS;
        if self.accumulator > FRAME_MS * MAX_FRAMES_PER_REFRESH as f64 {
            self.accumulator = 0.0;
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x102].copy_from_slice(&[0x18, 0xFE]); // JR -2
        rom[0x134..0x138].copy_from_slice(b"TEST");
        rom[0x14E] = 0x12;
        rom[0x14F] = 0x34;
        rom
    }

    #[test]
    fn render_fills_rgba_with_palette() {
        let mut emulator = Emulator::create(&blank_rom()).unwrap();
        emulator.set_palette(&[10, 20, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        emulator.run_frame();
        emulator.run_frame();
        let mut out = vec![0u8; RGBA_SIZE];
        emulator.render(&mut out);
        // VRAM は空なので全面が色 0（BGP=0xFC でも色 0 は濃淡 0）
        assert_eq!(&out[..4], &[10, 20, 30, 255]);
        assert_eq!(&out[RGBA_SIZE - 4..], &[10, 20, 30, 255]);
    }

    #[test]
    fn save_key_combines_title_and_checksum() {
        let emulator = Emulator::create(&blank_rom()).unwrap();
        assert_eq!(emulator.title(), "TEST");
        assert_eq!(emulator.save_key(), "TEST:1234");
        assert!(!emulator.has_battery());
    }

    /// 一定の更新間隔で 10 秒ぶん回したときの (平均 fps, 1 回あたりの最大, 0 フレームだった回数)
    fn simulate(refresh_hz: f64) -> (f64, u32, u32) {
        let delta = 1000.0 / refresh_hz;
        let mut pacer = FramePacer::new();
        for _ in 0..200 {
            pacer.frames_for(delta); // 更新間隔の推定が落ち着くまで
        }
        let refreshes = (refresh_hz * 10.0) as u32;
        let (mut total, mut max, mut zeros) = (0, 0, 0);
        for _ in 0..refreshes {
            let frames = pacer.frames_for(delta);
            total += frames;
            max = max.max(frames);
            zeros += (frames == 0) as u32;
        }
        (total as f64 / 10.0, max, zeros)
    }

    #[test]
    fn pacer_runs_exactly_one_frame_per_refresh_on_60hz() {
        let (fps, max, zeros) = simulate(60.0);
        assert_eq!((max, zeros), (1, 0), "毎回ちょうど 1 フレーム");
        assert!((fps - 60.0).abs() < 0.01);
    }

    #[test]
    fn pacer_keeps_real_speed_on_high_refresh_displays() {
        for hz in [90.0, 120.0, 144.0] {
            let (fps, max, _) = simulate(hz);
            assert!((fps - 59.73).abs() < 0.2, "{hz}Hz で {fps}fps");
            assert_eq!(max, 1);
        }
    }

    #[test]
    fn pacer_catches_up_when_refresh_is_slow() {
        let (fps, max, zeros) = simulate(30.0);
        assert!((fps - 59.73).abs() < 0.2, "30Hz で {fps}fps");
        assert_eq!((max, zeros), (2, 0));
    }

    #[test]
    fn pacer_does_not_fast_forward_after_a_long_pause() {
        let mut pacer = FramePacer::new();
        assert_eq!(pacer.frames_for(5000.0), 1);
        assert_eq!(pacer.frames_for(-10.0), 1);
    }

    #[test]
    fn audio_is_produced_only_after_a_sample_rate_is_set() {
        let mut emulator = Emulator::create(&blank_rom()).unwrap();
        emulator.run_frame();
        assert!(emulator.take_audio().is_empty());
        emulator.set_sample_rate(48_000.0);
        emulator.run_frame();
        // 1 フレーム（約 16.7ms）ぶん ≒ 804 フレーム × 2 チャンネル
        let samples = emulator.take_audio();
        assert!((1500..1700).contains(&samples.len()), "{}", samples.len());
    }

    #[test]
    fn rejects_invalid_rom() {
        assert!(Emulator::create(&[0u8; 16]).is_err());
    }
}
