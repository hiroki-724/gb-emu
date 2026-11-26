// 音声の再生側（AudioWorklet）。
// メインスレッドから届いたステレオのサンプルをリングバッファに溜め、音声出力の要求に応じて渡す。
// バッファの残量を定期的に報告し、メインスレッドはそれを見てエミュレータ側の生成レートを微調整する。

class GbAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { targetFrames } = options.processorOptions;
    /** ためておきたい量（フレーム数）。これが再生の遅れになる */
    this.targetFrames = targetFrames;
    /** これを超えたら古い音を捨てて追いつく */
    this.maxFrames = targetFrames * 4;
    this.capacity = targetFrames * 8;
    this.ring = new Float32Array(this.capacity * 2);
    this.readIndex = 0;
    this.writeIndex = 0;
    this.count = 0;
    /** 目標量まで溜まるのを待っている間は無音を出す */
    this.buffering = true;
    this.blocks = 0;

    this.port.onmessage = (event) => this.push(event.data);
  }

  push(samples) {
    const frames = samples.length >> 1;
    for (let i = 0; i < frames; i++) {
      if (this.count === this.capacity) {
        // あふれたら最も古いものを上書きする
        this.readIndex = (this.readIndex + 1) % this.capacity;
        this.count--;
      }
      this.ring[this.writeIndex * 2] = samples[i * 2];
      this.ring[this.writeIndex * 2 + 1] = samples[i * 2 + 1];
      this.writeIndex = (this.writeIndex + 1) % this.capacity;
      this.count++;
    }
    if (this.count > this.maxFrames) {
      // 一時停止からの復帰などで溜まりすぎた。目標量まで読み飛ばす
      const skip = this.count - this.targetFrames;
      this.readIndex = (this.readIndex + skip) % this.capacity;
      this.count -= skip;
    }
  }

  process(inputs, outputs) {
    const output = outputs[0];
    const left = output[0];
    const right = output[1] ?? output[0];

    if (this.buffering && this.count >= this.targetFrames) {
      this.buffering = false;
    }
    if (!this.buffering) {
      for (let i = 0; i < left.length; i++) {
        if (this.count === 0) {
          // 足りなくなった。残りは無音にして、また溜まるのを待つ
          this.buffering = true;
          break;
        }
        left[i] = this.ring[this.readIndex * 2];
        right[i] = this.ring[this.readIndex * 2 + 1];
        this.readIndex = (this.readIndex + 1) % this.capacity;
        this.count--;
      }
    }

    if (++this.blocks % 8 === 0) {
      this.port.postMessage(this.count);
    }
    return true;
  }
}

registerProcessor("gb-audio", GbAudioProcessor);
