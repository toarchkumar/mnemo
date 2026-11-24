"""Render the Claude Code plugin's listing icon,
integrations/claude-code/.claude-plugin/icon.png (512x512).

Run from the repository root with Pillow installed:

    test/.venv/bin/python docs/img/plugin-icon.py

The mark (docs/brand.md) drawn by social-card.py's `logo`, straight on the
page's dark background, with a sixth of the width clear on each side. The
plugin directory takes a square PNG of 512 to 2048 px, and keeps the icon
it first sees, so change this only before the first submission.
"""

import importlib.util
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("social_card", ROOT / "docs/img/social-card.py")
card = importlib.util.module_from_spec(spec)
spec.loader.exec_module(card)

SIZE = 512
OUT = ROOT / "integrations/claude-code/.claude-plugin/icon.png"


def main():
    img = Image.new("RGB", (SIZE, SIZE), card.BG)
    mark_size = SIZE * 2 // 3
    mark = card.logo(mark_size)
    at = (SIZE - mark_size) // 2
    img.paste(mark, (at, at), mark)
    img.save(OUT, optimize=True)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
