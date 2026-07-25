# Example themes

udg's theming surface is deliberate and small: the builtin stylesheet
exposes its palette as CSS custom properties, and `[output] extra_css`
loads one project stylesheet *after* it — so a theme can override the
variables, or restyle any builtin class outright. Templates are not
overridable.

The themes ship in the udg repository and in the release tarballs, not
with `cargo install`; copy the one you want next to your config and
point at it:

```toml
[output]
extra_css = "themes/rtd.css"
```

- **rtd.css** — an approximation of Sphinx's `sphinx_rtd_theme` (Read
  the Docs): blue top bar, dark sidebar, slab-serif headings, pale-blue
  signature boxes, red inline literals, admonitions with title bars.
  For projects that want the migration from Sphinx to feel like nothing
  happened. Light-only, like the original.

- **phosphor.css** — documentation on a green-phosphor terminal:
  monospace everything, amber links, scanlines, bracketed badges, a
  blinking cursor on titles. Dark-only. The demonstration that variable
  overrides plus ordinary selectors reach every part of the page.

Both are ordinary CSS files with no build step and no external
requests (fonts are local-or-fallback). Copy one next to your config
and edit — the builtin sheet's `:root` block in
`src/render/static/style.css` is the complete list of
variables to override, and single-scheme themes like these win over
the builtin dark-mode block simply by loading later. A theme that
wants to support both schemes wraps its dark values in
`@media (prefers-color-scheme: dark)` just like the builtin does.
