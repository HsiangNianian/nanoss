# CLI Extensions

## `nanoss server`

Serve the generated site locally and optionally watch for source changes.

```bash
nanoss server --content-dir content --template-dir templates --output-dir public --host 127.0.0.1 --port 1111
```

## `nanoss dev`

`dev` is an alias of `server` with watch enabled by default.

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
