// Generates the Glyphio menu-bar (tray) template icon.
//
// A macOS template image is BLACK + ALPHA only; the system recolors it for light/dark menu
// bars automatically. The mark echoes the app logo: the lowercase "g" + text-insertion caret
// monogram ("g|") — text expansion at a glance. (The app icon's colour/crop-frame don't
// survive as a template, so the monogram alone carries the identity at menu-bar size.)
//
// Usage: swift scripts/gen-tray-icon.swift src-tauri/icons/tray.png
// Rendered at 2x menu-bar size (44px) so Retina downscaling stays crisp.

import AppKit
import CoreText

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "tray.png"
let S = 44                      // 22pt menu bar @2x
let s = CGFloat(S)

let ctx = CGContext(
    data: nil, width: S, height: S, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!

// Everything black; the menu bar tints it. Alpha carries the shape.
let black = NSColor.black

// ---- lowercase "g" ----
let fontSize = s * 0.82
let font = NSFont.systemFont(ofSize: fontSize, weight: .semibold)
let attr: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: black]
let line = CTLineCreateWithAttributedString(NSAttributedString(string: "g", attributes: attr))
let b = CTLineGetImageBounds(line, ctx)

// ---- caret bar to the right of the g ----
let caretW = s * 0.085
let gap = s * 0.10
let totalW = b.width + gap + caretW
let gX = (s - totalW) / 2 - b.minX
let gY = (s - b.height) / 2 - b.minY
ctx.textPosition = CGPoint(x: gX, y: gY)
CTLineDraw(line, ctx)

// Caret spans a touch taller than the g's body, rounded ends — the text-insertion cursor.
let caretH = b.height * 1.06
let caretX = gX + b.maxX + gap
let caretY = gY + b.minY + (b.height - caretH) / 2
ctx.setFillColor(black.cgColor)
ctx.addPath(CGPath(
    roundedRect: CGRect(x: caretX, y: caretY, width: caretW, height: caretH),
    cornerWidth: caretW / 2, cornerHeight: caretW / 2, transform: nil))
ctx.fillPath()

let img = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: img)
let png = rep.representation(using: .png, properties: [:])!
try! png.write(to: URL(fileURLWithPath: out))
FileHandle.standardError.write("wrote \(out) (\(S)x\(S))\n".data(using: .utf8)!)
