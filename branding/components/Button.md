# Button

Buttons start an action or lead to the next page; Oxbloom uses at most one primary per view.

| Variant | Class | Use |
| --- | --- | --- |
| Primary | `ox-btn ox-btn--primary` | The one main call to action ("Get started"). `oxide` fill, `on-oxide` text. |
| Secondary | `ox-btn ox-btn--secondary` | Alternatives ("Read the docs"). `surface-200` with `control-border`. |
| Ghost | `ox-btn ox-btn--ghost` | Tertiary links with an arrow ("See the benchmark →"). |
| Code | `ox-btn ox-btn--code` | Copyable install command in heroes. Mono, dark, `$` prompt in `sprout`-family green. |

Labels are verbs in sentence case, two or three words. The consumer provides the element (`<a>` for navigation, `<button>` for actions) and the label. Focus ring: 2px `oxide`, offset 2px.
