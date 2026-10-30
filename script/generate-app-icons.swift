import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// Draws the Sift app icon: a funnel of syntax-highlighted code lines with a
// single grain falling through, on a macOS-style rounded-square body.
// Usage: swiftc -O script/generate-app-icons.swift -o /tmp/generate-app-icons \
//   && /tmp/generate-app-icons crates/zed/resources

struct Palette {
    let name: String
    let top: CGColor
    let bottom: CGColor
}

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255,
        alpha: alpha
    )
}

let palettes = [
    Palette(name: "app-icon", top: rgb(0x3D4FB5), bottom: rgb(0x151B45)),
    Palette(name: "app-icon-dev", top: rgb(0xC0612B), bottom: rgb(0x46190A)),
    Palette(name: "app-icon-preview", top: rgb(0x23927E), bottom: rgb(0x0A3029)),
    Palette(name: "app-icon-nightly", top: rgb(0x7A49B8), bottom: rgb(0x261245)),
]

func fillRoundedBar(_ context: CGContext, _ rect: CGRect, _ color: CGColor) {
    let radius = rect.height / 2
    context.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
    context.setFillColor(color)
    context.fillPath()
}

/// Draws the icon in a 1024x1024 coordinate space (origin at bottom left).
func drawIcon(_ context: CGContext, palette: Palette) {
    let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!

    // Body: 824pt square with a 100pt margin, per the macOS icon grid.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    // Corner radius matching the macOS icon template (about 22.5% of the body).
    let bodyPath = CGPath(roundedRect: body, cornerWidth: 185, cornerHeight: 185, transform: nil)

    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: rgb(0x000000, 0.35))
    context.addPath(bodyPath)
    context.setFillColor(palette.bottom)
    context.fillPath()
    context.restoreGState()

    context.saveGState()
    context.addPath(bodyPath)
    context.clip()
    let background = CGGradient(
        colorsSpace: colorSpace,
        colors: [palette.top, palette.bottom] as CFArray,
        locations: [0, 1]
    )!
    context.drawLinearGradient(
        background,
        start: CGPoint(x: 512, y: body.maxY),
        end: CGPoint(x: 512, y: body.minY),
        options: []
    )
    // Soft highlight across the top half.
    let highlight = CGGradient(
        colorsSpace: colorSpace,
        colors: [rgb(0xFFFFFF, 0.14), rgb(0xFFFFFF, 0)] as CFArray,
        locations: [0, 1]
    )!
    context.drawLinearGradient(
        highlight,
        start: CGPoint(x: 512, y: body.maxY),
        end: CGPoint(x: 512, y: body.midY),
        options: []
    )
    context.restoreGState()

    // Funnel of code lines: each line is split into "tokens" like syntax
    // highlighting, and each line is narrower than the one above it.
    let tokenColors: [UInt32] = [0xF2F5FF, 0x7FD1FF, 0xFFC857, 0xB9A6FF]
    let lines: [[(width: CGFloat, color: Int)]] = [
        [(150, 0), (230, 1), (120, 3)],
        [(120, 2), (250, 0)],
        [(90, 3), (150, 1)],
        [(130, 0)],
    ]
    let barHeight: CGFloat = 54
    let tokenGap: CGFloat = 22
    let lineGap: CGFloat = 40
    // Lowered from the geometric center: the funnel narrows towards the bottom,
    // so this keeps it optically centered.
    var y: CGFloat = 660

    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -4), blur: 10, color: rgb(0x000000, 0.25))
    for line in lines {
        let totalWidth = line.reduce(0) { $0 + $1.width } + tokenGap * CGFloat(line.count - 1)
        var x = 512 - totalWidth / 2
        for token in line {
            fillRoundedBar(
                context,
                CGRect(x: x, y: y, width: token.width, height: barHeight),
                rgb(tokenColors[token.color])
            )
            x += token.width + tokenGap
        }
        y -= barHeight + lineGap
    }

    // The grain that made it through.
    let grainDiameter: CGFloat = 70
    let grainCenterY = y + barHeight - 18
    context.setShadow(offset: .zero, blur: 36, color: rgb(0xFFC857, 0.9))
    context.addEllipse(in: CGRect(
        x: 512 - grainDiameter / 2,
        y: grainCenterY - grainDiameter / 2,
        width: grainDiameter,
        height: grainDiameter
    ))
    context.setFillColor(rgb(0xFFC857))
    context.fillPath()
    context.restoreGState()
}

func writePNG(palette: Palette, pixels: Int, to url: URL) throws {
    let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
    guard let context = CGContext(
        data: nil,
        width: pixels,
        height: pixels,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else {
        throw NSError(domain: "icon", code: 1, userInfo: [NSLocalizedDescriptionKey: "no context"])
    }
    context.interpolationQuality = .high
    context.setAllowsAntialiasing(true)
    let scale = CGFloat(pixels) / 1024
    context.scaleBy(x: scale, y: scale)
    drawIcon(context, palette: palette)

    guard let image = context.makeImage(),
          let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else {
        throw NSError(domain: "icon", code: 2, userInfo: [NSLocalizedDescriptionKey: "no image"])
    }
    CGImageDestinationAddImage(destination, image, nil)
    if !CGImageDestinationFinalize(destination) {
        throw NSError(domain: "icon", code: 3, userInfo: [NSLocalizedDescriptionKey: "write failed"])
    }
}

let outputDirectory = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
do {
    for palette in palettes {
        try writePNG(palette: palette, pixels: 1024, to: outputDirectory.appendingPathComponent("\(palette.name)@2x.png"))
        try writePNG(palette: palette, pixels: 512, to: outputDirectory.appendingPathComponent("\(palette.name).png"))
    }
    print("wrote \(palettes.count * 2) icons to \(outputDirectory.path)")
} catch {
    FileHandle.standardError.write("error: \(error.localizedDescription)\n".data(using: .utf8)!)
    exit(1)
}
