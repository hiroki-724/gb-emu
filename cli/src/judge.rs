//! テストROMの合否判定。
//!
//! 判定方法はそれぞれのテストROMが定めている手順に従う:
//! - Blargg: シリアル出力の `Passed` / `Failed`、または 0xA000 以降に書かれる結果
//! - mooneye: `LD B,B` 実行時のレジスタ（3,5,8,13,21,34 でパス）
//! - dmg-acid2: `LD B,B` 実行後の画面をリファレンス画像と比較

use std::path::Path;

use gb_core::{GameBoy, FRAMES_PER_SECOND};

use crate::image;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// 失敗（理由つき）
    Fail(String),
}

impl Outcome {
    pub fn passed(&self) -> bool {
        matches!(self, Outcome::Pass)
    }
}

fn frames_for_seconds(seconds: u64) -> u64 {
    (seconds as f64 * FRAMES_PER_SECOND).ceil() as u64
}

fn load(rom: &[u8]) -> Result<GameBoy, Outcome> {
    GameBoy::new(rom.to_vec()).map_err(|err| Outcome::Fail(format!("起動できません: {err}")))
}

/// Blargg のテストROM。`timeout_seconds` はエミュレータ内の時間
pub fn run_blargg(rom: &[u8], timeout_seconds: u64) -> Outcome {
    let mut gb = match load(rom) {
        Ok(gb) => gb,
        Err(outcome) => return outcome,
    };
    let mut serial = String::new();
    for _ in 0..frames_for_seconds(timeout_seconds) {
        gb.run_frame();

        let output = gb.take_serial_output();
        if !output.is_empty() {
            serial.push_str(&String::from_utf8_lossy(&output));
            if serial.contains("Passed") {
                return Outcome::Pass;
            }
            if serial.contains("Failed") {
                return Outcome::Fail(last_lines(&serial));
            }
        }

        // シリアルを使わない ROM は結果をカートリッジ RAM に書く:
        //   0xA000: 状態（0x80 = 実行中、それ以外 = 結果コード。0 が成功）
        //   0xA001〜0xA003: 目印 0xDE 0xB0 0x61
        //   0xA004〜: 結果の文字列（0 終端）
        let signature = [gb.peek(0xA001), gb.peek(0xA002), gb.peek(0xA003)];
        if signature == [0xDE, 0xB0, 0x61] {
            let status = gb.peek(0xA000);
            if status == 0x00 {
                return Outcome::Pass;
            }
            if status != 0x80 {
                let mut text = String::new();
                for addr in 0xA004..0xB000u16 {
                    let byte = gb.peek(addr);
                    if byte == 0 {
                        break;
                    }
                    text.push(byte as char);
                }
                return Outcome::Fail(format!("結果コード {status}: {}", last_lines(&text)));
            }
        }
    }
    if serial.trim().is_empty() {
        Outcome::Fail("タイムアウト（出力なし）".to_string())
    } else {
        Outcome::Fail(format!("タイムアウト: {}", last_lines(&serial)))
    }
}

/// 出力の末尾を 1 行にまとめる（表に載せるため）
fn last_lines(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(3);
    lines[start..].join(" / ")
}

/// mooneye-test-suite のテストROM
pub fn run_mooneye(rom: &[u8], timeout_seconds: u64) -> Outcome {
    let mut gb = match load(rom) {
        Ok(gb) => gb,
        Err(outcome) => return outcome,
    };
    let limit = timeout_seconds * gb_core::CLOCK_HZ as u64 / 4;
    while gb.cycles() < limit {
        gb.step();
        if gb.take_breakpoint() {
            let cpu = &gb.cpu;
            let regs = [cpu.b, cpu.c, cpu.d, cpu.e, cpu.h, cpu.l];
            return if regs == [3, 5, 8, 13, 21, 34] {
                Outcome::Pass
            } else if regs == [0x42; 6] {
                Outcome::Fail("テストROMが失敗を報告".to_string())
            } else {
                Outcome::Fail(format!("想定外のレジスタ値 {regs:02X?}"))
            };
        }
        if gb.cpu.locked {
            return Outcome::Fail(format!(
                "未定義オペコードで停止（PC={:04X}）",
                gb.cpu.pc.wrapping_sub(1)
            ));
        }
    }
    Outcome::Fail("タイムアウト".to_string())
}

/// dmg-acid2。描画が終わると `LD B,B` が実行されるので、その後の画面をリファレンス画像と比べる
pub fn run_acid2(rom: &[u8], reference: &Path, dump: Option<&Path>) -> Outcome {
    let expected = match image::read_reference(reference) {
        Ok(pixels) => pixels,
        Err(err) => {
            return Outcome::Fail(format!(
                "リファレンス画像 {} を読めません: {err}",
                reference.display()
            ))
        }
    };
    let mut gb = match load(rom) {
        Ok(gb) => gb,
        Err(outcome) => return outcome,
    };
    let limit = 20 * gb_core::CLOCK_HZ as u64 / 4;
    let mut finished = false;
    while gb.cycles() < limit {
        gb.step();
        if gb.take_breakpoint() {
            finished = true;
            break;
        }
    }
    // 合図の時点では描画途中のことがあるので、完全なフレームができるまで進める
    for _ in 0..3 {
        gb.run_frame();
    }
    let mismatches = gb
        .framebuffer()
        .iter()
        .zip(expected.iter())
        .filter(|(actual, expected)| actual != expected)
        .count();
    if finished && mismatches == 0 {
        return Outcome::Pass;
    }
    if let Some(path) = dump {
        if let Err(err) = image::write_screenshot(path, gb.framebuffer()) {
            eprintln!("{} に保存できません: {err}", path.display());
        }
    }
    if !finished {
        Outcome::Fail("タイムアウト（終了の合図が来ない）".to_string())
    } else {
        Outcome::Fail(format!("{mismatches} ピクセルがリファレンス画像と不一致"))
    }
}
