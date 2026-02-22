# Global Config (`nanoss.toml`)

Nanoss supports a project-level config file at repository root: `nanoss.toml`.

## Current schema

```toml
[build]
base_path = "/nanoss"

[theme]
name = "my-theme"

[plugins]
enabled = ["demo-plugin"]
```

## Notes

- `build.base_path` is used to rewrite absolute site links (`/foo`) for subpath deploys.
- Priority is: CLI flag > `nanoss.toml` > default.
  - Example: `nanoss build --base-path /docs-preview`
- If your site is deployed at domain root, use `/` (default).
- Plugin/theme keys are the same config entries used by `nanoss plugin` and `nanoss theme`.
