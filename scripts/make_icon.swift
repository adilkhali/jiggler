// Renders assets/AppIcon.icns. Run via: swift scripts/make_icon.swift
// Only needed when changing the icon; the generated .icns is checked in.
import AppKit

let root = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
let iconset = root.appendingPathComponent("build/AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let s = CGFloat(px) / 1024

    // Standard macOS icon grid: 824pt body, centred, ~185pt corner radius.
    let body = NSRect(x: 100 * s, y: 100 * s, width: 824 * s, height: 824 * s)
    let path = NSBezierPath(roundedRect: body, xRadius: 185 * s, yRadius: 185 * s)
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.3)
    shadow.shadowOffset = NSSize(width: 0, height: -10 * s)
    shadow.shadowBlurRadius = 20 * s
    shadow.set()
    NSColor.white.setFill()
    path.fill()
    NSGraphicsContext.restoreGraphicsState()

    NSGradient(colors: [
        NSColor(srgbRed: 0.20, green: 0.83, blue: 0.53, alpha: 1),
        NSColor(srgbRed: 0.05, green: 0.55, blue: 0.62, alpha: 1),
    ])!.draw(in: path, angle: -90)

    let config = NSImage.SymbolConfiguration(pointSize: 470 * s, weight: .semibold)
        .applying(.init(paletteColors: [.white]))
    if let symbol = NSImage(systemSymbolName: "cursorarrow.motionlines", accessibilityDescription: nil)?
        .withSymbolConfiguration(config) {
        let size = symbol.size
        symbol.draw(in: NSRect(x: body.midX - size.width / 2, y: body.midY - size.height / 2,
                               width: size.width, height: size.height))
    }

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

for base in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(base)x\(base).png" : "icon_\(base)x\(base)@2x.png"
        try! render(base * scale).write(to: iconset.appendingPathComponent(name))
    }
}

let output = root.appendingPathComponent("assets/AppIcon.icns")
try! FileManager.default.createDirectory(at: output.deletingLastPathComponent(), withIntermediateDirectories: true)
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", output.path]
try! task.run()
task.waitUntilExit()
precondition(task.terminationStatus == 0, "iconutil failed")
print("Wrote \(output.path)")
