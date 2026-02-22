# Plugins

Nanoss supports WASM component plugins and a local plugin registry.

## Commands

- List installed plugins:
  - `nanoss plugin list`
- Install a plugin from a `.wasm` file:
  - `nanoss plugin install --id demo --version 0.1.0 --source ./demo.wasm`
- Enable/disable for the current project:
  - `nanoss plugin enable demo`
  - `nanoss plugin disable demo`
- Update an installed plugin:
  - `nanoss plugin update demo --version 0.2.0 --source ./demo-v2.wasm`

## Registry and config files

- Registry: `.nanoss/plugins/registry.json`
- Project enablement: `nanoss.toml` (`[plugins].enabled`)

## Compatibility

Each plugin entry has `min_host_version`. The CLI rejects installation/use when the current host version is lower.
