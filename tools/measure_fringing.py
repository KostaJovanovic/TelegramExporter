"""Measure DirectWrite colour fringing on a rendered window.

Risk 4 in ROADMAP.md, carried over from the Qt original: light text over
near-black is exactly where subpixel antialiasing shows a colour cast, and in
the Python project DirectWrite ignored ``NoSubpixelAntialias`` entirely — 81% of
inked pixels fringed — which is why that project abandoned it for FreeType.

**The measurement has to be taken on a rendered window, not an offscreen
paint.** Under FreeType those two paths disagreed 0% against 90%, so a headless
render proves nothing about what is on screen. Take a screenshot of the running
app and pass it here.

    python tools/measure_fringing.py target/window.png [left top right bottom]

A pixel is *inked* if it is far enough from the darkest background value to be
type rather than backdrop, and it *fringes* if its channels disagree by more
than a threshold no monochrome rasteriser would produce. Both thresholds are
stated below rather than tuned until the answer is pleasing.

**Pass a region, and inset it past the window border.** A screenshot taken from
the window rect includes the drop shadow and whatever desktop wallpaper is
behind it, and a purple wallpaper is a hundred per cent "fringed" by this
measure. Measuring the whole capture reported 34.5% here and every one of those
pixels was chrome or wallpaper; four regions inset by 20px reported 0.0%. The
accent red is a real colour too, so a region containing it is not a text region.

Measured 2026-08-26 on the shipped window, Windows 11, gpui 0.2.2: **0.0% of
inked pixels fringed, worst channel spread 0 levels**, across the nav bar, the
chat list, the settings panel and the empty state. GPUI's DirectWrite path is
rasterising greyscale here, so the failure that drove the Qt original off
DirectWrite does not reproduce. Re-run this after any gpui bump.
"""

from __future__ import annotations

import sys
from collections import Counter

from PIL import Image

# How far from the page a pixel must be to count as ink. Below this it is
# backdrop, JPEG-style noise, or the grid hairline.
INK = 40
# Channel spread above which a grey has a colour cast. A pure greyscale
# rasteriser produces 0; anything at or under a couple of levels is rounding.
CAST = 12


def measure(path: str, box: tuple[int, int, int, int] | None = None) -> None:
    image = Image.open(path).convert("RGB")
    if box:
        image = image.crop(box)
    # `getdata` is deprecated in Pillow 12 and gone in 14; `get_flattened_data`
    # does not exist before it. Take whichever this machine has rather than
    # pinning a Pillow version for a script that runs once a release.
    reader = getattr(image, "get_flattened_data", None) or image.getdata
    pixels = list(reader())

    # The page is whatever colour occurs most: this design is near-black over
    # most of its area, and hard-coding #0a0a0a would misread a light theme.
    page = Counter(pixels).most_common(1)[0][0]
    page_luma = sum(page) / 3.0

    inked = 0
    fringed = 0
    worst = 0
    for r, g, b in pixels:
        if abs((r + g + b) / 3.0 - page_luma) < INK:
            continue
        inked += 1
        spread = max(r, g, b) - min(r, g, b)
        worst = max(worst, spread)
        if spread > CAST:
            fringed += 1

    share = (fringed / inked * 100.0) if inked else 0.0
    print(f"{path}{f' {box}' if box else ''}")
    print(f"  page          #{page[0]:02x}{page[1]:02x}{page[2]:02x}")
    print(f"  inked pixels  {inked:,} of {len(pixels):,}")
    print(f"  fringed       {fringed:,}  ({share:.1f}% of inked)")
    print(f"  worst spread  {worst} levels")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    region = None
    if len(sys.argv) == 6:
        region = tuple(int(v) for v in sys.argv[2:6])
    measure(sys.argv[1], region)
