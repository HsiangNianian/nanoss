use std::path::PathBuf;

#[salsa::input]
pub struct SourceFile {
    pub path: PathBuf,
    pub content: String,
}

#[salsa::tracked]
pub fn content_hash(db: &dyn salsa::Database, source: SourceFile) -> String {
    let digest = blake3::hash(source.content(db).as_bytes());
    digest.to_hex().to_string()
}

#[salsa::tracked]
pub fn page_fingerprint(db: &dyn salsa::Database, source: SourceFile) -> String {
    format!("{}:{}", source.path(db).display(), content_hash(db, source))
}

#[salsa::tracked]
pub fn combine_fingerprints(_db: &dyn salsa::Database, left: String, right: String) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(left.as_bytes());
    hasher.update(b"|");
    hasher.update(right.as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[salsa::db]
#[derive(Clone, Default)]
pub struct QueryDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for QueryDb {}
