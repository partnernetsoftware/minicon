# MiniCon application icon

`assets/minicon.icns` is the macOS application icon. The macOS signing court
copies it into `MiniCon.app/Contents/Resources/` and names it in the bundle's
`CFBundleIconFile`, which is what puts a real icon in the Dock, in Finder and
on the Launchpad tile instead of the generic blank document.

`assets/minicon.ico` is the Windows counterpart, embedded into `minicon.exe` by
`build.rs`. The two share one design on purpose.

## The artwork

A deep navy rounded square holding two overlapping tabbed panels -- cyan above,
amber below -- each showing a `>` prompt and a cursor bar. It says "a terminal
with tabs" at a glance, with no gloss, no gradient tricks and no mascot.

Two panels and two prompts turn to mush at Dock-adjacent sizes, so the 16x16
and 32x32 faces carry a simplified variant instead: one large cyan chevron and
one amber cursor bar on the same navy square. An `.icns` is allowed to hold
different art per size and macOS picks the right face; small-size legibility is
the constraint the design is actually built around.

## Regenerating

From the repository root:

```sh
python3 assets/icon/generate_icons.py
```

That renders the ten PNG faces into `assets/minicon.iconset/` (an intermediate,
git-ignored) and compiles `assets/minicon.icns` from them with `iconutil`. It
also drops a 512x512 preview at `/tmp/minicon-icon-preview.png`.

The generator uses only the Python standard library: a small signed-distance
rasterizer with analytic antialiasing, plus a `zlib` + `struct` PNG writer.
There is no PIL, cairosvg or ImageMagick dependency, and nothing in it depends
on randomness, iteration order or the clock -- rerunning it reproduces
byte-identical output, so a regenerated `.icns` that differs from the committed
one means the source changed.

`iconutil` and `sips` ship with macOS. On a machine without `iconutil` the
script still writes the `.iconset` and says so.

## Editing the design

All geometry lives in unit coordinates of the canvas at the top of
`generate_icons.py`, and all five brand colors are in the `palette` block. The
rounded square follows Apple's macOS grid: an 824x824 square with a 185px
corner radius on a 1024px canvas.
