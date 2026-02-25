# CLI Extensions

## `nanoss init`

Initialize a full starter project in the current directory (or custom dir).

```bash
nanoss init
nanoss init --dir my-site
nanoss init --dir my-site -f
```

`-f/--force` overwrites scaffold files if they already exist.

## `nanoss new`

Create scaffold resources.

```bash
nanoss new site my-site
nanoss new theme my-theme
nanoss new page docs/getting-started
nanoss new plugin my-plugin
nanoss new site my-site -f
nanoss new -f site my-site
```

If you run `nanoss new <name>`, Nanoss enters interactive mode and lets you choose `site/theme/page/plugin`.
If target already exists, use `-f/--force` to continue.

## `nanoss server`

Serve the generated site locally and optionally watch for source changes.

```bash
nanoss server --content-dir content --template-dir templates --output-dir public --host 127.0.0.1 --port 1111
nanoss server --mount-path /nanoss
```

## `nanoss dev`

`dev` is an alias of `server` with watch enabled by default. It also supports `--mount-path`.

## `nanoss deploy`

Generate platform deployment config files.

```bash
nanoss deploy netlify --output-dir public
nanoss deploy vercel --output-dir public
nanoss deploy cloudflare-pages --output-dir public
```

## `nanoss generate-ci`

Generate CI templates.

```bash
nanoss generate-ci github --output-dir public
nanoss generate-ci gitlab --output-dir public
```

Generated templates include benchmark regression gate script execution (`scripts/bench_gate.sh`).

## `nanoss build` extras

- Plugins can be enabled through `nanoss.toml` and plugin registry.
- Theme can be selected with `--theme <name>` or `nanoss.toml`.
- Site static assets directory can be configured with `--static-dir` (default `static`).
- Base path can be set with `--base-path /subpath` or `nanoss.toml` (`[build].base_path`).
- Site domain can be set with `--site-domain https://example.com` or `nanoss.toml` (`[build].site_domain`).
- Project config can be pinned with `--config /path/to/nanoss.toml` (otherwise uses content dir parent config when available).
- Draft/preview pages can be included with `--include-drafts` (also makes generated `robots.txt` use `Disallow: /`).
