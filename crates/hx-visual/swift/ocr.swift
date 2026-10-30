import Foundation
import Vision
import ImageIO

struct Box: Codable { let x: Float; let y: Float; let width: Float; let height: Float }
struct Line: Codable { let text: String; let confidence: Float; let bbox: Box }
struct Result: Codable { let lines: [Line]; let engine_name: String; let engine_version: String; let coverage: String }

do {
    let bytes = FileHandle.standardInput.readDataToEndOfFile()
    guard let source = CGImageSourceCreateWithData(bytes as CFData, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        throw NSError(domain: "hx-visual", code: 1, userInfo: [NSLocalizedDescriptionKey: "Cannot decode image"])
    }
    let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
    let orientation = CGImagePropertyOrientation(rawValue: (properties?[kCGImagePropertyOrientation] as? UInt32) ?? 1) ?? .up
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:]).perform([request])
    let lines = (request.results ?? []).compactMap { observation -> Line? in
        guard let candidate = observation.topCandidates(1).first else { return nil }
        let b = observation.boundingBox
        return Line(text: candidate.string, confidence: candidate.confidence,
                    bbox: Box(x: Float(b.minX), y: Float(b.minY), width: Float(b.width), height: Float(b.height)))
    }
    let result = Result(lines: lines, engine_name: "apple-vision", engine_version: "revision-\(request.revision)/macOS-\(ProcessInfo.processInfo.operatingSystemVersionString)", coverage: "partial")
    FileHandle.standardOutput.write(try JSONEncoder().encode(result))
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(1)
}
