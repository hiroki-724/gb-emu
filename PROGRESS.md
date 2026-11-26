# 進捗レポート

最終更新: 2026-11-26

## いまどこ
**M1〜M7 の実装は一通り完了。残りはオーナーの操作待ちの 2 点**
（GitHub Pages の公開、スマホ実機での確認）。

M7（サウンド）でやったこと:
- APU（矩形波 ×2・波形メモリ・ノイズ、長さ・エンベロープ・スイープ）を実装。
  Blargg の dmg_sound は 12 本すべてパス
- Web 版は AudioWorklet で再生。再生側のバッファ残量を見て生成レートを最大 ±2% 調整し、
  音が溜まったり途切れたりしないようにしている。設定で音の ON/OFF ができる

## テストROM合否表
| 目標 | テスト群 | パス | 前回比 |
|:--|:--|--:|--:|
| M2 | blargg/cpu_instrs | 11/11 | ±0 |
| M2 | blargg/instr_timing | 1/1 | ±0 |
| M2 | blargg/mem_timing | 3/3 | ±0 |
| M3 | mooneye/acceptance/timer | 13/13 | ±0 |
| M3 | mooneye/acceptance 割り込み・HALT | 12/12 | ±0 |
| M4 | dmg-acid2 | 1/1 | ±0 |
| M4 | mooneye/acceptance/ppu | 12/12 | ±0 |
| M5 | mooneye/emulator-only/mbc1 | 13/13 | ±0 |
| M5 | mooneye/emulator-only/mbc2 | 7/7 | ±0 |
| M5 | mooneye/emulator-only/mbc5 | 8/8 | ±0 |
| M7 | blargg/dmg_sound | 12/12 | +12 |
| — | blargg/mem_timing-2 | 3/3 | ±0 |
| — | blargg/halt_bug | 1/1 | ±0 |
| — | blargg/oam_bug | 3/8 | ±0 |
| — | mooneye/acceptance その他 | 21/21 | ±0 |
| — | mooneye/acceptance/bits | 3/3 | ±0 |
| — | mooneye/acceptance/instr | 1/1 | ±0 |
| — | mooneye/acceptance/oam_dma | 3/3 | ±0 |
| — | mooneye/acceptance/serial | 1/1 | ±0 |
| | **合計** | **129/134** | **+12** |

「前回比」は M6 時点（117/134）との差。「目標」が — の行は完成条件外の参考テスト。

## 詰まっていること・既知のバグ
- 詰まっているものはない
- **音は耳で確認できていない**。確認できたのは「ブラウザの音声出力が動き出し、無音でない
  サンプルが再生側に流れ、残量が増減している」ところまで。音程は単体テストで確認済み
- スマホ実機ではまだ誰も動かしていない（60fps・音・タッチ操作は実機で要確認）
- `blargg/oam_bug` 5 本が未通過（DMG 実機の「OAM 破壊バグ」の再現。完成条件外）
- 描画は 1 行単位。行の途中でスクロールやパレットを書き換える演出は、行頭の値で描かれる

## 次にやること
1. オーナーの確認結果を受けて、音・操作感・表示を調整する
2. 余力があれば `oam_bug`、行の途中の描画変更への対応

## オーナーに判断してほしいこと
- **GitHub Pages を有効にする**: リポジトリの Settings → Pages で Source を「GitHub Actions」に。
  その後の最初の push（または Actions 画面から手動実行）で
  https://hiroki-724.github.io/gb-emu/ に公開される。ビルドとテストはすでに Actions で通っている
- **スマホで遊んでみての感想**: 音が出るか・途切れないか、動きの滑らかさ、ボタンの大きさと位置
- **ライセンス**: リポジトリが公開なので、決めたいものがあれば教えてほしい（いまは何も宣言していない）
