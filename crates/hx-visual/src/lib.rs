//! Shared OCR and sampling. Perceptual hashes are sampling hints, never cache identities.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub confidence: f32,
    /// Normalized Vision coordinates, origin bottom-left.
    pub bbox: BoundingBox,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
    pub engine_name: String,
    pub engine_version: String,
    /// An OCR result describes extracted text, not proof all visible text was recognized.
    pub coverage: String,
}

pub trait OcrEngine: Send + Sync {
    fn ocr(&self, bytes: &[u8], media_type: &str) -> Result<OcrResult>;
}
pub struct NullEngine;
impl OcrEngine for NullEngine {
    fn ocr(&self, _bytes: &[u8], _media_type: &str) -> Result<OcrResult> {
        Ok(OcrResult {
            lines: vec![],
            engine_name: "null".into(),
            engine_version: "1".into(),
            coverage: "unknown".into(),
        })
    }
}

/// Scope each cache instance to one engine/configuration; only successful exact hits are reused.
pub struct ExactHashCache<E> {
    engine: E,
    results: Mutex<HashMap<String, OcrResult>>,
}
impl<E: OcrEngine> ExactHashCache<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            results: Mutex::new(HashMap::new()),
        }
    }
}
impl<E: OcrEngine> OcrEngine for ExactHashCache<E> {
    fn ocr(&self, bytes: &[u8], media_type: &str) -> Result<OcrResult> {
        let hash = hex::encode(Sha256::digest(bytes));
        let mut cache = self
            .results
            .lock()
            .map_err(|_| anyhow::anyhow!("OCR cache poisoned"))?;
        if let Some(result) = cache.get(&hash) {
            return Ok(result.clone());
        }
        let result = self.engine.ocr(bytes, media_type)?;
        cache.insert(hash, result.clone());
        Ok(result)
    }
}

/// Average hash of a decoded 8x8 luminance thumbnail, for SAMPLING ONLY.
/// Callers decode/resize pixels first. Never deduplicate evidence with this hash.
pub fn perceptual_hash(luminance_8x8: &[u8; 64]) -> u64 {
    let sum: u32 = luminance_8x8.iter().map(|v| u32::from(*v)).sum();
    luminance_8x8
        .iter()
        .enumerate()
        .fold(0, |hash, (i, value)| {
            hash | (u64::from(u32::from(*value) * 64 >= sum) << i)
        })
}

/// Decode with bounded dimensions/allocation and hash the same luminance thumbnail
/// on the server and device. Never use this sampling hint as an evidence identity.
pub fn thumbnail_hash(bytes: &[u8]) -> Result<u64> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let thumb = reader
        .decode()?
        .resize_exact(8, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    Ok(perceptual_hash(thumb.as_raw().as_slice().try_into()?))
}

/// Sampling threshold for a prior completed still. Callers must first restrict the
/// candidate to the same device, display and app, within the preceding 30 seconds.
/// A match skips OCR; it must never borrow the prior still's text.
pub fn similar_thumbnail_hashes(current: u64, previous: u64) -> bool {
    (current ^ previous).count_ones() <= 3
}

/// Includes the helper source/config and OS build: Vision behavior can change with macOS.
pub fn vision_version() -> Result<String> {
    anyhow::ensure!(cfg!(target_os = "macos"), "Vision requires macOS");
    let os = Command::new("/usr/bin/sw_vers")
        .arg("-buildVersion")
        .output()?;
    anyhow::ensure!(os.status.success(), "cannot identify Vision OS build");
    Ok(format!(
        "vision:{}:{}",
        hex::encode(Sha256::digest(include_bytes!("../swift/ocr.swift"))),
        String::from_utf8(os.stdout)?.trim()
    ))
}

/// Uses `HX_VISION_HELPER`, then a sibling `hx-vision-ocr`, then a locally compiled
/// Swift helper. An invalid explicit override is an error, not a compiler fallback.
pub struct VisionCliEngine {
    target_dir: PathBuf,
}
static COMPILE_LOCK: Mutex<()> = Mutex::new(());
impl Default for VisionCliEngine {
    fn default() -> Self {
        Self::new(
            std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")),
        )
    }
}
impl VisionCliEngine {
    pub fn new(target_dir: impl Into<PathBuf>) -> Self {
        Self {
            target_dir: target_dir.into(),
        }
    }
    fn executable(&self) -> Result<PathBuf> {
        if !cfg!(target_os = "macos") {
            bail!("Apple Vision OCR requires macOS");
        }
        // An explicit override wins over the release/app bundle's sibling helper.
        if let Some(helper) = std::env::var_os("HX_VISION_HELPER") {
            let helper = PathBuf::from(helper);
            anyhow::ensure!(helper.is_file(), "HX_VISION_HELPER is not a file");
            return Ok(helper);
        }
        let helper = std::env::current_exe()?
            .canonicalize()?
            .with_file_name("hx-vision-ocr");
        if helper.is_file() {
            return Ok(helper);
        }
        // Development fallback only; releases do not require a Swift toolchain.
        let _lock = COMPILE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("Swift compiler lock poisoned"))?;
        let source = include_str!("../swift/ocr.swift");
        let digest = hex::encode(Sha256::digest(source.as_bytes()));
        let dir = self.target_dir.join("hx-visual");
        std::fs::create_dir_all(&dir)?;
        let binary = dir.join(format!("vision-ocr-{}", &digest[..16]));
        if !binary.is_file() {
            let source_path = dir.join(format!("ocr-{}.swift", &digest[..16]));
            std::fs::write(&source_path, source)?;
            let temporary = dir.join(format!("vision-ocr-{}.tmp", std::process::id()));
            let output = Command::new("swiftc")
                .args(["-O"])
                .arg(&source_path)
                .arg("-o")
                .arg(&temporary)
                .output()
                .context("launch swiftc")?;
            if !output.status.success() {
                bail!("swiftc failed: {}", String::from_utf8_lossy(&output.stderr));
            }
            std::fs::rename(temporary, &binary)?;
        }
        Ok(binary)
    }
}
impl OcrEngine for VisionCliEngine {
    fn ocr(&self, bytes: &[u8], media_type: &str) -> Result<OcrResult> {
        if !["image/png", "image/jpeg", "image/heic", "image/tiff"].contains(&media_type) {
            bail!("unsupported OCR media type");
        }
        let binary = self.executable()?;
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().context("OCR stdin")?.write_all(bytes)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "Vision OCR failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        serde_json::from_slice(&output.stdout).context("decode OCR result")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn thumbnail_sampling_preserves_pixels_and_decode_limits() {
        let pixels =
            image::GrayImage::from_fn(64, 64, |x, _| image::Luma([if x < 32 { 10 } else { 240 }]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(pixels)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(
            thumbnail_hash(png.get_ref()).unwrap(),
            0xf0f0_f0f0_f0f0_f0f0
        );
        assert!(thumbnail_hash(b"not an image").is_err());

        let mut oversized = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image::GrayImage::new(16385, 1))
            .write_to(&mut oversized, image::ImageFormat::Png)
            .unwrap();
        assert!(thumbnail_hash(oversized.get_ref()).is_err());
    }

    #[test]
    fn similarity_includes_three_bits_but_excludes_four() {
        let hash = 0xf0f0_f0f0_f0f0_f0f0;
        assert!(similar_thumbnail_hashes(hash, hash));
        assert!(similar_thumbnail_hashes(hash, hash ^ 0x8000_0000_0000_0003));
        assert!(similar_thumbnail_hashes(hash ^ 7, hash));
        assert!(!similar_thumbnail_hashes(
            hash,
            hash ^ 0x8000_0000_0000_0007
        ));
    }

    struct Counting(AtomicUsize);
    impl OcrEngine for Counting {
        fn ocr(&self, _: &[u8], _: &str) -> Result<OcrResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            NullEngine.ocr(&[], "")
        }
    }
    #[test]
    fn exact_cache_and_sampling() {
        let cache = ExactHashCache::new(Counting(AtomicUsize::new(0)));
        cache.ocr(b"a", "image/png").unwrap();
        cache.ocr(b"a", "image/png").unwrap();
        cache.ocr(b"b", "image/png").unwrap();
        assert_eq!(cache.engine.0.load(Ordering::SeqCst), 2);
        assert_eq!(perceptual_hash(&[0; 64]), perceptual_hash(&[255; 64]));
        assert_eq!(NullEngine.ocr(&[], "").unwrap().coverage, "unknown");
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn vision_reads_generated_png() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("image.swift");
        let png = temp.path().join("text.png");
        std::fs::write(&script, r#"import AppKit
let image = NSImage(size: NSSize(width: 900, height: 160))
image.lockFocus()
NSColor.white.setFill()
NSRect(x: 0, y: 0, width: 900, height: 160).fill()
("SYNTHETIC ORCHID 472" as NSString).draw(at: NSPoint(x: 30, y: 50), withAttributes: [.font: NSFont.systemFont(ofSize: 48), .foregroundColor: NSColor.black])
image.unlockFocus()
let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
"#).unwrap();
        let output = Command::new("swift")
            .arg(script)
            .arg(&png)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let engine = VisionCliEngine::default();
        let bytes = std::fs::read(png).unwrap();
        let result = engine.ocr(&bytes, "image/png").unwrap();
        assert!(
            result
                .lines
                .iter()
                .any(|line| line.text.contains("SYNTHETIC ORCHID 472")),
            "{result:?}"
        );
        assert!(
            result
                .lines
                .iter()
                .all(|line| (0.0..=1.0).contains(&line.confidence))
        );
        assert!(engine.ocr(b"invalid image", "image/png").is_err());
    }
}
