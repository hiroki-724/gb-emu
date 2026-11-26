#!/usr/bin/env bash
# テストROMを test-roms/ 以下に取得する。
#
#   scripts/fetch-test-roms.sh          まだ無いものだけ取得する
#   scripts/fetch-test-roms.sh --force  すべて取得し直す
#
# 取得するもの（いずれも配布元が公開しているテスト用 ROM。市販ゲームの ROM は含まない）:
#   test-roms/blargg/     Blargg's test ROMs      https://github.com/retrio/gb-test-roms
#   test-roms/mooneye/    mooneye-test-suite      https://github.com/Gekkio/mooneye-test-suite（ビルド済み）
#   test-roms/dmg-acid2/  dmg-acid2 と比較用画像  https://github.com/mattcurrie/dmg-acid2
#
# 結果を再現できるよう、版を固定し SHA-256 で中身を確かめている。
set -euo pipefail

cd "$(dirname "$0")/.."
DEST="test-roms"

FORCE=0
if [[ "${1:-}" == "--force" ]]; then
  FORCE=1
elif [[ $# -gt 0 ]]; then
  echo "使い方: $0 [--force]" >&2
  exit 2
fi

BLARGG_COMMIT="c240dd7d700e5c0b00a7bbba52b53e4ee67b5f15"
BLARGG_URL="https://github.com/retrio/gb-test-roms/archive/${BLARGG_COMMIT}.tar.gz"
BLARGG_SHA256="1e121842bc380cc69ec4d5946335c0e6656926af8a794ccc732d1b293f29e411"

MOONEYE_VERSION="mts-20260714-0944-31510e1"
MOONEYE_URL="https://gekkio.fi/files/mooneye-test-suite/${MOONEYE_VERSION}/${MOONEYE_VERSION}.tar.xz"
MOONEYE_SHA256="6d4fdda2f1d8d2f5f51b0ff3f6f3cc2fbae047aa395a39c82bda3a0e7cbd2641"

ACID2_ROM_URL="https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb"
ACID2_ROM_SHA256="464e14b7d42e7feea0b7ede42be7071dc88913f75b9ffa444299424b63d1dff1"
ACID2_COMMIT="8a98ce731f96dde032ffb22ec36dc985d78fdb18"
ACID2_REF_URL="https://raw.githubusercontent.com/mattcurrie/dmg-acid2/${ACID2_COMMIT}/img/reference-dmg.png"
ACID2_REF_SHA256="ca966d50895c7efef05838590d148c2cbfd7fba57dab986f25b35b4da71abb57"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# download URL 保存先 期待するSHA-256
download() {
  local url="$1" out="$2" expected="$3" actual
  echo "  取得中: $url"
  curl --fail --location --silent --show-error --retry 3 --output "$out" "$url"
  actual="$(sha256_of "$out")"
  if [[ "$actual" != "$expected" ]]; then
    echo "SHA-256 が一致しません: $url" >&2
    echo "  期待: $expected" >&2
    echo "  実際: $actual" >&2
    exit 1
  fi
}

# アーカイブを展開し、最上位の 1 階層を取り除いて配置する
extract_to() {
  local archive="$1" target="$2"
  rm -rf "$target"
  mkdir -p "$target"
  tar -xf "$archive" -C "$target" --strip-components=1
}

mkdir -p "$DEST"

if [[ $FORCE -eq 1 || ! -f "$DEST/blargg/.version" || "$(cat "$DEST/blargg/.version")" != "$BLARGG_COMMIT" ]]; then
  echo "Blargg's test ROMs"
  download "$BLARGG_URL" "$TMP/blargg.tar.gz" "$BLARGG_SHA256"
  extract_to "$TMP/blargg.tar.gz" "$DEST/blargg"
  echo "$BLARGG_COMMIT" > "$DEST/blargg/.version"
else
  echo "Blargg's test ROMs: 取得済み"
fi

if [[ $FORCE -eq 1 || ! -f "$DEST/mooneye/.version" || "$(cat "$DEST/mooneye/.version")" != "$MOONEYE_VERSION" ]]; then
  echo "mooneye-test-suite"
  download "$MOONEYE_URL" "$TMP/mooneye.tar.xz" "$MOONEYE_SHA256"
  extract_to "$TMP/mooneye.tar.xz" "$DEST/mooneye"
  echo "$MOONEYE_VERSION" > "$DEST/mooneye/.version"
else
  echo "mooneye-test-suite: 取得済み"
fi

if [[ $FORCE -eq 1 || ! -f "$DEST/dmg-acid2/.version" || "$(cat "$DEST/dmg-acid2/.version")" != "$ACID2_COMMIT" ]]; then
  echo "dmg-acid2"
  download "$ACID2_ROM_URL" "$TMP/dmg-acid2.gb" "$ACID2_ROM_SHA256"
  download "$ACID2_REF_URL" "$TMP/reference-dmg.png" "$ACID2_REF_SHA256"
  rm -rf "$DEST/dmg-acid2"
  mkdir -p "$DEST/dmg-acid2"
  mv "$TMP/dmg-acid2.gb" "$TMP/reference-dmg.png" "$DEST/dmg-acid2/"
  echo "$ACID2_COMMIT" > "$DEST/dmg-acid2/.version"
else
  echo "dmg-acid2: 取得済み"
fi

echo "完了: $(find "$DEST" -name '*.gb' | wc -l | tr -d ' ') 本の ROM が $DEST/ にあります"
