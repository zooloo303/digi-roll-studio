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
