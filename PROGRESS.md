# 進捗レポート

最終更新: 2026-11-26

## いまどこ
**M2（CPU）・M3（タイマー・割り込み）完了。** 対象テストは全部パス。次は M4（画面）。

M3 でやったこと: 割り込みを受け付けるタイミングが実機より 1 命令遅れる場合があったのを修正
（実機は「次の命令を読み込むサイクルの中」で判定する。HALT からの復帰も同じ扱いにした）。
あわせて起動直後の DIV の位相とジョイパッドの初期値を実機に合わせた。

## テストROM合否表
| 目標 | テスト群 | パス | 前回比 |
|:--|:--|--:|--:|
| M2 | blargg/cpu_instrs | 11/11 | ±0 |
| M2 | blargg/instr_timing | 1/1 | ±0 |
| M2 | blargg/mem_timing | 3/3 | ±0 |
| M3 | mooneye/acceptance/timer | 13/13 | +1 |
| M3 | mooneye/acceptance 割り込み・HALT | 12/12 | ±0 |
| M4 | dmg-acid2 | 1/1 | ±0 |
| M4 | mooneye/acceptance/ppu | 6/12 | +2 |
| M5 | mooneye/emulator-only/mbc1 | 13/13 | ±0 |
| M5 | mooneye/emulator-only/mbc2 | 7/7 | ±0 |
| M5 | mooneye/emulator-only/mbc5 | 8/8 | ±0 |
| M7 | blargg/dmg_sound | 0/12 | ±0 |
| — | blargg/mem_timing-2 | 3/3 | ±0 |
| — | blargg/halt_bug | 0/1 | ±0 |
| — | blargg/oam_bug | 3/8 | ±0 |
| — | mooneye/acceptance その他 | 21/21 | +2 |
| — | mooneye/acceptance/bits | 3/3 | ±0 |
| — | mooneye/acceptance/instr | 1/1 | ±0 |
| — | mooneye/acceptance/oam_dma | 3/3 | ±0 |
| — | mooneye/acceptance/serial | 1/1 | ±0 |
| | **合計** | **110/134** | **+5** |

「前回比」は M1 完了時点（105/134）との差。「目標」が — の行は完成条件外の参考テスト。

## 詰まっていること・既知のバグ
- `ppu/hblank_ly_scx_timing-GS` が通らなくなった（M1 時点では偶然合っていただけ。
  割り込みタイミングを直したので PPU 側のずれが表に出た）。M4 で PPU のタイミングをまとめて合わせる
- 音は出ない（APU は仮実装。M7）
- `blargg/halt_bug` が未通過（タイムアウト。完成条件外だが M4 の後で調べる）

## 次にやること
1. M4: PPU のモード切り替え・STAT 割り込み・LCD ON 直後のタイミングを mooneye のテストに合わせる
2. M4: スプライトによるモード 3 の延長を入れる
3. M5: セーブデータの保存まわり（MBC のテスト自体はすでに全部パス）

## オーナーに判断してほしいこと
- **ライセンス**: リポジトリが公開なので、決めたいものがあれば教えてほしい
  （未指定のままでも開発は進められる。いまは何も宣言していない）
