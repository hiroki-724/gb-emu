//! セーブデータ（バッテリーバックアップ RAM）が実行をまたいで保たれることの確認。

use std::path::PathBuf;
use std::process::Command;

/// カートリッジ RAM の先頭バイトを 1 回だけ +1 して止まるプログラムを持つ ROM を作る
fn counter_rom(type_code: u8) -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]); // NOP / JP 0x0150
    rom[0x147] = type_code;
    rom[0x149] = 0x02; // RAM 8KB
    let program = [
        0x3E, 0x0A, // LD A,0x0A
        0xEA, 0x00, 0x00, // LD (0x0000),A  … RAM を有効にする
        0x21, 0x00, 0xA0, // LD HL,0xA000
        0x34, // INC (HL)
        0x18, 0xFE, // JR -2（ここで止まる）
    ];
    rom[0x150..0x150 + program.len()].copy_from_slice(&program);
    rom
}

fn temp_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gb-cli-save-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn run(rom: &PathBuf, save: &PathBuf) {
    let status = Command::new(env!("CARGO_BIN_EXE_cli"))
        .arg(rom)
        .args(["--frames", "2", "--save"])
        .arg(save)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn battery_ram_persists_across_runs() {
    let rom = temp_path("battery.gb");
    let save = temp_path("battery.sav");
    std::fs::write(&rom, counter_rom(0x03)).unwrap(); // MBC1+RAM+BATTERY
    let _ = std::fs::remove_file(&save);

    // RAM の初期値は 0xFF なので、1 回目で 0x00、2 回目で 0x01 になる
    run(&rom, &save);
    let first = std::fs::read(&save).unwrap();
    assert_eq!(first.len(), 0x2000);
    assert_eq!(first[0], 0x00);

    run(&rom, &save);
    assert_eq!(std::fs::read(&save).unwrap()[0], 0x01);
}

#[test]
fn cartridge_without_battery_writes_no_save_file() {
    let rom = temp_path("no-battery.gb");
    let save = temp_path("no-battery.sav");
    std::fs::write(&rom, counter_rom(0x02)).unwrap(); // MBC1+RAM（バッテリーなし）
    let _ = std::fs::remove_file(&save);

    run(&rom, &save);
    assert!(!save.exists());
}
