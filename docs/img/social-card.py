"""Render site/social-card.png, the site's Open Graph and Twitter card (1200x630).

Run from the repository root with Pillow installed:

    test/.venv/bin/python docs/img/social-card.py

It uses the macOS system fonts (Helvetica Neue, Menlo), which are in the
page's font stacks, and the page's dark palette (index.html). The text
stays inside a 64 px margin, so a feed's crop to 1.91:1 or a rounded
corner cuts nothing. No version number: the card outlives each release.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 1200, 630
PAD = 72

# The page's dark theme (index.html :root, prefers-color-scheme: dark).
BG = "#0E1113"
SURFACE_2 = "#1B2125"
LINE = "#252C31"
INK = "#E8EAED"
INK_2 = "#AEB5BD"
INK_3 = "#848C96"
ACCENT = "#2BB3A6"
ACCENT_INK = "#6FD6CB"
# The logo's own colors (the page's favicon).
LOGO_FILE = "#0E7C74"
LOGO_FOLD = "#5FD0C4"
AMBER = "#F0A35A"

FONTS = Path("/System/Library/Fonts")
OUT = Path(__file__).resolve().parents[2] / "site" / "social-card.png"


def sans(size, bold=False):
    # HelveticaNeue.ttc: index 0 regular, 1 bold.
    return ImageFont.truetype(str(FONTS / "HelveticaNeue.ttc"), size, index=1 if bold else 0)


def mono(size):
    return ImageFont.truetype(str(FONTS / "Menlo.ttc"), size, index=0)


def logo(size):
    """The favicon's mark (a file with a padlock and an M), drawn at 4x
    on its 64-unit grid and scaled down, so its edges are smooth."""
    s = 4 * size / 64
    big = Image.new("RGBA", (4 * size, 4 * size), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)

    def p(x, y):
        return (x * s, y * s)

    # The padlock's shackle: an arc from (24, 21) up and over to (40, 21).
    w = round(5.2 * s)
    d.arc([*p(24, 6), *p(40, 22)], start=180, end=360, fill=AMBER, width=w)
    for x in (24, 40):
        d.line([p(x, 14), p(x, 21)], fill=AMBER, width=w)
        d.ellipse([*p(x - 2.6, 21 - 2.6), *p(x + 2.6, 21 + 2.6)], fill=AMBER)
    # The file, its top-right corner folded.
    d.rounded_rectangle([*p(12, 20), *p(52, 60)], radius=5 * s, fill=LOGO_FILE)
    d.polygon([p(43, 19.5), p(52.5, 19.5), p(52.5, 29)], fill=BG)
    d.polygon([p(43, 20), p(43, 26), p(46, 29), p(52, 29)], fill=LOGO_FOLD)
    # The M.
    w = round(4.8 * s)
    pts = [p(22.5, 51), p(22.5, 33), p(32, 42.5), p(41.5, 33), p(41.5, 51)]
    d.line(pts, fill="#FFFFFF", width=w, joint="curve")
    for x, y in ((22.5, 51), (41.5, 51)):
        d.ellipse([*p(x - 2.4, y - 2.4), *p(x + 2.4, y + 2.4)], fill="#FFFFFF")
    return big.resize((size, size), Image.LANCZOS)


def main():
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # The accent rule along the top, as on the GitHub preview.
    d.rectangle([0, 0, W, 6], fill=ACCENT)

    # The mark and the name.
    mark = logo(72)
    img.paste(mark, (PAD, 74), mark)
    d.text((PAD + 92, 84), "MNemo", font=sans(52, bold=True), fill=INK)

    # The headline, at the largest size up to 76 px whose longer line fits
    # inside the margins, then the line under it.
    lines = ("Every agent you run,", "remembering and coordinating as one.")
    size = 76
    while max(d.textlength(t, font=sans(size, bold=True)) for t in lines) > W - 2 * PAD:
        size -= 1
    head = sans(size, bold=True)
    pitch = round(size * 1.2)
    d.text((PAD, 206), lines[0], font=head, fill=INK)
    d.text((PAD, 206 + pitch), lines[1], font=head, fill=ACCENT_INK)
    d.text((PAD, 206 + 2 * pitch + 44), "One encrypted file in your repo.", font=sans(40), fill=INK_2)

    # The command, as a code pill, and the address.
    f = mono(28)
    top = 500
    tw = d.textlength("$ mnemo onboard", font=f)
    d.rounded_rectangle([PAD, top, PAD + tw + 44, top + 58], radius=10, fill=SURFACE_2, outline=LINE, width=2)
    d.text((PAD + 22, top + 13), "$", font=f, fill=ACCENT_INK)
    d.text((PAD + 22 + d.textlength("$ ", font=f), top + 13), "mnemo onboard", font=f, fill=INK)
    addr = "toarchkumar.github.io/mnemo"
    af = sans(28)
    d.text((W - PAD - d.textlength(addr, font=af), top + 15), addr, font=af, fill=INK_3)

    OUT.parent.mkdir(exist_ok=True)
    img.save(OUT, optimize=True)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {W}x{H})")


if __name__ == "__main__":
    main()
