// Generates the Glyphio menu-bar (tray) template icon.
//
// A macOS template image is BLACK + ALPHA only; the system recolors it for light/dark menu
// bars automatically. The mark combines Glyphio's two jobs: capture crop-brackets framing a
// text I-beam caret (screenshot capture + text expansion).
//
// Usage: swift scripts/gen-tray-icon.swift src-tauri/icons/tray.png
// Rendered at 2x menu-bar size (44px) so Retina downscaling stays crisp.

import AppKit

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "tray.png"
let S = 44                      // 22pt menu bar @2x
let s = CGFloat(S)

let ctx = CGContext(
    data: nil, width: S, height: S, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!

// Everything black; the menu bar tints it. Alpha carries the shape.
let black = CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 1)
ctx.setStrokeColor(black)
ctx.setFillColor(black)
ctx.setLineCap(.round)
ctx.setLineJoin(.round)

let inset: CGFloat = s * 0.16
let box = CGRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset)
let arm = box.width * 0.28          // crop-bracket arm length
let lw = s * 0.085                  // stroke weight
ctx.setLineWidth(lw)

// Four corner crop brackets (the capture frame).
func bracket(_ corner: CGPoint, _ dx: CGFloat, _ dy: CGFloat) {
    ctx.beginPath()
    ctx.move(to: CGPoint(x: corner.x + dx * arm, y: corner.y))
    ctx.addLine(to: corner)
    ctx.addLine(to: CGPoint(x: corner.x, y: corner.y + dy * arm))
    ctx.strokePath()
}
bracket(CGPoint(x: box.minX, y: box.minY),  1,  1)  // bottom-left
bracket(CGPoint(x: box.maxX, y: box.minY), -1,  1)  // bottom-right
bracket(CGPoint(x: box.minX, y: box.maxY),  1, -1)  // top-left
bracket(CGPoint(x: box.maxX, y: box.maxY), -1, -1)  // top-right

// Centered text I-beam caret (the expansion).
let cx = box.midX
let capH = box.height * 0.44
let capW = box.width * 0.20
let capLw = s * 0.072
ctx.setLineWidth(capLw)
// vertical stem
ctx.beginPath()
ctx.move(to: CGPoint(x: cx, y: box.midY - capH / 2))
ctx.addLine(to: CGPoint(x: cx, y: box.midY + capH / 2))
ctx.strokePath()
// top + bottom serifs
for y in [box.midY - capH / 2, box.midY + capH / 2] {
    ctx.beginPath()
    ctx.move(to: CGPoint(x: cx - capW / 2, y: y))
    ctx.addLine(to: CGPoint(x: cx + capW / 2, y: y))
    ctx.strokePath()
}

let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
let png = rep.representation(using: .png, properties: [:])!
try! png.write(to: URL(fileURLWithPath: out))
FileHandle.standardError.write("wrote \(out) (\(S)x\(S))\n".data(using: .utf8)!)
