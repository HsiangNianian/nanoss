# Decoupling Validation Checklist

## Phase gates

- Compile workspace with `cargo check --workspace`.
- Run tests with `cargo test --workspace`.
- Verify `nanoss build` can render pages and assets with default starter.
- Verify plugin hook chain still runs (`transform_markdown`, `on_page_ir`, `on_post_render`).
- Verify remote data source fallback cache path still works.

## Output regression checks

- Compare rendered page count and skipped page count between old/new pipeline.
- Compare generated sitemap/rss files for canonical URL and base path behavior.
- Compare static asset copy behavior (site static overrides theme static as before).
- Compare JS/CSS/image processing outputs for expected extensions and variants.

## Rollback criteria

- Any mismatch in plugin output payload shape that breaks existing plugins.
- Any routing regression in `base_path` or `mount_path`.
- Any failure in cache compatibility loading.
