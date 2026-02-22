# Nanoss Plugin SDK (v0)

`nanoss` plugins use the WIT world in `crates/nanoss-plugin-api/wit/plugin.wit`.

## Lifecycle

1. `init(config-json)` once per build.
2. `transform-markdown(path, content)` before markdown parsing.
3. `on-page-ir(path, ir-json)` after markdown parse and before template render.
4. `on-post-render(path, html)` after template render and before writing output.
5. `shutdown()` once when build is done.

## Host interface

- `host.log(level, message)` is provided by nanoss host.
- Suggested levels: `debug`, `info`, `warn`, `error`.

## Compatibility rules

- Keep functions backward compatible in `0.x` by additive changes only.
- Breaking changes require a new package version in WIT namespace.
