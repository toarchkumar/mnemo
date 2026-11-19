"""Render docs/img/social-preview.png, the GitHub social preview (1280x640).

Run from the repository root with Pillow installed:

    test/.venv/bin/python docs/img/social-preview.py

It uses the macOS system fonts (Helvetica Neue, Menlo) and the site's dark
palette (index.html). Everything that matters stays inside the 40 px safe
margin GitHub asks for; the content sits inside 72 px.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 1280, 640
PAD = 72  # GitHub's safe margin is 40 px; keep well inside it

# The site's dark theme (index.html :root, prefers-color-scheme: dark).
BG = "#0E1113"
SURFACE = "#151A1D"
SURFACE_2 = "#1B2125"
LINE = "#252C31"
INK = "#E8EAED"
INK_2 = "#AEB5BD"
INK_3 = "#848C96"
ACCENT = "#2BB3A6"
ACCENT_INK = "#6FD6CB"
AMBER = "#F0A35A"

FONTS = Path("/System/Library/Fonts")


def sans(size, bold=False):
    # HelveticaNeue.ttc: index 0 regular, 1 bold.
    return ImageFont.truetype(str(FONTS / "HelveticaNeue.ttc"), size, index=1 if bold else 0)


def mono(size, bold=False):
    # Menlo.ttc: index 0 regular, 1 bold.
    return ImageFont.truetype(str(FONTS / "Menlo.ttc"), size, index=1 if bold else 0)


def main():
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # A faint accent rule along the top.
    d.rectangle([0, 0, W, 6], fill=ACCENT)

    # Left column: the name and the line.
    x = PAD
    d.text((x, 150), "MNemo", font=sans(128, bold=True), fill=INK)
    tag = sans(40)
    d.text((x, 318), "Shared memory for your", font=tag, fill=INK_2)
    d.text((x, 368), "coding agents, in one", font=tag, fill=INK_2)
    d.text((x, 418), "encrypted file.", font=tag, fill=INK_2)

    # The command, as a code pill.
    cmd = "$ mnemo onboard"
    f = mono(26)
    tw = d.textlength(cmd, font=f)
    top = 500
    d.rounded_rectangle([x, top, x + tw + 40, top + 52], radius=10, fill=SURFACE_2, outline=LINE, width=2)
    d.text((x + 20, top + 11), "$", font=f, fill=ACCENT_INK)
    d.text((x + 20 + d.textlength("$ ", font=f), top + 11), "mnemo onboard", font=f, fill=INK)

    # Right column: one file holding three layers.
    # Three 84 px layers with 16 px gaps, 92 px of header and 28 px below.
    fx0, fy0, fx1 = 720, 118, W - PAD
    fy1 = fy0 + 92 + 3 * 84 + 2 * 16 + 28
    d.rounded_rectangle([fx0, fy0, fx1, fy1], radius=18, fill=SURFACE, outline=LINE, width=2)
    # The file's name, with a padlock.
    hf = mono(24, bold=True)
    lx, ly = fx0 + 32, fy0 + 30
    d.rounded_rectangle([lx, ly + 10, lx + 22, ly + 28], radius=3, fill=AMBER)
    d.arc([lx + 4, ly - 2, lx + 18, ly + 18], start=180, end=360, fill=AMBER, width=3)
    d.text((lx + 36, ly), "project.mnemo", font=hf, fill=INK)

    layers = [
        ("1", "Memory you can search", ACCENT),
        ("2", "Agents sharing work", ACCENT_INK),
        ("3", "The engine underneath", INK_3),
    ]
    lf = sans(28, bold=True)
    nf = mono(24, bold=True)
    cy = fy0 + 92
    for n, label, color in layers:
        y0, y1 = cy, cy + 84
        d.rounded_rectangle([fx0 + 28, y0, fx1 - 28, y1], radius=12, fill=SURFACE_2, outline=LINE, width=2)
        d.rounded_rectangle([fx0 + 28, y0, fx0 + 36, y1], radius=4, fill=color)
        d.text((fx0 + 58, y0 + 27), n, font=nf, fill=color)
        d.text((fx0 + 96, y0 + 25), label, font=lf, fill=INK)
        cy = y1 + 16

    out = Path(__file__).with_suffix(".png")
    img.save(out, optimize=True)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
