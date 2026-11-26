//! ゲームボーイエミュレータのヘッドレス実行とテストROMランナー。

mod image;
mod judge;
mod suite;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use gb_core::GameBoy;

const USAGE: &str = "\
使い方:
  cli <rom> [--serial] [--frames N] [--screenshot out.png] [--save FILE]
      ROM を画面なしで実行する
        --serial          シリアル出力（0xFF01 / 0xFF02）を標準出力に流す
        --frames N        実行するフレーム数（既定 1800 = 約 30 秒ぶん）
        --screenshot FILE 終了時の画面を PNG で保存する
        --save FILE       セーブデータ（バッテリーバックアップ RAM）。あれば起動時に読み込み、
                          終了時に書き出す。バッテリーのないカートリッジでは何もしない

  cli suite [DIR] [--results FILE] [--filter STR] [--dump-dir DIR] [--jobs N] [--list-failures]
      DIR（既定 test-roms）以下のテストROMをすべて実行し、合否表を出す
        --results FILE    結果ファイル。前回の内容と比べて変化を表示し、今回の結果で上書きする
        --filter STR      パスに STR を含むテストだけ実行する（結果ファイルは更新しない）
        --dump-dir DIR    画面比較テストが失敗したとき、実際の画面を PNG で保存する
        --jobs N          並列数（既定は CPU コア数）
        --list-failures   失敗したテストの一覧と理由も表示する
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("suite") => suite::main(&args[1..]),
        Some(_) => run_rom(&args),
    }
}

fn run_rom(args: &[String]) -> ExitCode {
    let mut rom_path: Option<PathBuf> = None;
    let mut serial = false;
    let mut frames: u64 = 1800;
    let mut screenshot: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--serial" => serial = true,
            "--frames" => match iter.next().and_then(|v| v.parse().ok()) {
                Some(n) => frames = n,
                None => return usage_error("--frames には数値を指定してください"),
            },
            "--screenshot" => match iter.next() {
                Some(path) => screenshot = Some(PathBuf::from(path)),
                None => return usage_error("--screenshot には出力ファイルを指定してください"),
            },
            "--save" => match iter.next() {
                Some(path) => save = Some(PathBuf::from(path)),
                None => return usage_error("--save にはファイルを指定してください"),
            },
            other if other.starts_with("--") => {
                return usage_error(&format!("不明なオプションです: {other}"))
            }
            other => {
                if rom_path.is_some() {
                    return usage_error("ROM は 1 つだけ指定してください");
                }
                rom_path = Some(PathBuf::from(other));
            }
        }
    }
    let Some(rom_path) = rom_path else {
        return usage_error("ROM ファイルを指定してください");
    };

    let rom = match std::fs::read(&rom_path) {
        Ok(rom) => rom,
        Err(err) => {
            eprintln!("{} を読めません: {err}", rom_path.display());
            return ExitCode::FAILURE;
        }
    };
    let mut gb = match GameBoy::new(rom) {
        Ok(gb) => gb,
        Err(err) => {
            eprintln!("{}: {err}", rom_path.display());
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &save {
        // ファイルがまだ無いのは初回起動なので、エラーにしない
        match std::fs::read(path) {
            Ok(data) => gb.cartridge_mut().load_save_data(&data, unix_time()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                eprintln!("{} を読めません: {err}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    let stdout = std::io::stdout();
    for _ in 0..frames {
        gb.run_frame();
        let output = gb.take_serial_output();
        if serial && !output.is_empty() {
            let mut out = stdout.lock();
            // パイプの先が閉じられても異常終了しないよう、書き込みエラーは無視する
            let _ = out.write_all(&output);
            let _ = out.flush();
        }
    }

    if let Some(path) = &save {
        if let Some(data) = gb.cartridge().save_data(unix_time()) {
            if let Err(err) = std::fs::write(path, data) {
                eprintln!("{} に保存できません: {err}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    if let Some(path) = screenshot {
        if let Err(err) = image::write_screenshot(&path, gb.framebuffer()) {
            eprintln!("{} に保存できません: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// 現在時刻（UNIX 秒）。RTC 付きカートリッジのセーブデータに記録する
fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("{message}\n\n{USAGE}");
    ExitCode::from(2)
}
