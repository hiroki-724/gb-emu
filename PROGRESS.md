# 進捗レポート

最終更新: 2026-11-26

## いまどこ
**M5（カートリッジ）は Web 版のセーブ以外完了、M6（Web）に着手。**
MBC1 / MBC2 / MBC3（時計つき）/ MBC5 に対応し、mooneye の MBC テスト 28 本は全部パス。
セーブデータは CLI の `--save` で読み書きできる（時計つきカートリッジは、電源を切っていた間の
経過時間ぶん時計が進む）。Web 版のセーブ（localStorage）は M6 で一緒に作る。

M4 までの要約: dmg-acid2 一致、mooneye acceptance（DMG 向け 67 本）全パス。

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

「前回比」は M4 完了時点（117/134）との差。「目標」が — の行は完成条件外の参考テスト。
速度: 60 秒ぶんのエミュレーションが Mac 上で約 0.5 秒（実機の約 120 倍速）。

## 詰まっていること・既知のバグ
- 詰まっているものはない
- 音は出ない（APU は仮実装。M7）
- `blargg/oam_bug` 5 本が未通過（DMG 実機の「OAM 破壊バグ」の再現。完成条件外で、
  これに依存する市販ソフトはほぼ無いので後回し）
- 描画は 1 行単位。行の途中でスクロールやパレットを書き換える演出は、行頭の値で描かれる

## 次にやること
1. M6: Web フロントエンド（wasm-bindgen、整数倍表示、キーボード・タッチ操作、localStorage セーブ）
2. M6: GitHub Actions でビルドして GitHub Pages にデプロイ
3. M7: サウンド

## オーナーに判断してほしいこと
- **ライセンス**: リポジトリが公開なので、決めたいものがあれば教えてほしい
  （未指定のままでも開発は進められる。いまは何も宣言していない）
- **GitHub Pages**: M6 のデプロイ前に、リポジトリの Settings → Pages で Source を
  「GitHub Actions」にしておいてほしい
