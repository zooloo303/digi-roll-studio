# Digi Roll Studio icon set — mark C ("pips that become notes")

Colours: trig green #3ddc97, light green #5fe8ac, p-lock purple #a071e0, window ground #23282d → #101315.

## macOS (`icons/mac/`)
Rename into a `.iconset` folder and run `iconutil`:

```sh
mkdir AppIcon.iconset
cp icon_16x16.png    AppIcon.iconset/icon_16x16.png
cp icon_32x32.png    AppIcon.iconset/icon_16x16@2x.png
cp icon_32x32.png    AppIcon.iconset/icon_32x32.png
cp icon_64x64.png    AppIcon.iconset/icon_32x32@2x.png
cp icon_128x128.png  AppIcon.iconset/icon_128x128.png
cp icon_256x256.png  AppIcon.iconset/icon_128x128@2x.png
cp icon_256x256.png  AppIcon.iconset/icon_256x256.png
cp icon_512x512.png  AppIcon.iconset/icon_256x256@2x.png
cp icon_512x512.png  AppIcon.iconset/icon_512x512.png
cp icon_1024x1024.png AppIcon.iconset/icon_512x512@2x.png
iconutil -c icns AppIcon.iconset
```

## Web (`icons/web/`)
16/32/48 favicons, a 180px apple-touch icon (square, no rounding — iOS masks it), and 192/512 for the manifest.

## Windows (`icons/windows/`)
256px master; convert to .ico with any packer (e.g. `magick icon-256.png -define icon:auto-resize=256,64,48,32,16 icon.ico`).

16 and 32 use a simplified two-row cut: one pip fewer, thicker bars, so the mark still reads at menu-bar size.

## Two decisions and one open item

**The mark is full-bleed on purpose.** Measured: the artwork fills 100% of the
canvas at every size, padding 0 on all four edges. Apple's grid insets the shape
to ~824 in a 1024 canvas, so this renders about 1.24× larger than its Dock
neighbours. That is a look-at-it item rather than a bug — Neil has seen it and
preferred it twice. The asymmetric-corner defect from the first cut *is* fixed:
all four corners round at a symmetric 223px (21.8%), where the first version had
a square top-left at every size.

**`icons/` is a build input, not an asset folder.** `crates/app/src/main.rs`
pulls `windows/icon-256.png` and `windows/icon.ico` through `include_bytes!`, so
these files can never go untracked again without breaking the build.

**Open: the embedded master should probably be 512, not 256.** `eframe` has a
macOS path (`set_title_and_icon_mac` → `NSApplication::setApplicationIconImage`)
that `winit` alone does not, so the Dock shows the icon *without* an `.app`
bundle — which makes the Dock the largest consumer of that embedded PNG, at a
size the 256 master has to be scaled up to fill. Reading winit's "Windows and X11
only" note and stopping there is how this was first got wrong.
