//! テストROMの一括実行と合否表の出力。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::judge::{self, Outcome};

/// Blargg のテストに与える時間（エミュレータ内の秒数）
const BLARGG_TIMEOUT_SECONDS: u64 = 120;
/// mooneye のテストに与える時間（エミュレータ内の秒数）
const MOONEYE_TIMEOUT_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Blargg,
    Mooneye,
    Acid2,
}

#[derive(Clone, Debug)]
struct TestCase {
    /// テストROMディレクトリからの相対パス（区切りは `/`）
    name: String,
    path: PathBuf,
    kind: Kind,
    group: String,
}

/// 表に出す順番と、対応するマイルストーン
const GROUP_ORDER: &[(&str, &str)] = &[
    ("M2", "blargg/cpu_instrs"),
    ("M2", "blargg/instr_timing"),
    ("M2", "blargg/mem_timing"),
    ("M3", "mooneye/acceptance/timer"),
    ("M3", "mooneye/acceptance 割り込み・HALT"),
    ("M4", "dmg-acid2"),
    ("M4", "mooneye/acceptance/ppu"),
    ("M5", "mooneye/emulator-only/mbc1"),
    ("M5", "mooneye/emulator-only/mbc2"),
    ("M5", "mooneye/emulator-only/mbc5"),
    ("M7", "blargg/dmg_sound"),
    ("—", "blargg/mem_timing-2"),
    ("—", "blargg/halt_bug"),
    ("—", "blargg/oam_bug"),
    ("—", "mooneye/acceptance その他"),
    ("—", "mooneye/acceptance/bits"),
    ("—", "mooneye/acceptance/instr"),
    ("—", "mooneye/acceptance/oam_dma"),
    ("—", "mooneye/acceptance/serial"),
];

/// mooneye のファイル名末尾の機種指定を見て、DMG（リビジョン A/B/C）で通るべきテストかを返す。
///
/// 末尾の `-xxx` が対象機種を表す。指定なしは全機種共通。
/// グループ記号は G = DMG+MGB、S = SGB+SGB2、C = CGB+AGB+AGS、A = AGB+AGS。
fn mooneye_targets_dmg(stem: &str) -> bool {
    let Some((_, suffix)) = stem.rsplit_once('-') else {
        return true;
    };
    if suffix.contains("dmgABC") {
        return true;
    }
    if !suffix.is_empty() && suffix.chars().all(|c| matches!(c, 'G' | 'S' | 'C' | 'A')) {
        return suffix.contains('G');
    }
    const OTHER_MODELS: [&str; 6] = ["dmg0", "mgb", "sgb", "cgb", "agb", "ags"];
    !OTHER_MODELS.iter().any(|model| suffix.starts_with(model))
}

/// 割り込み・HALT 関連として数える mooneye acceptance 直下のテスト
fn is_interrupt_test(stem: &str) -> bool {
    const PREFIXES: [&str; 7] = [
        "ei_",
        "di_",
        "halt_",
        "intr_",
        "if_ie",
        "rapid_di_ei",
        "reti_intr",
    ];
    PREFIXES.iter().any(|prefix| stem.starts_with(prefix))
}

/// 相対パスから種類とグループを決める。対象外のファイルは None
fn classify(name: &str) -> Option<(Kind, String)> {
    let parts: Vec<&str> = name.split('/').collect();
    let stem = parts.last()?.strip_suffix(".gb")?;
    match parts.as_slice() {
        ["blargg", "cpu_instrs", "individual", _] => {
            Some((Kind::Blargg, "blargg/cpu_instrs".into()))
        }
        ["blargg", "instr_timing", "instr_timing.gb"] => {
            Some((Kind::Blargg, "blargg/instr_timing".into()))
        }
        ["blargg", "mem_timing", "individual", _] => {
            Some((Kind::Blargg, "blargg/mem_timing".into()))
        }
        ["blargg", "mem_timing-2", "rom_singles", _] => {
            Some((Kind::Blargg, "blargg/mem_timing-2".into()))
        }
        ["blargg", "halt_bug.gb"] => Some((Kind::Blargg, "blargg/halt_bug".into())),
        ["blargg", "dmg_sound", "rom_singles", _] => {
            Some((Kind::Blargg, "blargg/dmg_sound".into()))
        }
        ["blargg", "oam_bug", "rom_singles", _] => Some((Kind::Blargg, "blargg/oam_bug".into())),
        ["dmg-acid2", "dmg-acid2.gb"] => Some((Kind::Acid2, "dmg-acid2".into())),
        ["mooneye", "acceptance", rest @ ..] => {
            if !mooneye_targets_dmg(stem) {
                return None;
            }
            let group = match rest {
                [_] if is_interrupt_test(stem) => "mooneye/acceptance 割り込み・HALT".to_string(),
                [_] => "mooneye/acceptance その他".to_string(),
                ["interrupts", ..] => "mooneye/acceptance 割り込み・HALT".to_string(),
                [dir, ..] => format!("mooneye/acceptance/{dir}"),
                [] => return None,
            };
            Some((Kind::Mooneye, group))
        }
        ["mooneye", "emulator-only", dir, ..] => {
            if !mooneye_targets_dmg(stem) {
                return None;
            }
            Some((Kind::Mooneye, format!("mooneye/emulator-only/{dir}")))
        }
        _ => None,
    }
}

fn collect_roms(dir: &Path, base: &Path, out: &mut Vec<TestCase>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_roms(&path, base, out)?;
            continue;
        }
        let Ok(relative) = path.strip_prefix(base) else {
            continue;
        };
        let name = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if let Some((kind, group)) = classify(&name) {
            out.push(TestCase {
                name,
                path,
                kind,
                group,
            });
        }
    }
    Ok(())
}

fn run_case(case: &TestCase, dump_dir: Option<&Path>) -> Outcome {
    let rom = match std::fs::read(&case.path) {
        Ok(rom) => rom,
        Err(err) => return Outcome::Fail(format!("読み込み失敗: {err}")),
    };
    // エミュレータ本体のバグで 1 本がパニックしても、残りのテストは最後まで実行する
    let result = std::panic::catch_unwind(|| match case.kind {
        Kind::Blargg => judge::run_blargg(&rom, BLARGG_TIMEOUT_SECONDS),
        Kind::Mooneye => judge::run_mooneye(&rom, MOONEYE_TIMEOUT_SECONDS),
        Kind::Acid2 => {
            let reference = case.path.with_file_name("reference-dmg.png");
            let dump = dump_dir.map(|dir| dir.join("dmg-acid2.png"));
            judge::run_acid2(&rom, &reference, dump.as_deref())
        }
    });
    result.unwrap_or_else(|_| Outcome::Fail("エミュレータがパニック".to_string()))
}

fn run_all(cases: &[TestCase], jobs: usize, dump_dir: Option<&Path>) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; cases.len()]);
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(index) else {
                    break;
                };
                let outcome = run_case(case, dump_dir);
                results.lock().unwrap()[index] = Some(outcome);
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|outcome| outcome.expect("全テストに結果がある"))
        .collect()
}

/// 結果ファイル（`PASS<TAB>名前` / `FAIL<TAB>名前` の行）を読む
fn read_results(path: &Path) -> Option<BTreeMap<String, bool>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((status, name)) = line.split_once('\t') {
            map.insert(name.to_string(), status == "PASS");
        }
    }
    Some(map)
}

fn write_results(path: &Path, cases: &[TestCase], outcomes: &[Outcome]) -> std::io::Result<()> {
    let mut lines: Vec<String> = cases
        .iter()
        .zip(outcomes)
        .map(|(case, outcome)| {
            let status = if outcome.passed() { "PASS" } else { "FAIL" };
            format!("{status}\t{}", case.name)
        })
        .collect();
    lines.sort_by(|a, b| a.split('\t').nth(1).cmp(&b.split('\t').nth(1)));
    std::fs::write(path, lines.join("\n") + "\n")
}

fn format_delta(delta: i64) -> String {
    match delta {
        0 => "±0".to_string(),
        d if d > 0 => format!("+{d}"),
        d => d.to_string(),
    }
}

pub fn main(args: &[String]) -> ExitCode {
    let mut dir = PathBuf::from("test-roms");
    let mut results_path: Option<PathBuf> = None;
    let mut filter: Option<String> = None;
    let mut dump_dir: Option<PathBuf> = None;
    let mut list_failures = false;
    let mut jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next().cloned().ok_or_else(|| {
                eprintln!("{name} には値を指定してください");
                ExitCode::from(2)
            })
        };
        match arg.as_str() {
            "--results" => match value("--results") {
                Ok(v) => results_path = Some(PathBuf::from(v)),
                Err(code) => return code,
            },
            "--filter" => match value("--filter") {
                Ok(v) => filter = Some(v),
                Err(code) => return code,
            },
            "--dump-dir" => match value("--dump-dir") {
                Ok(v) => dump_dir = Some(PathBuf::from(v)),
                Err(code) => return code,
            },
            "--jobs" => match value("--jobs").map(|v| v.parse::<usize>()) {
                Ok(Ok(n)) if n > 0 => jobs = n,
                Ok(_) => {
                    eprintln!("--jobs には 1 以上の数値を指定してください");
                    return ExitCode::from(2);
                }
                Err(code) => return code,
            },
            "--list-failures" => list_failures = true,
            other if other.starts_with("--") => {
                eprintln!("不明なオプションです: {other}");
                return ExitCode::from(2);
            }
            other => dir = PathBuf::from(other),
        }
    }

    let mut cases = Vec::new();
    if let Err(err) = collect_roms(&dir, &dir, &mut cases) {
        eprintln!(
            "{} を読めません: {err}\nscripts/fetch-test-roms.sh でテストROMを取得してください",
            dir.display()
        );
        return ExitCode::FAILURE;
    }
    if let Some(filter) = &filter {
        cases.retain(|case| case.name.contains(filter.as_str()));
    }
    if cases.is_empty() {
        eprintln!(
            "{} に実行できるテストROMがありません。scripts/fetch-test-roms.sh で取得してください",
            dir.display()
        );
        return ExitCode::FAILURE;
    }
    if let Some(dump_dir) = &dump_dir {
        if let Err(err) = std::fs::create_dir_all(dump_dir) {
            eprintln!("{} を作れません: {err}", dump_dir.display());
            return ExitCode::FAILURE;
        }
    }

    // パニック時の既定メッセージは並列実行だと読みにくいので抑える（結果は表に「パニック」と出る）
    std::panic::set_hook(Box::new(|_| {}));
    let started = Instant::now();
    let outcomes = run_all(&cases, jobs, dump_dir.as_deref());
    let elapsed = started.elapsed();
    let _ = std::panic::take_hook();

    let baseline = results_path.as_deref().and_then(read_results);

    // グループごとの集計
    let mut groups: Vec<(&str, String)> = GROUP_ORDER
        .iter()
        .map(|(milestone, group)| (*milestone, group.to_string()))
        .collect();
    for case in &cases {
        if !groups.iter().any(|(_, group)| *group == case.group) {
            groups.push(("—", case.group.clone()));
        }
    }

    println!("| 目標 | テスト群 | パス | 前回比 |");
    println!("|:--|:--|--:|--:|");
    let mut total_pass = 0;
    let mut total_delta = 0i64;
    for (milestone, group) in &groups {
        let members: Vec<usize> = (0..cases.len())
            .filter(|&i| cases[i].group == *group)
            .collect();
        if members.is_empty() {
            continue;
        }
        let pass = members.iter().filter(|&&i| outcomes[i].passed()).count();
        total_pass += pass;
        let delta = baseline.as_ref().map(|baseline| {
            let before = members
                .iter()
                .filter(|&&i| baseline.get(&cases[i].name).copied().unwrap_or(false))
                .count();
            pass as i64 - before as i64
        });
        if let Some(delta) = delta {
            total_delta += delta;
        }
        println!(
            "| {milestone} | {group} | {pass}/{} | {} |",
            members.len(),
            delta.map(format_delta).unwrap_or_else(|| "—".to_string())
        );
    }
    println!(
        "| | **合計** | **{total_pass}/{}** | **{}** |",
        cases.len(),
        if baseline.is_some() {
            format_delta(total_delta)
        } else {
            "—".to_string()
        }
    );
    println!();
    println!(
        "{} 本を {:.1} 秒で実行（並列 {jobs}）",
        cases.len(),
        elapsed.as_secs_f64()
    );

    // 前回との差分
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    if let Some(baseline) = &baseline {
        for (case, outcome) in cases.iter().zip(&outcomes) {
            match (baseline.get(&case.name).copied(), outcome) {
                (Some(true), Outcome::Fail(reason)) => regressions.push((case, reason)),
                (Some(false), Outcome::Pass) | (None, Outcome::Pass) => improvements.push(case),
                _ => {}
            }
        }
    }
    if !improvements.is_empty() {
        println!();
        println!("新しく通ったテスト（{} 本）:", improvements.len());
        for case in &improvements {
            println!("  + {}", case.name);
        }
    }
    if !regressions.is_empty() {
        println!();
        println!("通らなくなったテスト（{} 本）:", regressions.len());
        for (case, reason) in &regressions {
            println!("  - {}: {reason}", case.name);
        }
    }

    if list_failures {
        println!();
        println!("失敗したテスト:");
        for (case, outcome) in cases.iter().zip(&outcomes) {
            if let Outcome::Fail(reason) = outcome {
                println!("  {}: {reason}", case.name);
            }
        }
    }

    // 絞り込み実行のときは結果ファイルを書き換えない（全体の記録が欠けてしまうため）
    if filter.is_none() {
        if let Some(path) = &results_path {
            if let Err(err) = write_results(path, &cases, &outcomes) {
                eprintln!("{} に書けません: {err}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    if regressions.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mooneye_model_suffixes() {
        assert!(mooneye_targets_dmg("tima_reload"));
        assert!(mooneye_targets_dmg("boot_regs-dmgABC"));
        assert!(mooneye_targets_dmg("boot_div-dmgABCmgb"));
        assert!(mooneye_targets_dmg("halt_ime1_timing2-GS"));
        assert!(!mooneye_targets_dmg("boot_regs-dmg0"));
        assert!(!mooneye_targets_dmg("boot_regs-mgb"));
        assert!(!mooneye_targets_dmg("boot_regs-sgb2"));
        assert!(!mooneye_targets_dmg("boot_div-S"));
        assert!(!mooneye_targets_dmg("unused_hwio-C"));
        assert!(!mooneye_targets_dmg("boot_regs-cgbABCDE"));
        assert!(!mooneye_targets_dmg("boot_regs-A"));
    }

    #[test]
    fn classification() {
        let group = |name: &str| classify(name).map(|(_, group)| group);
        assert_eq!(
            group("blargg/cpu_instrs/individual/01-special.gb").as_deref(),
            Some("blargg/cpu_instrs")
        );
        assert_eq!(group("blargg/cpu_instrs/cpu_instrs.gb"), None);
        assert_eq!(group("blargg/cgb_sound/rom_singles/01-registers.gb"), None);
        assert_eq!(
            group("mooneye/acceptance/timer/tima_reload.gb").as_deref(),
            Some("mooneye/acceptance/timer")
        );
        assert_eq!(
            group("mooneye/acceptance/ei_timing.gb").as_deref(),
            Some("mooneye/acceptance 割り込み・HALT")
        );
        assert_eq!(
            group("mooneye/acceptance/interrupts/ie_push.gb").as_deref(),
            Some("mooneye/acceptance 割り込み・HALT")
        );
        assert_eq!(
            group("mooneye/acceptance/call_timing.gb").as_deref(),
            Some("mooneye/acceptance その他")
        );
        assert_eq!(group("mooneye/acceptance/boot_regs-mgb.gb"), None);
        assert_eq!(group("mooneye/manual-only/sprite_priority.gb"), None);
        assert_eq!(
            group("mooneye/emulator-only/mbc1/rom_8Mb.gb").as_deref(),
            Some("mooneye/emulator-only/mbc1")
        );
        assert_eq!(
            group("dmg-acid2/dmg-acid2.gb").as_deref(),
            Some("dmg-acid2")
        );
    }

    #[test]
    fn delta_formatting() {
        assert_eq!(format_delta(0), "±0");
        assert_eq!(format_delta(3), "+3");
        assert_eq!(format_delta(-2), "-2");
    }
}
