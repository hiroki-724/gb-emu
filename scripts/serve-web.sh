#!/usr/bin/env bash
# Web 版をビルドして、手元のブラウザで確認できるようにする。
#
#   scripts/serve-web.sh            ビルドして http://localhost:8000 で配信する
#   scripts/serve-web.sh --out DIR  ビルドして DIR に公開用ファイル一式を置くだけ（配信しない）
#
# 公開用ファイルは index.html / style.css / app.js / audio-worklet.js / pkg/（wasm-pack の出力）。
set -euo pipefail

cd "$(dirname "$0")/.."

OUT=""
if [[ "${1:-}" == "--out" ]]; then
  OUT="${2:?--out には出力先ディレクトリを指定してください}"
elif [[ $# -gt 0 ]]; then
  echo "使い方: $0 [--out DIR]" >&2
  exit 2
fi

wasm-pack build web --target web --release

if [[ -z "$OUT" ]]; then
  OUT="$(mktemp -d)"
  SERVE=1
else
  SERVE=0
fi

mkdir -p "$OUT/pkg"
cp web/index.html web/style.css web/app.js web/audio-worklet.js "$OUT/"
cp web/pkg/gb_web.js web/pkg/gb_web_bg.wasm "$OUT/pkg/"
echo "公開用ファイルを $OUT に置きました"

if [[ $SERVE -eq 1 ]]; then
  echo "http://localhost:8000 を開いてください（Ctrl+C で終了）"
  cd "$OUT"
  exec python3 -m http.server 8000
fi
