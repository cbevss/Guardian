//! Screen capture. Real capture on Windows (xcap); a moving test pattern
//! elsewhere so the pipeline can be tested on any machine. Linux/Raspberry Pi
//! capture arrives in milestone 4.
use serde_json::{json, Value};

fn encode(rgb: &image::RgbImage, quality: u8) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality).encode_image(rgb).ok()?;
    Some(buf)
}

#[cfg(windows)]
pub fn list_monitors() -> Value {
    let mons = xcap::Monitor::all().unwrap_or_default();
    json!(mons.iter().enumerate().map(|(i, m)| json!({
        "index": i, "name": m.name(), "width": m.width(), "height": m.height(), "primary": m.is_primary(),
    })).collect::<Vec<_>>())
}

#[cfg(windows)]
pub fn capture_jpeg(index: usize, quality: u8) -> Option<Vec<u8>> {
    let mons = xcap::Monitor::all().ok()?;
    let m = mons.get(index).or_else(|| mons.first())?;
    let img = m.capture_image().ok()?;
    encode(&image::DynamicImage::ImageRgba8(img).to_rgb8(), quality)
}

#[cfg(not(windows))]
pub fn list_monitors() -> Value {
    json!([
        {"index":0,"name":"Test screen A","width":1280,"height":720,"primary":true},
        {"index":1,"name":"Test screen B","width":1024,"height":768,"primary":false}
    ])
}

#[cfg(not(windows))]
pub fn capture_jpeg(index: usize, quality: u8) -> Option<Vec<u8>> {
    let (w, h) = if index == 1 { (1024, 768) } else { (1280, 720) };
    let t = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() / 20) as u32;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let bar = ((x + t) / 80) % 2 == 0;
        if index == 1 { image::Rgb([40, if bar { 160 } else { 90 }, 60]) }
        else { image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, if bar { 200 } else { 60 }]) }
    });
    encode(&img, quality)
}

/// Monitor position and size in desktop coordinates: (x, y, width, height).
#[cfg(windows)]
pub fn monitor_rect(index: usize) -> Option<(i32, i32, u32, u32)> {
    let mons = xcap::Monitor::all().ok()?;
    let m = mons.get(index).or_else(|| mons.first())?;
    Some((m.x(), m.y(), m.width(), m.height()))
}

#[cfg(not(windows))]
pub fn monitor_rect(index: usize) -> Option<(i32, i32, u32, u32)> {
    Some(if index == 1 { (1280, 0, 1024, 768) } else { (0, 0, 1280, 720) })
}
