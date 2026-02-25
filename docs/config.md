# Global Config (`nanoss.toml`)

Nanoss supports a project-level config file at repository root: `nanoss.toml`.

## Current schema

```toml
[build]
base_path = "/nanoss"
site_domain = "https://hsiangnianian.github.io"

[build.images]
enabled = true
generate_webp = true
generate_avif = false
widths = [480, 768, 1200]

[build.data_sources.hn]
url = "https://hacker-news.firebaseio.com/v0/topstories.json"
method = "GET"
fail_fast = false

[build.i18n]
locales = ["en", "zh"]
default_locale = "en"
prefix_default_locale = false

[server]
mount_path = "/nanoss"

[theme]
name = "my-theme"

[plugins]
enabled = ["demo-plugin"]
config = { env = "dev", feature_flags = ["toc", "search"] }
```

## Notes

- `nanoss init` / `nanoss new site <name>` generate a starter `nanoss.toml` by default.
- `build.base_path` is used to rewrite absolute site links (`/foo`) for subpath deploys.
- `build.site_domain` is optional. When set, sitemap/RSS links become absolute URLs.
- `build.images` config controls image variant generation (`webp`/`avif` and widths).
- `build.data_sources` fetches remote JSON and injects it into template `data` context.
- `build.i18n` defines locale list/default locale and output prefix strategy.
- `server.mount_path` is optional. Use it in local `dev/server` to simulate subpath hosting.
- Priority is: CLI flag > `--config <path>` > `<content_dir>/../nanoss.toml` (if exists) > current-dir `nanoss.toml` > default.
  - Example: `nanoss build --base-path /docs-preview`
- CLI override for domain: `nanoss build --site-domain https://example.com`
- If your site is deployed at domain root, use `/` (default).
- Plugin/theme keys are the same config entries used by `nanoss plugin` and `nanoss theme`.
- `plugins.config` is optional and will be serialized as JSON, then passed to plugin `init(config-json)`.
