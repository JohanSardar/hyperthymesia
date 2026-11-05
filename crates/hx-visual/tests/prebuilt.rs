//! Real Vision, synthetic pixels, and a child process so PATH changes cannot race other tests.
#![cfg(target_os = "macos")]

use hx_visual::{OcrEngine, VisionCliEngine};
use std::{path::Path, process::Command};

#[test]
fn prebuilt_helper_works_with_empty_path() {
    let temp = tempfile::tempdir().unwrap();
    let helper = temp.path().join("hx-vision-ocr");
    let output = Command::new("swiftc")
        .arg("-O")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/swift/ocr.swift"))
        .arg("-o")
        .arg(&helper)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let script = temp.path().join("image.swift");
    let png = temp.path().join("synthetic.png");
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

    // Copy the test runner to exercise real current_exe() sibling discovery.
    let runner = temp.path().join("ocr-test");
    std::fs::copy(std::env::current_exe().unwrap(), &runner).unwrap();
    // A prebuilt helper must not even try to create a compiler cache.
    let cache = temp.path().join("compiler-cache");
    run_child(&runner, &png, &cache, None);

    // The explicit override takes precedence, even with an unusable sibling present.
    let override_helper = temp.path().join("override-ocr");
    std::fs::rename(&helper, &override_helper).unwrap();
    std::fs::write(&helper, b"not executable").unwrap();
    run_child(&runner, &png, &cache, Some(&override_helper));
    assert!(
        !cache.exists(),
        "prebuilt OCR must not create a compiler cache"
    );
}

fn run_child(runner: &Path, png: &Path, cache: &Path, helper: Option<&Path>) {
    let mut command = Command::new(runner);
    command
        .args(["--exact", "prebuilt_child", "--nocapture"])
        .env("PATH", "")
        .env("HX_VISION_TEST_IMAGE", png)
        .env("CARGO_TARGET_DIR", cache)
        .env_remove("HX_VISION_HELPER");
    if let Some(helper) = helper {
        command.env("HX_VISION_HELPER", helper);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prebuilt_child() {
    let Some(png) = std::env::var_os("HX_VISION_TEST_IMAGE") else {
        return;
    };
    assert_eq!(std::env::var_os("PATH").unwrap(), "");
    // Server startup must identify the engine without relying on PATH, too.
    assert!(hx_visual::vision_version().unwrap().starts_with("vision:"));
    let engine = VisionCliEngine::default();
    let result = engine
        .ocr(&std::fs::read(png).unwrap(), "image/png")
        .unwrap();
    assert_eq!(result.engine_name, "apple-vision");
    assert_eq!(result.coverage, "partial");
    assert!(
        result
            .lines
            .iter()
            .any(|line| line.text.contains("SYNTHETIC ORCHID 472")),
        "{result:?}"
    );
    assert!(engine.ocr(b"invalid image", "image/png").is_err());
}
