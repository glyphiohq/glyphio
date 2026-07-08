// glyphio-ocr — on-device text recognition for captures, via Apple's Vision framework.
// No network, no models to ship: recognition runs entirely locally, which is the only
// OCR consistent with Glyphio's privacy posture (captures never leave the device).
//
// Usage: glyphio-ocr <image-path>
// Output: JSON on stdout — { "lines": [ { "text", "x", "y", "w", "h" } ] } where the box is
// normalized to the image (0–1), top-left origin — ready for a selectable text overlay.
// Build: scripts/build-ocr.sh (compiled per-arch, bundled as a Tauri sidecar).

import Foundation
import Vision
import CoreImage

struct Line: Codable {
    let text: String
    let x: Double
    let y: Double
    let w: Double
    let h: Double
}

struct Output: Codable {
    let lines: [Line]
}

guard CommandLine.arguments.count > 1 else {
    FileHandle.standardError.write("usage: glyphio-ocr <image-path>\n".data(using: .utf8)!)
    exit(2)
}
let path = CommandLine.arguments[1]
guard let image = CIImage(contentsOf: URL(fileURLWithPath: path)) else {
    FileHandle.standardError.write("could not read image\n".data(using: .utf8)!)
    exit(1)
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
// Let Vision auto-detect languages; add revision pinning here if output must be stable.

let handler = VNImageRequestHandler(ciImage: image, options: [:])
do {
    try handler.perform([request])
} catch {
    FileHandle.standardError.write("recognition failed: \(error.localizedDescription)\n".data(using: .utf8)!)
    exit(1)
}

let observations = request.results ?? []
// Sort by vertical position (Vision's coordinates are bottom-left origin), then horizontal —
// gives natural reading order for screenshots.
let lines: [Line] = observations
    .sorted {
        let a = $0.boundingBox, b = $1.boundingBox
        return abs(a.midY - b.midY) > 0.01 ? a.midY > b.midY : a.minX < b.minX
    }
    .compactMap { obs in
        guard let text = obs.topCandidates(1).first?.string, !text.isEmpty else { return nil }
        let box = obs.boundingBox // normalized, bottom-left origin
        return Line(
            text: text,
            x: Double(box.minX),
            y: Double(1.0 - box.maxY), // flip to top-left origin
            w: Double(box.width),
            h: Double(box.height)
        )
    }

let encoder = JSONEncoder()
let data = try! encoder.encode(Output(lines: lines))
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
