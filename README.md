# gb-emu

初代ゲームボーイ（DMG）のエミュレータ。本体は Rust で書き、WebAssembly にしてブラウザで動かす。
PC でもスマホでも遊べる。

- 進み具合: [PROGRESS.md](PROGRESS.md)
- 作り方の方針と完成条件: [CLAUDE.md](CLAUDE.md)

## 遊び方

ブラウザでページを開き、「ROM を開く」から自分の ROM ファイル（.gb）を選ぶ。
ROM は端末の中だけで使われ、どこにも送信されない。このリポジトリに ROM は含まれていない。

| 操作 | キーボード | スマホ |
|:--|:--|:--|
| 十字キー | 矢印キー | 画面の十字キー |
| A / B | X / Z | 画面の A / B |
| START / SELECT | Enter / Shift | 画面の START / SELECT |

セーブデータ（バッテリーバックアップ）はブラウザの中に自動で保存される。
「設定」から書き出し・読み込みもできる。

## 手元で動かす

Rust（rustup）と wasm-pack が必要。

```sh
scripts/serve-web.sh          # Web 版をビルドして http://localhost:8000 で配信
cargo run --release -p cli -- <rom> --frames 600 --screenshot out.png   # 画面なしで実行
```

## テスト

```sh
cargo test --workspace        # 単体テスト
scripts/fetch-test-roms.sh    # テストROM（Blargg / mooneye / dmg-acid2）を取得
scripts/run-test-roms.sh      # 全テストROMを実行して合否表を出す
```

## 構成

```
core/     エミュレータ本体（外部依存なし）
cli/      画面なし実行とテストROMランナー
web/      ブラウザ用フロントエンド（wasm-bindgen + HTML/JS）
scripts/  テストROMの取得・一括実行、Web 版のビルド
```
