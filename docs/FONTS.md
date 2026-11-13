# Fonts

How the MapleStory Classic World (MSCW) client renders its UI text, and where the fonts live on disk.

## Finding

In-game UI text is Arial for English and numbers, with Korean and CJK fallbacks:

| Script | Font family | Used for |
|---|---|---|
| English / numbers | **Arial** (Regular + Bold) | EXP, level, HP/MP, map labels, most UI |
| Korean | **Gulim** / **Dotum** (both inside `gulim.ttc`) | Korean UI text |
| Other (CJK) | **Noto Sans** (CJK variable fonts) | Non-Latin fallback |

The family chain is declared in the text renderer, `Canvas.dll`, as a fallback table (`Noto Sans`, `Gulim`, `Dotum`, `Arial`) with keys `fontName`, `scales`, `charRanges`, `fallback`, `replaces`, `fonts`, `sets`. `MapleStory.exe` adds a per-element text config of `fontName` / `fontHeight` / `fontStyle` plus a `fontBold` flag, but there is no italic flag anywhere in the UI config.

Only **Arial Regular + Arial Bold** are needed to cover the English/number UI. A dialog style named `ExtraBold` exists (`UI/UtilDlgEx.img/...`), but Arial has no 800-weight face, so it renders as Arial Bold (700). The classic client shipped exactly `Arial.ttf` + `Arial-Bold.ttf` — no italic.

## Where the fonts live

### In the game client

Fonts are not shipped as `.ttf`/`.otf`. They are bitmap glyphs packed in Wizet's proprietary `PKG1` WZ archive:

| Path (under the game folder) | Role |
|---|---|
| `Data\UI\Font\Font_000.wz` | The font payload (~6.2 MB bitmap glyphs) |
| `Data\UI\Font\Font.wz` | 63-byte index stub |
| `Data\UI\Font\Font.ini` | `LastWzIndex|0` |

The game folder is the Steam install directory, e.g. `D:\SteamLibrary\steamapps\common\MapleStory\MSCW` (varies by Steam library). Find it with:

```powershell
$game = "D:\SteamLibrary\steamapps\common\MapleStory\MSCW"
Get-ChildItem "$game\Data\UI\Font"
```

### In Windows

The typefaces the client references are the Windows system fonts:

| Font | File |
|---|---|
| Arial (Regular) | `C:\Windows\Fonts\arial.ttf` |
| Arial Bold | `C:\Windows\Fonts\arialbd.ttf` |
| Arial Italic / Bold Italic | `C:\Windows\Fonts\ariali.ttf` / `arialbi.ttf` (unused by the game) |
| Gulim + Dotum (Korean) | `C:\Windows\Fonts\gulim.ttc` |
| Noto Sans CJK | `C:\Windows\Fonts\NotoSans{HK,JP,KR,SC,TC}-VF.ttf` |

Find them with:

```powershell
Get-ChildItem C:\Windows\Fonts\arial*.ttf
Get-ChildItem C:\Windows\Fonts\gulim*.ttc
Get-ChildItem C:\Windows\Fonts\NotoSans*.ttf
```

## Local copy

Extracted to `fonts/` in the project root (gitignored):

- `fonts/client/` — the game's own `Font_000.wz` / `Font.wz` / `Font.ini`
- `fonts/system/` — `arial.ttf` + `arialbd.ttf` (the only two needed for English/numbers)

## Licensing

Arial is proprietary (Monotype, bundled with Windows) and `Font_000.wz` is Nexon's game asset — both may be used locally for reference but must not be committed or redistributed. If a redistributable substitute is needed, Liberation Sans (OFL) or Arimo (Apache 2.0) are metric-compatible with Arial but not pixel-identical.
