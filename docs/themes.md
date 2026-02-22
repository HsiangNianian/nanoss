# Themes

Nanoss themes live in `.nanoss/themes/<name>`.

## Theme layout

- `theme.toml`
- `templates/page.html`
- `static/` (optional assets copied to output)

Template resolution order:

1. site template directory (`--template-dir`)
2. active theme template (`.nanoss/themes/<name>/templates/page.html`)
3. built-in default template

Static asset precedence:

- Existing output/site assets win over theme `static` files.

## Commands

- List themes: `nanoss theme list`
- Create scaffold: `nanoss theme new my-theme`
- Activate theme: `nanoss theme use my-theme`
- Validate theme: `nanoss theme validate my-theme`
