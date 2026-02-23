use std::collections::HashSet;
use std::path::Path;

use crate::BuildScope;

pub(crate) fn scope_paths_set(scope: &BuildScope) -> HashSet<String> {
    match scope {
        BuildScope::AssetsOnly { paths } => {
            paths.iter().map(|path| normalize_fs_path(path)).collect()
        }
        BuildScope::SinglePage { path } => std::iter::once(normalize_fs_path(path)).collect(),
        BuildScope::Full => HashSet::new(),
    }
}

pub(crate) fn scope_includes_entry(
    scope: &BuildScope,
    scope_paths: &HashSet<String>,
    path: &Path,
) -> bool {
    match scope {
        BuildScope::Full => true,
        BuildScope::SinglePage { .. } => {
            path.extension().and_then(|ext| ext.to_str()) == Some("md")
                && scope_paths.contains(&normalize_fs_path(path))
        }
        BuildScope::AssetsOnly { .. } => scope_paths.contains(&normalize_fs_path(path)),
    }
}

pub(crate) fn normalize_fs_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
