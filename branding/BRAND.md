# Oxbloom

**Enterprise Rust, in bloom.**

Oxbloom is an application framework for Rust that gives you the productivity of Spring Boot — dependency injection, auto-configuration, starters, profiles, actuator-style health — with Rust's guarantees. Wiring happens at compile time, so a missing dependency is a build error, not a 3 a.m. stack trace.

This book is the source of truth for how Oxbloom looks and sounds: the website, docs, README banners, conference slides, social posts and stickers all start here.

## The name

*Oxide* + *bloom*. Rust is iron oxide; spring is when things bloom. The name says "a Spring-like framework grown out of Rust" without borrowing anyone's trademark. It is always written **Oxbloom** in prose, **oxbloom** in code, crate names and the wordmark. Never "OxBloom", "Ox-Bloom" or "OXBLOOM" (except in eyebrow labels, which are uppercase by style).

## Positioning

| | |
| --- | --- |
| **For** | Backend teams who know Spring Boot (or ASP.NET, NestJS) and want Rust's performance and safety without hand-wiring every service. |
| **Oxbloom is** | a batteries-included application framework for Rust. |
| **Unlike** | assembling axum, sqlx, tracing and config crates yourself, |
| **it** | gives you conventions, starters and dependency injection that the compiler checks. |

### Message pillars

1. **Wired at compile time.** Components and their dependencies are resolved by macros during the build. No reflection, no classpath scanning, no startup surprises.
2. **Starters, not boilerplate.** `oxbloom-starter-web`, `-data`, `-security`, `-observability`: add one and get sensible, overridable defaults.
3. **Configuration that type-checks.** Profiles and properties map onto typed structs; a typo in `application.toml` fails the build.
4. **Production from day one.** Health, metrics and graceful shutdown come built in, so day two looks like day one.

### Taglines

- Primary: **Enterprise Rust, in bloom.**
- Developer-facing: **Spring-style productivity. Rust-grade guarantees.**
- Short / sticker: **Grown from iron.**
- Technical one-liner: *Dependency injection, auto-configuration and starters — resolved by the compiler, not at startup.*

### Boilerplate (50 words)

> Oxbloom is an open-source application framework for Rust. It brings the conventions enterprise developers know from Spring Boot — dependency injection, auto-configuration, starters and profiles — and resolves them at compile time. The result is services that start instantly, use little memory and catch wiring and configuration mistakes before they ship.

## Voice

Oxbloom sounds like a senior engineer who likes you: **precise, warm, unhurried**.

- **Precise.** Say what happens and when. "Resolved at compile time" beats "magically wired".
- **Warm.** Welcome people coming from Java, Kotlin and C#. Never mock other ecosystems.
- **Unhyped.** No "blazingly fast", no rocket emoji, no unbenchmarked numbers. Show a code sample instead.
- **Growth metaphors, lightly.** *Root, grow, wire, bloom, season* — one per page at most. Rust puns: zero or one, never in headlines.

| Write | Don't write |
| --- | --- |
| A missing bean is a compile error. | Never worry about DI again! 🚀 |
| Familiar if you've used Spring Boot. | The Spring killer. |
| Starts in milliseconds on our benchmark (link). | Blazingly fast. |
| Add `oxbloom-starter-data` to get a connection pool. | Just sprinkle in some data magic. |

### About Spring

Spring and Spring Boot are trademarks of Broadcom. Refer to them only to describe compatibility of ideas ("familiar to Spring developers", "inspired by Spring Boot"). Never use the Spring leaf, its green (#6db33f), or "Spring" inside a product, crate or starter name.

## Logo

The mark is **a five-petal bloom around a hex nut**: something grown, fastened at its centre by hardware. Petals are always `oxide` (on light) or `bloom` (on `oxblood`); the nut is always `ink` or `surface-100`.

- **Lockup** (`oxbloom-lockup.svg`) is the default. Use the **stacked** lockup for square spaces, the **mark** alone where the name appears nearby (favicons, avatars, docs nav).
- **Clear space:** the height of one petal on every side.
- **Minimum size:** mark 16px, lockup 96px wide.
- **Grounds:** `surface-100`, `surface-200` or `oxblood` (with the reversed or app-icon versions). On photos, place the lockup on an `oxblood` or `surface-100` panel.
- **Don't** recolour petals in other hues, rotate the mark, add gradients or shadows, outline it, or re-set the wordmark in another font.

The wordmark is Fraunces SemiBold, outlined, lowercase, tracked −1.5%.

## Colour

Oxbloom is a **warm, paper-first** palette: oxide and oxblood carry the brand, bloom softens, sprout signals growth, ink and paper do the work.

- **Ratio on marketing pages:** ~70% `surface-100`/`surface-200`, ~15% `ink`, ~10% `oxblood`/`oxide`, the rest `bloom` and `sprout`.
- **Hero bands, social cards, app icon:** `oxblood` ground with `surface-100` text and `bloom` accents.
- **Links:** `oxide-text`, underlined on hover. **Primary buttons:** `oxide` with `on-oxide`.
- **Code windows are always dark** (`code-bg`) in both themes — the one place the brand goes "forge".
- `sprout` means success or growth only. Don't use it decoratively next to `oxide` — the pair reads as Christmas.
- Every text token is ≥4.5:1 on its ground in both themes; ratios are in each token's note.

## Typography

- **Fraunces** (display) — a soft, slightly wonky serif. Headlines, the wordmark, pull quotes in italic. Weight 600 for headlines, 400 italic for quotes.
- **IBM Plex Sans** (text) — engineered and friendly. Body, UI, labels.
- **JetBrains Mono** (code) — every code sample, CLI command and version tag.

Headlines are sentence case and end with a full stop when they're a statement ("Wired at compile time."). Eyebrows are uppercase Plex SemiBold in `oxide-text`.

## Shape and layout

- Radii are soft but not bubbly: `radius-md` for controls and code windows, `radius-lg` for cards, `radius-pill` for chips.
- One shadow (`shadow-card`), used sparingly. Prefer `hairline` borders on paper.
- **Petal motif:** the teardrop petal from the mark may be used as a large cropped shape bleeding off a hero or card edge, in `bloom` on `oxblood` or `surface-000` on paper. One petal cluster per layout.
- **Hex motif:** a hairline hexagon grid at `space-6` pitch may sit behind code windows on dark grounds, at low contrast.
- Generous whitespace: sections pad `space-16` on desktop, `space-12` on mobile.

## Iconography

Use a 1.5px-stroke outline icon set with rounded joins (Lucide fits) in `ink` or `ink-muted`. Icons sit at 20px beside `label` text with `space-2` gap. No filled, duotone or emoji icons.

## Imagery

Code is the hero image. When photography is needed: close, warm, tactile — workbenches, patinated metal, plants in a workshop window. Never stock "developers high-fiving", never glowing circuit boards.

## Assets in this system

- **Logos** — lockups, mark (colour, reversed, mono), wordmark, app icon, favicon.
- **Components** — Button, Badge, Code window: the building blocks of the website and slides.
