# CLAUDE.md — ゲームボーイエミュレータ

## プロジェクト概要
初代ゲームボーイ（DMG）のエミュレータを Rust で作り、WebAssembly でブラウザ上で動かす。
最終的に GitHub Pages で公開し、PC とスマホのブラウザで遊べるようにする。

オーナー（人間）は主にスマホから進捗を確認する。長時間自走する前提なので、判断に迷ったら
「作業ルール」に従って自分で決めて進めること。

## 完成条件（マイルストーン）
上から順に進める。各項目は自動テストで合否を判定できる状態にしてから「完了」とする。

### M1: 土台
- [x] Cargo workspace（`core` / `cli` / `web`）を作る
- [x] テストROM取得スクリプト `scripts/fetch-test-roms.sh` を作る
- [x] `cli` で ROM をヘッドレス実行し、シリアル出力（0xFF01 / 0xFF02）を標準出力に流せる
- [x] `scripts/run-test-roms.sh` で全テストROMを実行し、合否表を出せる

### M2: CPU
- [x] Blargg `cpu_instrs` 全11個パス
- [x] Blargg `instr_timing` パス
- [x] Blargg `mem_timing` パス

### M3: タイマー・割り込み
- [x] mooneye `acceptance/timer/` 全パス
- [x] mooneye `acceptance/` の割り込み・HALT 関連（`ei_*`, `di_timing`, `halt_*`, `intr_*` など）パス

### M4: PPU（画面）
- [x] `dmg-acid2` がリファレンス画像と一致
- [x] mooneye `acceptance/ppu/` をできる範囲でパス（難しいものは PROGRESS.md に記録して後回し可）

### M5: カートリッジ
- [ ] MBC1 / MBC3 / MBC5 対応
- [ ] mooneye `emulator-only/mbc1/` 全パス（mbc2・mbc5 のテストも可能な範囲で）
- [ ] バッテリーバックアップRAMの保存（web版は localStorage）

### M6: Web フロントエンド
- [ ] ブラウザで ROM ファイルを選んで起動できる（ROM は同梱しない）
- [ ] 160×144 を整数倍スケールし、ピクセルをくっきり表示（`image-rendering: pixelated`）
- [ ] キーボード操作（矢印 / Z=B / X=A / Enter=Start / Shift=Select）
- [ ] スマホ用タッチ操作（十字キー・A/B・Start/Select）、縦画面でも横画面でも遊べるレイアウト
- [ ] スマホのブラウザで約 60fps（59.7fps）で安定動作
- [ ] GitHub Actions でビルド → GitHub Pages に自動デプロイ

### M7: サウンド（余力があれば）
- [ ] APU 実装、Web Audio で再生
- [ ] Blargg `dmg_sound` をできる範囲でパス

## 構成
```
core/       エミュレータ本体。純粋な Rust、外部依存なし、I/O なし
            cpu / bus / ppu / timer / apu / cartridge(mbc) / joypad / interrupts
cli/        ヘッドレス実行・テスト用ランナー
web/        wasm-bindgen + HTML/JS のフロントエンド
scripts/    テストROMの取得・一括実行
test-roms/  取得したテストROM（.gitignore に入れる）
test-results.txt  テストROMごとの合否の記録（前回との比較に使う）
PROGRESS.md 進捗レポート
```
- `core` はフロントエンドに依存しない。画面はフレームバッファ（160×144）、入力はボタン状態、
  音はサンプル列としてやり取りする。
- タイミングは M サイクル単位で正確に。CPU がメモリアクセスするたびに PPU・タイマーなどを進める方式にする。
- `core` 以外（`cli` / `web`）は必要最小限の外部クレートを使ってよい。

## テストROMと合否判定
取得元（スクリプトでダウンロードする）:
- Blargg's test ROMs: https://github.com/retrio/gb-test-roms
- mooneye-test-suite: https://github.com/Gekkio/mooneye-test-suite （ビルド済み ROM を使う）
- dmg-acid2: https://github.com/mattcurrie/dmg-acid2 （Releases）

判定方法（細部は各 ROM の README で確認すること）:
- Blargg: シリアル出力に `Passed` / `Failed` が出る。一部の ROM は結果を $A000 以降のメモリに書く
- mooneye: `LD B,B`（0x40）実行時に B,C,D,E,H,L = 3,5,8,13,21,34 ならパス、すべて 0x42 ならフェイル
- dmg-acid2: 完了時のフレームバッファをリファレンス画像と比較
- 各テストにタイムアウトを設け、無限ループで止まらないようにする

## コマンド
- `cargo test --workspace` — 単体テスト
- `cargo fmt --all` / `cargo clippy --workspace --all-targets`
- `cargo run --release -p cli -- <rom> [--serial] [--frames N] [--screenshot out.png]` — 画面なしで実行
  （`--frames` の既定は 1800 = 約 30 秒ぶん）
- `scripts/fetch-test-roms.sh [--force]` — テストROMを `test-roms/` に取得（版と SHA-256 を固定）
- `scripts/run-test-roms.sh [--list-failures] [--filter STR]` → 合否表を出力。
  結果を `test-results.txt` に記録し、前回パスしていたテストが落ちたら終了コード 1 を返す。
  dmg-acid2 が失敗したときの画面は `target/test-output/` に保存される
- `wasm-pack build web --target web` → `web/` を静的サーバーで確認（M6 で用意する）

補足:
- パッケージ名は `core/` が `gb-core`、`cli/` が `cli`、`web/` が `gb-web`
  （`core` という名前のクレートは Rust 標準の `core` と衝突するため）
- Rust は rustup で導入済み。`cargo` が見つからないシェルでは `source ~/.cargo/env` を実行する

## 作業ルール
- 作業の区切りごとに `cargo test --workspace` と `scripts/run-test-roms.sh` を実行し、
  パス数が減っていないことを確認してからコミット・プッシュする
- コミットは小さく。メッセージは日本語で、何を直したか分かるように書く
- **テストの期待値を書き換えて通すのは禁止**。特定のテストROMだけを通すための決め打ち処理も禁止
- 同じテストでアプローチを3回変えても通らないときは、調べたことと仮説を PROGRESS.md に書いて
  次の項目へ進み、あとで戻る
- 参考資料（Pan Docs など）と違う実装をするときは、理由をコメントに残す
- 独立したモジュール（例: APU と Web フロントエンド）はサブエージェントに並行して任せてよい。
  統合とレビューは自分で行う
- 市販ゲームの ROM は絶対にダウンロード・同梱しない。使うのは上記のテストROMと、
  ライセンスが明示された homebrew のみ

## PROGRESS.md（オーナーがスマホで見る）
作業の区切りごとに更新する。常に以下を含める:
1. 現在のマイルストーンと一言サマリー
2. テストROMの合否表（パス数 / 総数、前回からの変化）
3. 詰まっていること・既知のバグ
4. 次にやること
5. オーナーに判断してほしいこと（あれば）

スマホ1〜2画面で読める長さに保つ。

## オーナーがやること
- リポジトリの Settings → Pages で Source を「GitHub Actions」にする（M6 のデプロイ前）
- 実際に遊んでみてのフィードバック

## 参考資料
- Pan Docs: https://gbdev.io/pandocs/
- オペコード表: https://gbdev.io/gb-opcodes/optables/
- Gekkio "Game Boy: Complete Technical Reference"
