"""Rebuild the site's brand assets with Pillow (no network or API keys).

Run: python3 website/generate_assets.py
Use --font-dir for a directory containing Arial or DejaVu Sans fonts.
"""

import argparse
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--font-dir", type=Path)
args = parser.parse_args()
font_dirs = ([args.font_dir] if args.font_dir else []) + [
    Path("/System/Library/Fonts/Supplemental"),
    Path("/usr/share/fonts/truetype/dejavu"),
]


def font(size, bold=False, mono=False):
    names = (
        ["Courier New.ttf", "DejaVuSansMono.ttf"] if mono else
        ["Arial Bold.ttf", "DejaVuSans-Bold.ttf"] if bold else
        ["Arial.ttf", "DejaVuSans.ttf"]
    )
    for directory in font_dirs:
        for name in names:
            path = directory / name
            if path.exists():
                return ImageFont.truetype(str(path), size)
    raise SystemExit("Install Arial/DejaVu fonts or specify --font-dir.")


assets = Path(__file__).resolve().parent / "assets"
assets.mkdir(exist_ok=True)
paper, ink, lime, muted = "#f7f8f2", "#20251b", "#d5f86b", "#646b5c"


def mark(size):
    # Render large and downsample so the same vector mark stays crisp at 16px.
    image = Image.new("RGB", (1024, 1024), lime)
    draw = ImageDraw.Draw(image)
    draw.polygon([(597, 110), (213, 635), (527, 635), (423, 1019-110),
                  (841, 460), (527, 460)], fill=ink)
    return image.resize((size, size), Image.Resampling.LANCZOS)


mark(180).save(assets / "apple-touch-icon.png", optimize=True)
mark(512).save(assets / "icon-512.png", optimize=True)
mark(48).save(assets / "favicon.ico", sizes=[(16, 16), (32, 32), (48, 48)])
(assets / "favicon.svg").write_text(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">'
    '<rect width="40" height="40" rx="10" fill="#d5f86b"/>'
    '<path d="M23 6 10 23h10l-3 11 13-17H20z" fill="#20251b"/>'
    '</svg>\n'
)

image = Image.new("RGB", (1200, 630), paper)
draw = ImageDraw.Draw(image)
image.paste(mark(48), (64, 54))
draw.text((128, 60), "Ultrafinance", font=font(34, bold=True), fill=ink)
draw.line((64, 134, 1136, 134), fill="#dde1d3", width=2)
draw.text((64, 176), "MERCHANT ENRICHMENT API", font=font(17, mono=True), fill=muted)
draw.text((60, 226), "Make sense", font=font(78, bold=True), fill=ink)
draw.rounded_rectangle((292, 321, 592, 413), radius=4, fill=lime)
draw.text((60, 322), "of the spend.", font=font(78, bold=True), fill=ink)
draw.text((64, 453), "One simple API. Open source at its core.", font=font(25), fill=muted)

draw.rounded_rectangle((698, 202, 1136, 476), radius=16, fill="#ffffff", outline="#dde1d3", width=2)
draw.text((725, 229), "POST /v1/enrich", font=font(20, mono=True), fill="#58712e")
draw.line((698, 273, 1136, 273), fill="#dde1d3", width=2)
for y, line in [(301, '{'), (337, '  "description": "LS",'),
                (373, '  "currency": "CAD"'), (409, '}')]:
    draw.text((725, y), line, font=font(21, mono=True), fill=ink)
draw.text((64, 553), "ultrafinance.app", font=font(21, mono=True), fill=ink)
draw.text((958, 553), "Built in Rust", font=font(18, mono=True), fill=muted)
image.save(assets / "share.png", optimize=True)
print(f"Generated brand assets in {assets}")
