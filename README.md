# nanoss

Modern Rust static site generator prototype.

## Quick start

```bash
cargo run -p nanoss-cli -- build \
  --content-dir examples/blog-basic/content \
  --template-dir examples/blog-basic/templates \
  --output-dir public
```

Enable semantic index output:

```bash
cargo run -p nanoss-cli -- build \
  --content-dir examples/blog-basic/content \
  --template-dir examples/blog-basic/templates \
  --output-dir public \
  --enable-ai-index
```

Tailwind (Rust backend):

```bash
cargo run -p nanoss-cli -- build \
  --content-dir examples/blog-basic/content \
  --template-dir examples/blog-basic/templates \
  --output-dir public \
  --tailwind-input examples/blog-basic/content/styles/site.scss \
  --tailwind-output public/styles/tailwind.css \
  --tailwind-backend rswind
```

## Docs

- `docs/architecture.md`
- `docs/plugin-sdk.md`
- `docs/benchmarks.md`
- `docs/plugins.md`
- `docs/themes.md`
- `docs/cli.md`
