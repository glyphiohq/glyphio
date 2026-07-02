// Generates the Glyphio app icon source (1024x1024 PNG).
// Design: "Ink & Brass" — an ink rounded square with capture crop-marks in the
// corners and a lowercase "g" + text-cursor monogram (screenshot capture + text expansion).
// Usage: swift scripts/gen-icon.swift src-tauri/icons/source.png
// Then regenerate the full set: npx tauri icon src-tauri/icons/source.png

import AppKit
import CoreText

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "source.png"
let S: CGFloat = 1024

let ctx = CGContext(
    data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!

func rgba(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}

// ---- background: rounded square (82% of canvas, macOS icon-grid margin), slate gradient ----
let inset: CGFloat = S * 0.09
let bg = CGRect(x: inset, y: inset, width: S - 2 * inset, height: S - 2 * inset)
let bgPath = CGPath(roundedRect: bg, cornerWidth: S * 0.185, cornerHeight: S * 0.185, transform: nil)

ctx.saveGState()
ctx.addPath(bgPath)
ctx.clip()
let grad = CGGradient(
    colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
    colors: [rgba(0x22252C), rgba(0x0B0C0F)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(grad, start: CGPoint(x: S / 2, y: S), end: CGPoint(x: S / 2, y: 0), options: [])
ctx.restoreGState()

// subtle top edge highlight
ctx.saveGState()
ctx.addPath(bgPath)
ctx.setLineWidth(6)
ctx.setStrokeColor(rgba(0xFFFFFF, 0.08))
ctx.strokePath()
ctx.restoreGState()

// ---- crop-mark corner brackets ----
let accent = rgba(0xD9A54A)
let mInset: CGFloat = S * 0.215        // bracket inset from canvas edge
let arm: CGFloat = S * 0.085           // bracket arm length
let bw: CGFloat = S * 0.030            // bracket stroke width
ctx.setLineWidth(bw)
ctx.setLineCap(.round)
ctx.setStrokeColor(accent)
for (cx, cy, dx, dy) in [
    (mInset, mInset, 1.0, 1.0), (S - mInset, mInset, -1.0, 1.0),
    (mInset, S - mInset, 1.0, -1.0), (S - mInset, S - mInset, -1.0, -1.0),
] {
    ctx.beginPath()
    ctx.move(to: CGPoint(x: cx + CGFloat(dx) * arm, y: cy))
    ctx.addLine(to: CGPoint(x: cx, y: cy))
    ctx.addLine(to: CGPoint(x: cx, y: cy + CGFloat(dy) * arm))
    ctx.strokePath()
}

// ---- monogram: lowercase "g" + text cursor ----
let fontSize: CGFloat = S * 0.46
let font = NSFont.systemFont(ofSize: fontSize, weight: .bold)
let attr = [
    NSAttributedString.Key.font: font,
    NSAttributedString.Key.foregroundColor: NSColor.white,
] as [NSAttributedString.Key: Any]
let line = CTLineCreateWithAttributedString(NSAttributedString(string: "g", attributes: attr))
let bounds = CTLineGetImageBounds(line, ctx)
let cursorW: CGFloat = S * 0.028
let cursorGap: CGFloat = S * 0.05
let totalW = bounds.width + cursorGap + cursorW
let gX = (S - totalW) / 2 - bounds.minX
let gY = (S - bounds.height) / 2 - bounds.minY + S * 0.02
ctx.textPosition = CGPoint(x: gX, y: gY)
CTLineDraw(line, ctx)

// cursor bar (text-insertion caret), x-height sized, accent color
let caretH: CGFloat = bounds.height * 1.05
let caretX = gX + bounds.maxX + cursorGap
let caretY = gY + bounds.minY + (bounds.height - caretH) * 0.15
let caret = CGPath(
    roundedRect: CGRect(x: caretX, y: caretY, width: cursorW, height: caretH),
    cornerWidth: cursorW / 2, cornerHeight: cursorW / 2, transform: nil)
ctx.setFillColor(accent)
ctx.addPath(caret)
ctx.fillPath()

// ---- write PNG ----
let image = ctx.makeImage()!
let dest = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: out) as CFURL, "public.png" as CFString, 1, nil)!
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)
print("wrote \(out)")
