//! PNG の読み書き（スクリーンショットと、リファレンス画像との比較用）。

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use gb_core::{FRAMEBUFFER_SIZE, SCREEN_HEIGHT, SCREEN_WIDTH};

/// 濃淡値 0〜3 に対応するグレー（0 = 白）
const GRAYS: [u8; 4] = [0xFF, 0xAA, 0x55, 0x00];

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// フレームバッファを 8bit グレースケールの PNG として保存する
pub fn write_screenshot(path: &Path, framebuffer: &[u8; FRAMEBUFFER_SIZE]) -> Result<(), BoxError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let pixels: Vec<u8> = framebuffer
        .iter()
        .map(|&shade| GRAYS[shade as usize & 3])
        .collect();
    writer.write_image_data(&pixels)?;
    Ok(())
}

/// 160×144 の PNG を読み、濃淡値 0〜3 の列に変換する。
/// 4 階調（0x00 / 0x55 / 0xAA / 0xFF）以外の色が含まれていたらエラーにする
pub fn read_reference(path: &Path) -> Result<Vec<u8>, BoxError> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    // パレット画像や 1〜4bit のグレースケールも 8bit に展開して読む
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;
    if info.width as usize != SCREEN_WIDTH || info.height as usize != SCREEN_HEIGHT {
        return Err(format!(
            "画像サイズが {}×{} です（160×144 が必要）",
            info.width, info.height
        )
        .into());
    }
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("パレット画像を展開できませんでした".into()),
    };
    buffer[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|pixel| match pixel[0] {
            0xFF => Ok(0),
            0xAA => Ok(1),
            0x55 => Ok(2),
            0x00 => Ok(3),
            other => Err(format!("想定外の色 0x{other:02X} が含まれています").into()),
        })
        .collect()
}
