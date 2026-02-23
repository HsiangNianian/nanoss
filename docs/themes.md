# Themes

Nanoss themes live in `.nanoss/themes/<name>`.

## Theme layout

- `theme.toml`
- `templates/page.html`
- `templates/*.html` (optional additional template entries, e.g. `post.html`)
- `templates/partials/*.html` (optional partials)
- `static/` (optional assets copied to output)

Template resolution order:

1. site template directory (`--template-dir`)
2. active theme template (`.nanoss/themes/<name>/templates/page.html`)
3. built-in default template

Additional templates follow the same precedence rules (site template dir overrides theme templates).

Static asset precedence:

- Existing output/site assets win over theme `static` files.

Content organization pages (`/posts`, `/tags/*`, `/categories/*`) are rendered through the same `page.html` template pipeline, so active theme/site templates apply consistently.

## Commands

- List themes: `nanoss theme list`
- Create scaffold: `nanoss theme new my-theme`
- Activate theme: `nanoss theme use my-theme`
- Validate theme: `nanoss theme validate my-theme`
