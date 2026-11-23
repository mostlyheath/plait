//! Explicit, windowless probe of the pinned native clipboard reader.
//! This replaces the clipboard; save and restore it outside this process.
use gpui::{ClipboardEntry, ClipboardItem, Image, ImageFormat};
use std::time::Instant;

fn crc(bytes: &[u8]) -> u32 {
    let mut value = !0u32;
    for byte in bytes {
        value ^= u32::from(*byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(value & 1));
        }
    }
    !value
}
fn png(length: usize) -> Vec<u8> {
    let small = include_bytes!("../../../runtime/test/support/images/0.png");
    let mut bytes = small[..small.len() - 12].to_vec();
    let padding = length - small.len() - 12;
    bytes.extend_from_slice(&(padding as u32).to_be_bytes());
    let start = bytes.len();
    bytes.extend_from_slice(b"tEXt");
    bytes.extend_from_slice(b"padding\0");
    bytes.resize(bytes.len() + padding - 8, b'x');
    bytes.extend_from_slice(&crc(&bytes[start..]).to_be_bytes());
    bytes.extend_from_slice(&small[small.len() - 12..]);
    assert_eq!(bytes.len(), length);
    bytes
}
fn main() {
    // Construction without application.run creates no application or window.
    let platform = gpui_platform::current_platform(true);
    let expected = std::env::args().nth(2);
    if std::env::args().any(|arg| arg == "--read") {
        let result = platform.read_from_clipboard();
        if let Some(expected) = expected {
            let entries = &result.as_ref().expect("clipboard representation").entries;
            match expected.as_str() {
                "text" => assert!(matches!(entries.as_slice(), [ClipboardEntry::String(_)])),
                "png" | "jpeg" | "tiff" => {
                    let [ClipboardEntry::Image(image)] = entries.as_slice() else {
                        panic!("expected image")
                    };
                    assert_eq!(format!("{:?}", image.format).to_lowercase(), expected);
                    assert_eq!(
                        plait_client::images::describe(&image.bytes).is_ok(),
                        expected != "tiff"
                    );
                }
                _ => panic!("unknown expectation"),
            }
        }
        match result {
            Some(item) => {
                for entry in item.entries {
                    match entry {
                        ClipboardEntry::Image(image) => println!(
                            "image {:?}, {} bytes; validation: {:?}",
                            image.format,
                            image.bytes.len(),
                            plait_client::images::describe(&image.bytes)
                                .map(|image| image.media_type)
                        ),
                        ClipboardEntry::String(_) => println!("text representation"),
                        ClipboardEntry::ExternalPaths(paths) => {
                            println!("{} file paths", paths.paths().len())
                        }
                    }
                }
            }
            None => println!("no supported representation"),
        }
        return;
    }
    for bytes in [5_000_000, 20_000_000] {
        platform.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::Image(Image::from_bytes(
                ImageFormat::Png,
                png(bytes),
            ))],
        });
        let mut samples = Vec::new();
        for index in 0..35 {
            let start = Instant::now();
            let item = platform.read_from_clipboard().expect("clipboard");
            for entry in item.entries {
                if let ClipboardEntry::Image(image) = entry {
                    assert_eq!(image.bytes.len(), bytes);
                    assert_eq!(
                        plait_client::images::describe(&image.bytes).is_ok(),
                        bytes <= plait_client::images::IMAGE_BYTES
                    );
                }
            }
            if index >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{bytes} bytes PNG: read + validate + drop, 30 samples; min {:.3} ms, median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            samples[0], samples[14], samples[28], samples[29]
        );
    }
}
