# Oxbloom branding

Everything needed to present Oxbloom consistently. Start with **[BRAND.md](BRAND.md)**: the brand book covering name, positioning, taglines, voice, logo rules, colour and type.

| Folder / file | What's in it |
| --- | --- |
| `BRAND.md` | Brand book: positioning, message pillars, taglines, boilerplate, voice, logo, colour, type, imagery |
| `logos/` | SVG logo suite (lockups, mark, wordmark, app icon, favicon), `favicon.ico`, and `png/` exports. Usage notes in `logos/README.md` |
| `tokens.json` | Design tokens: colours (light "Paper" / dark "Forge"), type styles, spacing, radii, shadow |
| `tokens.css` | The same as CSS custom properties, `@font-face` rules and type-style classes (`.display-xl`, `.body`, `.code`…). Respects `prefers-color-scheme` and `data-theme="light|dark"` |
| `fonts/` | Fraunces, IBM Plex Sans, JetBrains Mono (latin subset, woff2) |
| `components/` | `oxbloom.css` (Button, Badge, Code window) plus usage notes |
| `marketing/` | Standalone HTML: landing page, Open Graph card (1200×630), GitHub social preview (1280×640), launch post (1080×1080), sticker, X/LinkedIn header (1500×500), talk title slide (1920×1080) |

## Quick use

```html
<link rel="stylesheet" href="branding/tokens.css">
<link rel="stylesheet" href="branding/components/oxbloom.css">
<h1 class="display-xl">Enterprise Rust, in bloom.</h1>
<a class="ox-btn ox-btn--primary" href="/docs">Get started</a>
```

README banner:

```markdown
<p align="center"><img src="branding/logos/oxbloom-lockup.svg" alt="Oxbloom" width="360"></p>
```

The marketing pages load fonts from Google Fonts and logos from `../logos/`. Open one in a browser at its native size and screenshot it to get a PNG. Text in [BRACKETS] is a placeholder.

## Licences

The fonts are licensed under the SIL Open Font License 1.1 (Fraunces: Undercase Type; IBM Plex: IBM; JetBrains Mono: JetBrains). See `fonts/LICENSE.md`.
