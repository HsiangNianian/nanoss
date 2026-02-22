use std::path::PathBuf;

#[salsa::input]
pub struct SourceFile {
    pub path: PathBuf,
    pub content: String,
}

#[salsa::tracked]
pub struct PageFingerprint {
    pub path: PathBuf,
    pub content_hash: String,
}

#[salsa::tracked]
pub fn content_hash(db: &dyn salsa::Database, source: SourceFile) -> String {
    let digest = blake3::hash(source.content(db).as_bytes());
    digest.to_hex().to_string()
}

#[salsa::tracked]
pub fn page_fingerprint(db: &dyn salsa::Database, source: SourceFile) -> PageFingerprint {
    PageFingerprint::new(db, source.path(db).clone(), content_hash(db, source))
}

#[salsa::db]
#[derive(Default)]
pub struct QueryDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for QueryDb {}
