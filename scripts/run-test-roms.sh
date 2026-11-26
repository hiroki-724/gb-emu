#!/usr/bin/env bash
# test-roms/ 以下の全テストROMを実行し、合否表を出す。
#
#   scripts/run-test-roms.sh                    全テストを実行し、test-results.txt を更新する
#   scripts/run-test-roms.sh --list-failures    失敗したテストと理由も表示する
#   scripts/run-test-roms.sh --filter timer     名前に timer を含むテストだけ実行する（記録は更新しない）
#
# 前回の結果（test-results.txt）と比べ、通らなくなったテストがあれば終了コード 1 を返す。
# そのほかのオプションは `cargo run -p cli -- --help` を参照。
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ ! -d test-roms/blargg || ! -d test-roms/mooneye || ! -d test-roms/dmg-acid2 ]]; then
  echo "テストROMがありません。先に scripts/fetch-test-roms.sh を実行してください" >&2
  exit 1
fi

cargo build --release --quiet -p cli
exec target/release/cli suite test-roms --results test-results.txt --dump-dir target/test-output "$@"
