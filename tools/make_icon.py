"""Draw crates/tgx-app/icon/TelegramExporter.ico from the design tokens.

    python tools/make_icon.py

**This script is the source; the .ico is its build artefact** — the same
relationship ``tools/gen_jpeg_header.py`` has with
``crates/tgx-media/src/jpeg_header.rs``, which CLAUDE.md describes as
"Regenerate rather than hand-edit". The .ico is committed so that a build never
depends on Pillow being installed, but every change to the mark belongs here.

What is drawn
-------------
The app's own window, reduced to the three primitives the design has and
nothing else: **a filled field, one hairline, one red.**

* the field is ``bg`` dark, ``#0a0a0a`` — the page, full-bleed, square corners
  (``metrics::RADIUS`` is 0; square corners are the design, not a default);
* the hairline is ``fg`` dark, ``#e8e8e8``, full-bleed across the upper quarter
  — the rule under the nav bar, the divider that does all the dividing here;
* the one red is ``accent`` dark, ``#ff3347``, a solid bar on the lower half,
  flush to the left margin and stopping at 11/16 of the width.

The asymmetry is the point: a bar that bled to both edges would read as a flag,
and a centred one as a button. Stopping it short of the right edge is the
Swiss/International composition the rest of the interface is built from, and it
is what stops this looking like some other program's tile.

Why not something else
----------------------
No wordmark, no letterform, no paper-plane, no gradient, no rounded corners, no
second colour. None of those exist in ``crates/tgx-ui/src/tokens.rs``, and an
icon a user could mistake for another program's is worse than a plain one. The
mark is drawn from the *dark* palette rather than the light one because a
Windows icon sits on backgrounds it does not control: a near-black tile with a
light rule and a red bar reads on the light Explorer list, the dark taskbar and
the Alt-Tab strip alike, whereas a white field disappears into the first of
them.

Every element is an axis-aligned rectangle on integer pixel bounds, so there is
no antialiasing anywhere in the file and no element can land half on a pixel.

How the small sizes differ
--------------------------
**Each size is drawn, not downscaled.** Resampling a 256px master to 16px turns
a 1px hairline into three rows of grey mush — the same failure
``components::rule``'s doc comment describes for on-screen hairlines, and it
applies identically to a bitmap. So the geometry below is a table of integers
per size, and two things in it are deliberately not proportional:

* **The hairline stays one device pixel** at 16, 24, 32, 48 and 64, because at
  those sizes Windows draws the entry 1:1 and a hairline is by definition one
  pixel. It thickens only at 128 (2px) and 256 (3px), and even there
  sublinearly — a proportional 16px slab at 256 would be a band, not a rule.
  The thickening exists because the shell resamples the 256 entry down for the
  intermediate views (96px "large icons" is 256 x 0.375), and a single pixel
  does not survive that.
* **The red bar is rounded up at 16px** (3.5px of a proportional height becomes
  4) and down at 24 and 48, so its edges are always whole pixels. Losing half a
  pixel of red is invisible; smearing its edge is not.

The container
-------------
Entries at 16, 24, 32, 48, 64, 128 and 256 — the sizes Windows actually asks
for. The ICO is assembled by hand rather than through ``Image.save(sizes=...)``,
which resizes one bitmap into every entry and would undo the whole point above.
Sizes up to 128 are stored as 32-bit BGRA DIBs, which every consumer of the
format has understood since Windows 95; only the 256 entry is PNG-compressed,
the one size where that encoding is expected. Pillow is used solely to produce
those PNG bytes.
"""

from __future__ import annotations

import io
import struct
from pathlib import Path

from PIL import Image

OUT = Path(__file__).resolve().parent.parent / "crates" / "tgx-app" / "icon" / "TelegramExporter.ico"

# crates/tgx-ui/src/tokens.rs, Palette::dark().
BG = (0x0A, 0x0A, 0x0A)
FG = (0xE8, 0xE8, 0xE8)
ACCENT = (0xFF, 0x33, 0x47)

# size -> (rule_y, rule_thickness, bar_y, bar_height, bar_width), in pixels.
# Written out per size rather than computed, so what lands in each entry is
# readable here and a rounding change is a visible diff.
GEOMETRY = {
    16: (4, 1, 8, 4, 11),
    24: (6, 1, 12, 5, 16),
    32: (8, 1, 16, 7, 22),
    48: (12, 1, 24, 10, 33),
    64: (16, 1, 32, 14, 44),
    128: (32, 2, 64, 28, 88),
    256: (64, 3, 128, 56, 176),
}

# PNG-compress at and above this size; BMP below it. See the module docstring.
PNG_FROM = 256


def draw(size: int) -> Image.Image:
    """The mark at one size, in whole pixels, with no resampling."""
    rule_y, rule_t, bar_y, bar_h, bar_w = GEOMETRY[size]
    image = Image.new("RGBA", (size, size), BG + (0xFF,))
    # `paste` with a colour and a box fills exact integer bounds; `ImageDraw`
    # would be one more place for an off-by-one on the inclusive rectangle.
    image.paste(FG + (0xFF,), (0, rule_y, size, rule_y + rule_t))
    image.paste(ACCENT + (0xFF,), (0, bar_y, bar_w, bar_y + bar_h))
    return image


def as_bmp(image: Image.Image) -> bytes:
    """A 32-bit BGRA DIB with the AND mask an ICO entry still has to carry.

    The header declares twice the real height: the format inherited that from
    BMP, where the XOR bitmap and the 1-bit AND mask are stacked. The mask is
    all zeroes — every pixel opaque, the alpha channel does the work — but it
    must be present and 4-byte row aligned or the shell reads the next entry's
    bytes as image data.
    """
    size = image.width
    pixels = image.load()
    header = struct.pack(
        "<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0
    )
    xor = bytearray()
    for y in range(size - 1, -1, -1):  # DIBs are bottom-up.
        for x in range(size):
            r, g, b, a = pixels[x, y]
            xor += bytes((b, g, r, a))
    mask_stride = ((size + 31) // 32) * 4
    return header + bytes(xor) + bytes(mask_stride * size)


def as_png(image: Image.Image) -> bytes:
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=True)
    return buffer.getvalue()


def main() -> None:
    sizes = sorted(GEOMETRY)
    payloads = [
        as_png(draw(s)) if s >= PNG_FROM else as_bmp(draw(s)) for s in sizes
    ]

    # ICONDIR, then one 16-byte ICONDIRENTRY each, then the payloads.
    offset = 6 + 16 * len(sizes)
    directory = struct.pack("<HHH", 0, 1, len(sizes))
    for size, payload in zip(sizes, payloads):
        # 256 is stored as 0: the field is a single byte and 256 does not fit.
        directory += struct.pack(
            "<BBBBHHII",
            size % 256,
            size % 256,
            0,
            0,
            1,
            32,
            len(payload),
            offset,
        )
        offset += len(payload)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(directory + b"".join(payloads))
    print(f"{OUT}  {OUT.stat().st_size:,} bytes")
    for size, payload in zip(sizes, payloads):
        kind = "png" if size >= PNG_FROM else "bmp"
        print(f"  {size:>3}  {kind}  {len(payload):>7,} bytes")


if __name__ == "__main__":
    main()
