# Realworld E2E Example

This example is used to validate a realistic Nanoss workflow from scratch.

## Build (production mode)

```bash
cargo run -p nanoss-cli -- build \
  --content-dir examples/realworld-e2e/content \
  --template-dir examples/realworld-e2e/templates \
  --output-dir examples/realworld-e2e/public
```

Expected behavior:

- `draft-post` is excluded
- `future-post` is excluded
- `robots.txt` is generated with `Allow: /`
- `_nanoss/build-report.json` is generated

## Build (preview mode with drafts)

```bash
cargo run -p nanoss-cli -- build \
  --content-dir examples/realworld-e2e/content \
  --template-dir examples/realworld-e2e/templates \
  --output-dir examples/realworld-e2e/public-preview \
  --include-drafts
```

Expected behavior:

- `draft-post` and `future-post` are included
- `robots.txt` contains `Disallow: /`
