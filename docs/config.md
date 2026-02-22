# Global Config (`nanoss.toml`)

Nanoss supports a project-level config file at repository root: `nanoss.toml`.

## Current schema

```toml
[build]
base_path = "/nanoss"
site_domain = "https://hsiangnianian.github.io"

[theme]
name = "my-theme"

[plugins]
enabled = ["demo-plugin"]
```

## Notes

- `nanoss init` / `nanoss new site <name>` generate a starter `nanoss.toml` by default.
- `build.base_path` is used to rewrite absolute site links (`/foo`) for subpath deploys.
- `build.site_domain` is optional. When set, sitemap/RSS links become absolute URLs.
- Priority is: CLI flag > `nanoss.toml` > default.
  - Example: `nanoss build --base-path /docs-preview`
- CLI override for domain: `nanoss build --site-domain https://example.com`
- If your site is deployed at domain root, use `/` (default).
- Plugin/theme keys are the same config entries used by `nanoss plugin` and `nanoss theme`.
