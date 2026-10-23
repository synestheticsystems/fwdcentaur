# The fwd centaur mark

Daniel drew the mark on 2026-09-29: a blue centaur striding to the right, a white arrow pointing
forward on its flank. Three renderings, each 1254 by 1254 pixels:

- `fwdcentaur-logo.webp`: the mark alone on a transparent background, the outline a lighter navy.
- `fwdcentaur-app-icon.webp`: the mark on a light-blue rounded tile, no transparency; the app-icon
  form for stores and launchers.
- `fwdcentaur-logo-deep-outline.webp`: the mark alone on a transparent background, the outline and
  the hooves a deeper navy, the blues a little more saturated.

No vector source is in the repository yet. Anything that needs the mark small is derived from these
renderings until a vector exists, by `derive.py` in this folder (Python with Pillow and NumPy; run
it with `uv run assets/logo/derive.py`, and commit what it writes):

- `extensions/vscode/media/icon.png`, the VS Code extension's icon: the tile rendering, 256 by 256
  pixels (VS Code's size for Retina screens), cut to the tile, the white around its corners made
  transparent.
- `crates/fwd-web/assets/mark.png`, the page's mark: the transparent rendering, cut to the drawing,
  96 by 96 pixels. The page shows it beside its name in the header and as its favicon, and the VS
  Code panel that shows the page shows it on its tab.
