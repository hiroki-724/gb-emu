# 進捗レポート

最終更新: 2026-11-26

## いまどこ
**M6（Web 版）がほぼ完成。残りは「GitHub Pages の公開」と「スマホ実機での確認」で、
どちらもオーナーの操作待ち**（下の「判断してほしいこと」）。M1〜M5 は完了。

Web 版でできること:
- 「ROM を開く」で自分の ROM を選んで起動（ROM は同梱していない・外に送らない）
- 画面は端末のピクセルで整数倍に拡大してくっきり表示。配色は 4 種類
- キーボード（矢印 / Z=B / X=A / Enter / Shift）と、スマホ用のタッチ操作
  （十字キーの斜め、A+B 同時押し、複数の指に対応）。縦持ち・横持ちの両方のレイアウト
- セーブは自動でブラウザ内に保存。書き出し・読み込みもできる
- 前回の ROM を端末に覚えておき「つづきから」で再開できる

確認したこと（Mac 上のブラウザ、スマホ相当の画面サイズ）: 表示・入力・セーブ・配色切り替え。
速度は 1 フレームあたり約 0.17 ミリ秒で、60fps に必要な時間の約 100 分の 1。

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
| M7 | blargg/dmg_sound | 0/12 | ±0 |
| — | blargg/mem_timing-2 | 3/3 | ±0 |
| — | blargg/halt_bug | 1/1 | ±0 |
| — | blargg/oam_bug | 3/8 | ±0 |
| — | mooneye/acceptance その他 | 21/21 | ±0 |
| — | mooneye/acceptance/bits | 3/3 | ±0 |
| — | mooneye/acceptance/instr | 1/1 | ±0 |
| — | mooneye/acceptance/oam_dma | 3/3 | ±0 |
| — | mooneye/acceptance/serial | 1/1 | ±0 |
| | **合計** | **117/134** | **±0** |

「前回比」は M5 時点との差。「目標」が — の行は完成条件外の参考テスト。

## 詰まっていること・既知のバグ
- 詰まっているものはない
- 音は出ない（APU は仮実装。M7）
- スマホ実機ではまだ誰も動かしていない（60fps で安定するかは実機で要確認）
- `blargg/oam_bug` 5 本が未通過（DMG 実機の「OAM 破壊バグ」の再現。完成条件外）
- 描画は 1 行単位。行の途中でスクロールやパレットを書き換える演出は、行頭の値で描かれる

## 次にやること
1. GitHub Actions のビルドが通ることを確認する
2. M7: サウンド（APU の実装と Web Audio での再生、Blargg `dmg_sound`）

## オーナーに判断してほしいこと
- **GitHub Pages を有効にする**: リポジトリの Settings → Pages で Source を「GitHub Actions」に。
  有効にした後の最初の push（または Actions 画面から手動実行）で
  https://hiroki-724.github.io/gb-emu/ に公開される。それまではビルドとテストだけ行う
- **スマホで遊んでみての感想**: 動きの滑らかさ、ボタンの大きさ・位置、押し間違いなど
- **ライセンス**: リポジトリが公開なので、決めたいものがあれば教えてほしい（いまは何も宣言していない）
