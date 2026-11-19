// Builds the menu bar template image from a black-on-white line drawing.
//
//   swift scripts/menubar-icon.swift <source image> <out dir> [stroke radius] [height pt]
//
// Writes <out dir>/menubar.png (1x) and <out dir>/menubar@2x.png: black ink, transparent
// everywhere else, cropped to the drawing and scaled to the height in points. The drawing's
// strokes are about 1/60 of its height, under a pixel at 18 pt, so each stroke is first thickened
// by the radius (in source pixels) to stay readable in the menu bar.

import AppKit
import CoreImage
import CoreImage.CIFilterBuiltins

let args = CommandLine.arguments
guard args.count >= 3 else {
    FileHandle.standardError.write("usage: menubar-icon.swift <source> <out dir> [radius] [pt]\n".data(using: .utf8)!)
    exit(2)
}
let source = URL(fileURLWithPath: args[1])
let outDir = URL(fileURLWithPath: args[2])
let radius = args.count > 3 ? Float(args[3])! : 10
let heightPt = args.count > 4 ? CGFloat(Double(args[4])!) : 18

guard let drawing = CIImage(contentsOf: source) else {
    fatalError("cannot read \(source.path)")
}
let white = CIImage(color: .white).cropped(to: drawing.extent)
let flat = drawing.composited(over: white)

let thicken = CIFilter.morphologyMinimum()
thicken.inputImage = flat
thicken.radius = radius
let thick = thicken.outputImage!.cropped(to: flat.extent)

// Ink is 1 - luminance; the crop is the box of pixels darker than mid-gray.
let gray = CGColorSpace(name: CGColorSpace.genericGrayGamma2_2)!
guard let cg = CIContext().createCGImage(thick, from: thick.extent) else {
    fatalError("cannot render")
}
let (w, h) = (cg.width, cg.height)
var pixels = [UInt8](repeating: 255, count: w * h)
let bitmap = CGContext(
    data: &pixels, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
    space: gray, bitmapInfo: CGImageAlphaInfo.none.rawValue)!
bitmap.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
var (minX, minY, maxX, maxY) = (w, h, -1, -1)
for y in 0..<h {
    for x in 0..<w where pixels[y * w + x] < 128 {
        minX = min(minX, x); maxX = max(maxX, x)
        minY = min(minY, y); maxY = max(maxY, y)
    }
}
guard maxX >= 0 else { fatalError("no ink in \(source.path)") }
let box = CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1)

// Each output pixel is the mean ink over its footprint in the source, so a stroke thinner than
// a pixel turns partly transparent instead of dropping out or aliasing.
func write(scale: CGFloat, name: String) {
    let outH = Int((heightPt * scale).rounded())
    let outW = Int((heightPt * box.width / box.height).rounded(.up) * scale)
    let step = box.height / CGFloat(outH)
    let left = box.midX - CGFloat(outW) * step / 2
    var ink = [Int](repeating: 0, count: outW * outH)
    for y in 0..<outH {
        let y0 = Int(box.minY + CGFloat(y) * step), y1 = Int(box.minY + CGFloat(y + 1) * step)
        for x in 0..<outW {
            let x0 = Int(left + CGFloat(x) * step), x1 = Int(left + CGFloat(x + 1) * step)
            var sum = 0, n = 0
            for sy in max(y0, 0)..<min(max(y1, y0 + 1), h) {
                for sx in max(x0, 0)..<min(max(x1, x0 + 1), w) {
                    sum += 255 - Int(pixels[sy * w + sx])
                    n += 1
                }
            }
            ink[y * outW + x] = n == 0 ? 0 : sum / n
        }
    }

    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: outW, pixelsHigh: outH, bitsPerSample: 8,
        samplesPerPixel: 2, hasAlpha: true, isPlanar: false, colorSpaceName: .calibratedWhite,
        bytesPerRow: outW * 2, bitsPerPixel: 16)!
    let dst = rep.bitmapData!
    for y in 0..<outH {
        for x in 0..<outW {
            dst[(y * outW + x) * 2] = 0
            dst[(y * outW + x) * 2 + 1] = UInt8(ink[y * outW + x])
        }
    }
    rep.size = NSSize(width: CGFloat(outW) / scale, height: CGFloat(outH) / scale)
    let png = rep.representation(using: .png, properties: [:])!
    try! png.write(to: outDir.appendingPathComponent(name))
    print("\(name) \(outW)x\(outH) px")
}

try! FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
write(scale: 1, name: "menubar.png")
write(scale: 2, name: "menubar@2x.png")
