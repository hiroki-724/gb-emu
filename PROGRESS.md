# 進捗レポート

最終更新: 2026-11-26

## いまどこ
**M1（土台）完了。** Rust のワークスペース、テストROMの取得・一括実行の仕組み、
画面なしで ROM を動かす CLI ができた。エミュレータ本体も一通り書いたので、
M2 以降の対象テストもすでに多くが通っている（下の表）。次は M2 の確認から順に進める。

## テストROM合否表
| 目標 | テスト群 | パス | 前回比 |
|:--|:--|--:|--:|
| M2 | blargg/cpu_instrs | 11/11 | — |
| M2 | blargg/instr_timing | 1/1 | — |
| M2 | blargg/mem_timing | 3/3 | — |
| M3 | mooneye/acceptance/timer | 12/13 | — |
| M3 | mooneye/acceptance 割り込み・HALT | 12/12 | — |
| M4 | dmg-acid2 | 1/1 | — |
| M4 | mooneye/acceptance/ppu | 4/12 | — |
| M5 | mooneye/emulator-only/mbc1 | 13/13 | — |
| M5 | mooneye/emulator-only/mbc2 | 7/7 | — |
| M5 | mooneye/emulator-only/mbc5 | 8/8 | — |
| M7 | blargg/dmg_sound | 0/12 | — |
| — | blargg/mem_timing-2 | 3/3 | — |
| — | blargg/halt_bug | 0/1 | — |
| — | blargg/oam_bug | 3/8 | — |
| — | mooneye/acceptance その他 | 19/21 | — |
| — | mooneye/acceptance/bits | 3/3 | — |
| — | mooneye/acceptance/instr | 1/1 | — |
| — | mooneye/acceptance/oam_dma | 3/3 | — |
| — | mooneye/acceptance/serial | 1/1 | — |
| | **合計** | **105/134** | **—** |

「目標」が — の行は CLAUDE.md の完成条件には入っていない参考テスト。
mooneye は DMG 向けのものだけ数えている（SGB・CGB 専用は除外）。

## 詰まっていること・既知のバグ
- まだ詰まっているものはない
- 音は出ない（APU はレジスタだけの仮実装。M7 で作る）
- PPU の細かいタイミングは未調整（mooneye/ppu が 4/12。M4 で詰める）
- `timer/rapid_toggle`、`boot_div`、`boot_hwio`、`halt_bug` が未通過

## 次にやること
1. M2: CPU 系テストが全部通っていることを確認して完了にする
2. M3: `timer/rapid_toggle` を通す
3. M4: PPU のタイミングを詰める

## オーナーに判断してほしいこと
- **ライセンス**: リポジトリが公開なので、決めたいものがあれば教えてほしい
  （未指定のままでも開発は進められる。いまは何も宣言していない）
