# The MNemo brand

**The user locked this on 2026-11-19 (REV-013.1, `decision.brand`).** The
site, `mnemo inspect`, the social preview, the docs and any app screens
use this mark and these colors. Changing either is a brand rethink, and
only the user starts one.

## The mark

A sealed file with an `M` on it: an amber shackle (encrypted), a teal
file with a mint folded corner (one file), and the site's 45° `M`.

| File | Use |
|---|---|
| [`img/logo.svg`](img/logo.svg) | the mark, in color, on light or dark |
| [`img/logo-mono-dark.svg`](img/logo-mono-dark.svg) | one color, on a dark surface |
| [`img/logo-mono-light.svg`](img/logo-mono-light.svg) | one color, on a light surface |

Its four colors are fixed in both themes: shackle `#F0A35A`, file
`#0E7C74`, fold `#5FD0C4`, `M` `#FFFFFF`.

**Rules:**
- Put the mark straight on the background or a surface, never inside a
  teal box: it's already a teal file.
- Never recolor it for a theme, stretch it, change the `M`'s angle, or
  add a shadow, glow or gradient. No brain, neural-net or fish pictures
  beside it.
- It stays sharp down to 16 px (the favicon). Leave an eighth of its
  width clear around it next to the wordmark.
- **The wordmark** is `MNemo`, in the system sans at weight 700, letter
  spacing -0.01em to -0.025em, in `--ink`. Code, the CLI, the packages
  and the file stay `mnemo`, `mnemo-engine` and `.mnemo`.

## The colors

`index.html` is the source; these tables copy its tokens. A new surface
takes its colors from them.

### Light Theme (`:root`, default)

| Token | Value | Role |
|---|---|---|
| `--bg` | `#FBFAF7` | Warm paper primary canvas (`theme-color` light) |
| `--bg-2` | `#F5F3EE` | Secondary warm band / alternating section background |
| `--surface` | `#FFFFFF` | Elevated cards, panels, dialogs |
| `--surface-2` | `#F4F2EC` | Subtle inset panels, inline `<code>` fill |
| `--ink` | `#15181E` | Primary headings & body text |
| `--ink-2` | `#4A5160` | Secondary body copy, nav links |
| `--ink-3` | `#6E7583` | Muted labels, metadata, captions |
| `--line` | `#E7E3DA` | Hairline borders & dividers |
| `--line-2` | `#D9D4C8` | Emphasized borders & control outlines |
| `--accent` | `#0E7C74` | Primary brand teal (buttons, focus rings, logo body) |
| `--accent-ink` | `#0A5E58` | High-contrast teal for links & text on light surfaces |
| `--accent-tint` | `#E3F2EF` | Soft teal tint for announcement bars, badges, callouts |
| `--on-accent` | `#FFFFFF` | Text/icons placed on `--accent` |
| `--amber` | `#A34B07` | High-contrast amber for text/warnings on light surfaces |
| `--amber-tint` | `#FDF1E3` | Soft amber callout tint |
| `--nav-bg` | `rgba(251,250,247,.85)` | Sticky frosted navbar background |

### Dark Theme (`prefers-color-scheme: dark` / `:root[data-theme="dark"]`)

| Token | Value | Role |
|---|---|---|
| `--bg` | `#0E1113` | Deep obsidian slate primary canvas (`theme-color` dark, `social-preview.png` BG) |
| `--bg-2` | `#12161A` | Secondary band / alternating section background |
| `--surface` | `#151A1D` | Elevated cards & panels (`social-preview.png` card) |
| `--surface-2` | `#1B2125` | Inset panels, code pills (`social-preview.png` layer rows) |
| `--ink` | `#E8EAED` | Primary headings & body text |
| `--ink-2` | `#AEB5BD` | Secondary body copy, nav links |
| `--ink-3` | `#848C96` | Muted labels, metadata, layer-3 bar |
| `--line` | `#252C31` | Hairline borders & dividers |
| `--line-2` | `#333C42` | Emphasized borders & control outlines |
| `--accent` | `#2BB3A6` | Luminous brand teal for dark surfaces |
| `--accent-ink` | `#6FD6CB` | High-contrast mint-teal for links & dark-surface highlights |
| `--accent-tint` | `rgba(43,179,166,.13)` | Soft teal tint on dark surfaces |
| `--on-accent` | `#06201D` | Dark ink placed on `--accent` buttons |
| `--amber` | `#F0A35A` | Signature warm amber (logo lock shackle, dark warnings/accents) |
| `--amber-tint` | `rgba(240,163,90,.12)` | Soft amber callout tint on dark surfaces |
| `--nav-bg` | `rgba(14,17,19,.82)` | Sticky frosted navbar background |

### Terminal Surface (Always Dark in Both Themes)

| Token | Light Mode Value | Dark Mode Value | Role |
|---|---|---|---|
| `--term-bg` | `#101518` | `#0B0F11` | Terminal body |
| `--term-bar` | `#161D21` | `#12181B` | Terminal chrome header |
| `--term-line` | `#232C31` | `#1F272B` | Terminal divider |
| `--term-ink` | `#D7DCE0` | `#D7DCE0` | Terminal output text |
| `--term-dim` | `#7F8A93` | `#7F8A93` | Terminal comments / dim output |
| `--term-accent` | `#5FD0C4` | `#5FD0C4` | Prompt `$`, success tags, logo fold highlight |
| `--term-amber` | `#F0A35A` | `#F0A35A` | Warnings, lock accent |
| `--term-user` | `#E8EAED` | `#E8EAED` | User-typed commands |

### Syntax Highlighting Tokens

| Token | Light Mode | Dark Mode | Role |
|---|---|---|---|
| `--code-ink` | `#2B303B` | `#D7DCE0` | Default code ink |
| `--code-k` | `#7C3AED` | `#C4A7FF` | Keywords |
| `--code-s` | `#0F766E` | `#6FD6CB` | Strings |
| `--code-n` | `#B45309` | `#F0A35A` | Numbers / constants |
| `--code-c` | `#7E8590` | `#7F8A93` | Comments |
| `--code-f` | `#1D4ED8` | `#8AB4FF` | Functions |
| `--code-t` | `#A16207` | `#E8C37A` | Types |

### Type and radii
- **Sans stack (`--sans`):** `ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif` (zero webfont download overhead).
- **Mono stack (`--mono`):** `ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace`.
- **Radii:** `--radius: 12px` (cards/panels), `--radius-sm: 8px` (buttons/inputs), `5px` (inline `<code>`).
