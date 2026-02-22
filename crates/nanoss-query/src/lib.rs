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

#[salsa::db]
#[derive(Clone, Default)]
pub struct QueryDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for QueryDb {}
