# Code window

The code window is Oxbloom's hero image: a dark, rounded panel showing a short, real Rust sample.

Structure: `.ox-code` › `.ox-code__bar` (three `.ox-code__dot`s, the first in `oxide`, then `.ox-code__file` with the file name) › `pre` with spans `.k` keyword, `.s` string, `.c` comment.

- Always dark (`code-bg`), in both themes.
- 6–14 lines. Show one idea: a component with an injected dependency, a starter in `Cargo.toml`, a typed config struct.
- Code must compile against the current API. No pseudo-code in marketing.
- The consumer provides the file name and the highlighted code.
