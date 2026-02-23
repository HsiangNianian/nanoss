
# Decoupling Release Strategy

## Rollout

1. `cargo check --workspace` + `cargo test --workspace` must both be green.
2. Run `nanoss build` on a real site sample and compare the number of pages, the number of static resources, `sitemap.xml`, and `rss.xml`.
3. First, validate the new modular path in internal projects for 1-2 iteration cycles.
4. Without regression, the ability to quickly roll back to the previous stable version is enabled by default and is retained.

## Smoke checklist

- The three-stage chain of plugin hooks can still be executed: `transform_markdown` / `on_page_ir` / `on_post_render`.
- Correctly fall back to the cache file when the remote data source fails.
  The child process paths for `esbuild` and `tailwind` are still subject to timeout control.
- The i18n output path and the `base_path` rewrite behavior remain unchanged.
