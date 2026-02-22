# Nanoss Architecture

## Build pipeline

1. Scan `content_dir` for markdown and assets.
2. For markdown files:
   - Use `nanoss-query` (Salsa) to compute a content hash.
   - Skip rendering when hash and output path match the build cache.
   - Execute plugin hooks:
     - `transform_markdown`
     - `on_page_ir`
     - `on_post_render`
   - Render markdown -> HTML with TOC, anchors, and syntax highlighting.
   - Compile islands and inject runtime when needed.
3. For assets:
   - Sass -> CSS with `grass`, then optimize with `LightningCSS`.
   - CSS optimize with `LightningCSS`.
   - JS/TS via backend abstraction (`passthrough` or `esbuild`).
   - Optional Tailwind generation (`standalone` or `rswind` backend).
4. Optional post steps:
   - External link checking.
   - Semantic index generation.
5. Persist build cache to `public/.nanoss-cache.json`.

## Key crates

- `nanoss-core`: orchestration for content, assets, plugins, islands, AI index.
- `nanoss-cli`: command-line interface.
- `nanoss-plugin-api`: WIT contract for plugin lifecycle hooks.
- `nanoss-plugin-host`: Wasmtime component host runtime.
- `nanoss-query`: Salsa-based content hash and fingerprint query layer.

## Product infrastructure

- Plugin infrastructure:
  - local plugin registry and per-project enable/disable state
  - compatibility gate via `min_host_version`
- Theme infrastructure:
  - scaffold/validate/use workflow
  - template and static asset precedence rules
- CLI infrastructure:
  - `build`, `server`, `deploy`, `generate-ci`, `plugin`, `theme`

## Runtime outputs

- Rendered pages under `public/`.
- Islands runtime at `public/_nanoss/islands-runtime.js`.
- Semantic index at `public/search/semantic-index.json` when enabled.
