# The ZoopSync mark

A bold **Z** with a sync node on its diagonal: two ends, one link between them.
This is an original design for the ZoopSync fork. It replaces the original
NectarLink mark, which is not used here.

| File | What it is |
|---|---|
| `icon.svg` | The full-color app icon: deep indigo tile, soft violet glow |
| `icon-flat.svg` | The same without the glow, for flat contexts |
| `icon-mono.svg` | The mark alone in white, for one-color uses (notifications, themed icons) |
| `icon-512.png` | `icon.svg` at 512 × 512 |

The Android app draws the mark from the same geometry in
`android/app/src/main/res/drawable/ic_launcher_*.xml` and `ic_notification.xml`.

**Not updated yet:** the Windows app draws its own icon, tray icon and
notification mark in code (`desktop/app/src/mark.rs`, `desktop/app/build.rs`),
and those still show the original mark until that code is changed and tested.

Colors (Lavender palette, `docs/design/tokens.json`): tile `#1B1530`, glow
`#4D3D75`, mark `#F3EDFF`, node `#A98BFF`.
