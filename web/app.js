// ブラウザ用フロントエンド。エミュレータ本体は WebAssembly（pkg/ 以下、wasm-pack が生成）。

import init, { Emulator, FramePacer } from "./pkg/gb_web.js";

const WIDTH = 160;
const HEIGHT = 144;

/** ボタンのビット（Emulator.set_buttons と同じ並び） */
const BIT = { right: 1, left: 2, up: 4, down: 8, a: 16, b: 32, select: 64, start: 128 };

const KEY_BITS = {
  ArrowRight: BIT.right,
  ArrowLeft: BIT.left,
  ArrowUp: BIT.up,
  ArrowDown: BIT.down,
  KeyX: BIT.a,
  KeyZ: BIT.b,
  Enter: BIT.start,
  ShiftLeft: BIT.select,
  ShiftRight: BIT.select,
};

/** 配色。それぞれ明るい順に 4 色 */
const PALETTES = [
  { id: "green", name: "みどり", colors: ["#e8f0d8", "#a0b880", "#507050", "#182820"] },
  { id: "gray", name: "グレー", colors: ["#ffffff", "#aaaaaa", "#555555", "#000000"] },
  { id: "olive", name: "オリーブ", colors: ["#c9cfa4", "#8f966b", "#4d543a", "#1a1e15"] },
  { id: "amber", name: "アンバー", colors: ["#fff1d0", "#e2a655", "#8b5024", "#2b1608"] },
];

const SETTINGS_KEY = "gb-emu:settings";
const SAVE_PREFIX = "gb-emu:save:";

const $ = (id) => document.getElementById(id);
const canvas = $("screen");
const context = canvas.getContext("2d", { alpha: false });
const imageData = context.createImageData(WIDTH, HEIGHT);
// wasm 側には Uint8Array として渡す（中身は imageData と同じメモリ）
const pixels = new Uint8Array(imageData.data.buffer);

const state = {
  emulator: null,
  romBytes: null,
  romName: "",
  keyboardBits: 0,
  touchBits: 0,
  rafId: 0,
  lastTime: 0,
  /** 画面の更新 1 回で何フレーム進めるかを決める（wasm 側。init の後で作る） */
  pacer: null,
  framesInWindow: 0,
  windowStart: 0,
  lastSaveCheck: 0,
  /** pad: 画面上のボタンを出すか（auto = タッチできる端末でだけ出す） */
  settings: { palette: "green", showFps: false, pad: "auto" },
};

// ---- 設定 ----

function loadSettings() {
  try {
    Object.assign(state.settings, JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}"));
  } catch {
    // 壊れていたら既定値のまま使う
  }
  if (!PALETTES.some((palette) => palette.id === state.settings.palette)) {
    state.settings.palette = PALETTES[0].id;
  }
  if (!["auto", "show", "hide"].includes(state.settings.pad)) {
    state.settings.pad = "auto";
  }
}

function applyPadVisibility() {
  const touch = matchMedia("(pointer: coarse)").matches || navigator.maxTouchPoints > 0;
  const mode = state.settings.pad;
  const show = mode === "show" || (mode !== "hide" && touch);
  document.documentElement.dataset.pad = show ? "show" : "hide";
  fitScreen();
}

function storeSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(state.settings));
  } catch {
    // 保存できなくても動作には支障がない
  }
}

function currentPalette() {
  return PALETTES.find((palette) => palette.id === state.settings.palette) ?? PALETTES[0];
}

function applyPalette() {
  const palette = currentPalette();
  canvas.style.background = palette.colors[0];
  if (!state.emulator) return;
  const rgb = new Uint8Array(12);
  palette.colors.forEach((color, index) => {
    const value = parseInt(color.slice(1), 16);
    rgb.set([value >> 16, (value >> 8) & 0xff, value & 0xff], index * 3);
  });
  state.emulator.set_palette(rgb);
  draw();
}

function buildPaletteOptions() {
  const container = $("palette-options");
  for (const palette of PALETTES) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "palette-option";
    button.dataset.palette = palette.id;
    const swatch = document.createElement("span");
    swatch.className = "palette-swatch";
    for (const color of palette.colors) {
      const chip = document.createElement("i");
      chip.style.background = color;
      swatch.append(chip);
    }
    button.append(swatch, palette.name);
    button.addEventListener("click", () => {
      state.settings.palette = palette.id;
      storeSettings();
      applyPalette();
      refreshPaletteOptions();
    });
    container.append(button);
  }
  refreshPaletteOptions();
}

function refreshPaletteOptions() {
  for (const button of $("palette-options").children) {
    button.setAttribute("aria-pressed", String(button.dataset.palette === state.settings.palette));
  }
}

// ---- 通知 ----

let toastTimer = 0;
function showToast(message) {
  const toast = $("toast");
  toast.textContent = message;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toast.hidden = true;
  }, 3200);
}

// ---- 画面 ----

/** 端末のピクセルで整数倍になる最大の大きさに合わせる */
function fitScreen() {
  const stage = $("stage");
  const style = getComputedStyle(stage);
  const framePadding = 12;
  const availableWidth =
    stage.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - framePadding;
  const availableHeight =
    stage.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) - framePadding;
  const ratio = window.devicePixelRatio || 1;
  const scale = Math.max(
    1,
    Math.floor(Math.min((availableWidth * ratio) / WIDTH, (availableHeight * ratio) / HEIGHT)),
  );
  canvas.style.width = `${(WIDTH * scale) / ratio}px`;
  canvas.style.height = `${(HEIGHT * scale) / ratio}px`;
}

function draw() {
  if (!state.emulator) return;
  state.emulator.render(pixels);
  context.putImageData(imageData, 0, 0);
}

// ---- メインループ ----

function frame(now) {
  state.rafId = requestAnimationFrame(frame);
  const emulator = state.emulator;
  if (!emulator) return;

  const delta = now - state.lastTime;
  state.lastTime = now;
  if (delta <= 0) return;

  const frames = state.pacer.frames_for(delta);
  for (let i = 0; i < frames; i++) {
    emulator.run_frame();
  }
  if (frames > 0) draw();

  state.framesInWindow += frames;
  if (now - state.windowStart >= 1000) {
    if (state.settings.showFps) {
      const fps = (state.framesInWindow * 1000) / (now - state.windowStart);
      $("fps").textContent = `${fps.toFixed(1)} fps`;
    }
    state.framesInWindow = 0;
    state.windowStart = now;
  }

  if (now - state.lastSaveCheck >= 1000) {
    state.lastSaveCheck = now;
    persistSave(false);
  }
}

function start() {
  if (state.rafId || !state.emulator || document.hidden || $("menu").open) return;
  const now = performance.now();
  state.lastTime = now;
  state.windowStart = now;
  state.framesInWindow = 0;
  state.pacer.reset();
  state.rafId = requestAnimationFrame(frame);
}

function stop() {
  cancelAnimationFrame(state.rafId);
  state.rafId = 0;
}

// ---- 入力 ----

function sendButtons() {
  const bits = state.keyboardBits | state.touchBits;
  state.emulator?.set_buttons(bits);
  for (const element of document.querySelectorAll("[data-bit]")) {
    element.classList.toggle("pressed", (bits & Number(element.dataset.bit)) !== 0);
  }
}

function setupKeyboard() {
  window.addEventListener("keydown", (event) => {
    const bit = KEY_BITS[event.code];
    if (!bit || event.metaKey || event.ctrlKey || event.altKey || $("menu").open) return;
    event.preventDefault();
    state.keyboardBits |= bit;
    sendButtons();
  });
  window.addEventListener("keyup", (event) => {
    const bit = KEY_BITS[event.code];
    if (!bit) return;
    state.keyboardBits &= ~bit;
    sendButtons();
  });
  window.addEventListener("blur", () => {
    state.keyboardBits = 0;
    sendButtons();
  });
}

/** 十字キー: 中心からの向きで決める。上下左右は各 60 度、斜めは各 30 度 */
function dpadBits(element, x, y) {
  const rect = element.getBoundingClientRect();
  const dx = x - (rect.left + rect.width / 2);
  const dy = y - (rect.top + rect.height / 2);
  const distance = Math.hypot(dx, dy);
  if (distance < rect.width * 0.1) return 0;
  const threshold = distance * 0.5;
  let bits = 0;
  if (dx > threshold) bits |= BIT.right;
  if (dx < -threshold) bits |= BIT.left;
  if (dy > threshold) bits |= BIT.down;
  if (dy < -threshold) bits |= BIT.up;
  return bits;
}

/** A・B: ボタンの中心からの距離で決める。2 つの間に指を置くと同時押しになる */
function roundButtonBits(element, x, y) {
  let bits = 0;
  for (const button of element.children) {
    const rect = button.getBoundingClientRect();
    const radius = rect.width / 2;
    const distance = Math.hypot(x - (rect.left + radius), y - (rect.top + radius));
    if (distance < radius * 1.64) bits |= Number(button.dataset.bit);
  }
  return bits;
}

function pillBits(element, x, y) {
  const margin = 10;
  let bits = 0;
  for (const pill of element.children) {
    const rect = pill.getBoundingClientRect();
    const inside =
      x >= rect.left - margin &&
      x <= rect.right + margin &&
      y >= rect.top - margin &&
      y <= rect.bottom + margin;
    if (inside) bits |= Number(pill.dataset.bit);
  }
  return bits;
}

function setupTouch() {
  /** 触れている指ごとの、押しているボタン */
  const pointers = new Map();
  const groups = [
    [$("dpad"), dpadBits],
    [$("ab"), roundButtonBits],
    [$("pills"), pillBits],
  ];

  const refresh = () => {
    let bits = 0;
    for (const value of pointers.values()) bits |= value;
    const added = bits & ~state.touchBits;
    state.touchBits = bits;
    sendButtons();
    // 押した瞬間に短く振動させる（対応端末のみ）
    if (added && navigator.vibrate) navigator.vibrate(8);
  };

  for (const [element, bitsAt] of groups) {
    const update = (event) => {
      pointers.set(event.pointerId, bitsAt(element, event.clientX, event.clientY));
      refresh();
    };
    const release = (event) => {
      if (pointers.delete(event.pointerId)) refresh();
    };
    element.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      try {
        // 指がボタンの外へ滑っても追い続ける
        element.setPointerCapture(event.pointerId);
      } catch {
        // 捕まえられなくても、ボタンの上にある間は反応する
      }
      update(event);
    });
    element.addEventListener("pointermove", (event) => {
      if (pointers.has(event.pointerId)) update(event);
    });
    element.addEventListener("pointerup", release);
    element.addEventListener("pointercancel", release);
    element.addEventListener("lostpointercapture", release);
    element.addEventListener("contextmenu", (event) => event.preventDefault());
  }
}

// ---- セーブデータ ----

function toBase64(bytes) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

function fromBase64(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

const unixTime = () => Date.now() / 1000;

function saveKey() {
  return SAVE_PREFIX + state.emulator.save_key();
}

/** セーブデータを localStorage に書く。force が false のときは変更があった場合だけ */
function persistSave(force) {
  const emulator = state.emulator;
  if (!emulator || !emulator.has_battery()) return;
  const dirty = emulator.take_save_dirty();
  if (!dirty && !force) return;
  const data = emulator.save_data(unixTime());
  if (!data) return;
  try {
    localStorage.setItem(saveKey(), toBase64(data));
  } catch {
    showToast("セーブデータを保存できませんでした（端末の保存領域が足りません）");
  }
}

function restoreSave() {
  const emulator = state.emulator;
  if (!emulator || !emulator.has_battery()) return;
  try {
    const stored = localStorage.getItem(saveKey());
    if (stored) emulator.load_save_data(fromBase64(stored), unixTime());
  } catch {
    showToast("セーブデータを読み込めませんでした");
  }
}

function exportSave() {
  const emulator = state.emulator;
  const data = emulator?.has_battery() ? emulator.save_data(unixTime()) : null;
  if (!data) {
    showToast("このゲームにはセーブデータがありません");
    return;
  }
  const link = document.createElement("a");
  link.href = URL.createObjectURL(new Blob([data], { type: "application/octet-stream" }));
  link.download = `${state.romName.replace(/\.[^.]+$/, "") || "game"}.sav`;
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 10_000);
}

async function importSave(file) {
  if (!state.emulator?.has_battery()) {
    showToast("このゲームはセーブに対応していません");
    return;
  }
  const bytes = new Uint8Array(await file.arrayBuffer());
  try {
    localStorage.setItem(saveKey(), toBase64(bytes));
  } catch {
    showToast("セーブデータを保存できませんでした（端末の保存領域が足りません）");
    return;
  }
  // 読み込んだデータで電源を入れ直す
  boot(state.romBytes, state.romName, { keepSave: false });
  showToast("セーブデータを読み込みました");
}

// ---- 前回の ROM を端末に覚えておく（IndexedDB。外には送らない） ----

function openDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("gb-emu", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("roms");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function withStore(mode, action) {
  const database = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const transaction = database.transaction("roms", mode);
      const request = action(transaction.objectStore("roms"));
      transaction.oncomplete = () => resolve(request.result);
      transaction.onerror = () => reject(transaction.error);
      transaction.onabort = () => reject(transaction.error);
    });
  } finally {
    database.close();
  }
}

async function rememberRom(bytes, name) {
  try {
    await withStore("readwrite", (store) => store.put({ name, bytes }, "last"));
  } catch {
    // 覚えられなくても遊ぶのには支障がない（プライベートブラウズなど）
  }
}

async function recallRom() {
  try {
    return (await withStore("readonly", (store) => store.get("last"))) ?? null;
  } catch {
    return null;
  }
}

async function forgetRom() {
  try {
    await withStore("readwrite", (store) => store.delete("last"));
  } catch {
    // 何も覚えていない状態と同じ
  }
}

// ---- ROM の起動 ----

/**
 * ROM を起動する。
 * keepSave: いま動いているゲームのセーブを書き出してから切り替えるか
 */
function boot(bytes, name, { keepSave = true } = {}) {
  let emulator;
  try {
    emulator = new Emulator(bytes);
  } catch (error) {
    showToast(`この ROM は開けません（${error?.message ?? error}）`);
    return false;
  }

  if (state.emulator) {
    if (keepSave) persistSave(true);
    state.emulator.free();
  }
  state.emulator = emulator;
  state.romBytes = bytes;
  state.romName = name;

  restoreSave();
  applyPalette();
  sendButtons();
  $("rom-title").textContent = emulator.title() || name;
  $("welcome").hidden = true;
  if (bytes[0x143] === 0xc0) {
    showToast("この ROM はゲームボーイカラー専用のため、正しく動きません");
  }
  stop();
  start();
  return true;
}

async function openFile(file) {
  if (!file) return;
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (boot(bytes, file.name)) rememberRom(bytes, file.name);
}

// ---- 画面まわりの組み立て ----

function setupInterface() {
  const romInput = $("rom-input");
  const pickRom = () => {
    romInput.value = "";
    romInput.click();
  };
  $("open-button").addEventListener("click", pickRom);
  $("welcome-open").addEventListener("click", pickRom);
  romInput.addEventListener("change", () => openFile(romInput.files[0]));

  // パソコンではファイルを画面に落としても開ける
  window.addEventListener("dragover", (event) => event.preventDefault());
  window.addEventListener("drop", (event) => {
    event.preventDefault();
    openFile(event.dataTransfer?.files[0]);
  });

  const menu = $("menu");
  $("menu-button").addEventListener("click", () => {
    const hasGame = Boolean(state.emulator);
    $("reset-button").disabled = !hasGame;
    $("export-save-button").disabled = !hasGame;
    $("import-save-button").disabled = !hasGame;
    stop();
    menu.showModal();
  });
  menu.addEventListener("close", start);

  const fpsToggle = $("fps-toggle");
  fpsToggle.checked = state.settings.showFps;
  $("fps").hidden = !state.settings.showFps;
  fpsToggle.addEventListener("change", () => {
    state.settings.showFps = fpsToggle.checked;
    $("fps").hidden = !fpsToggle.checked;
    $("fps").textContent = "";
    storeSettings();
  });

  const padSelect = $("pad-select");
  padSelect.value = state.settings.pad;
  padSelect.addEventListener("change", () => {
    state.settings.pad = padSelect.value;
    storeSettings();
    applyPadVisibility();
  });

  $("reset-button").addEventListener("click", () => {
    menu.close();
    if (state.romBytes) boot(state.romBytes, state.romName);
  });

  const fullscreenButton = $("fullscreen-button");
  if (document.documentElement.requestFullscreen) {
    fullscreenButton.hidden = false;
    fullscreenButton.addEventListener("click", () => {
      menu.close();
      if (document.fullscreenElement) document.exitFullscreen();
      else document.documentElement.requestFullscreen().catch(() => {});
    });
  }

  $("export-save-button").addEventListener("click", exportSave);
  const saveInput = $("save-input");
  $("import-save-button").addEventListener("click", () => {
    saveInput.value = "";
    saveInput.click();
  });
  saveInput.addEventListener("change", () => {
    menu.close();
    if (saveInput.files[0]) importSave(saveInput.files[0]);
  });

  $("forget-rom-button").addEventListener("click", async () => {
    await forgetRom();
    $("resume-button").hidden = true;
    showToast("覚えていた ROM を消しました");
  });

  // 画面を離れるとき: セーブを書き出して止める。戻ってきたら再開
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      persistSave(true);
      stop();
    } else {
      start();
    }
  });
  window.addEventListener("pagehide", () => persistSave(true));

  new ResizeObserver(fitScreen).observe($("stage"));
  window.addEventListener("resize", fitScreen);
  applyPadVisibility();
}

async function offerResume() {
  const last = await recallRom();
  if (!last || state.emulator) return;
  const button = $("resume-button");
  button.textContent = `つづきから：${last.name}`;
  button.hidden = false;
  button.addEventListener("click", () => boot(new Uint8Array(last.bytes), last.name), { once: true });
}

async function main() {
  loadSettings();
  buildPaletteOptions();
  applyPalette();
  setupInterface();
  setupKeyboard();
  setupTouch();
  try {
    await init();
    state.pacer = new FramePacer();
  } catch (error) {
    showToast(`エミュレータ本体を読み込めませんでした（${error?.message ?? error}）`);
    return;
  }
  offerResume();
}

// 動作確認用: URL に ?debug を付けると、開発者ツールから中を触れる
if (new URLSearchParams(location.search).has("debug")) {
  globalThis.gbDebug = { state, boot, draw, frame, persistSave, applyPadVisibility };
}

main();
