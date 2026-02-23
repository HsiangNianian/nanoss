use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::{canonicalize_site_url, ContentEntry};

pub(crate) fn generate_sitemap_and_feed(
    entries: &[ContentEntry],
    output_dir: &Path,
    site_domain: Option<&str>,
) -> Result<()> {
    let mut sitemap = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for entry in entries {
        let canonical = canonicalize_site_url(&entry.url, site_domain);
        sitemap.push_str(&format!("  <url><loc>{canonical}</loc></url>\n"));
    }
    sitemap.push_str("</urlset>\n");
    fs::write(output_dir.join("sitemap.xml"), sitemap)
        .with_context(|| format!("failed to write {}", output_dir.join("sitemap.xml").display()))?;

    let mut rss = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"><channel>\n<title>Nanoss feed</title>\n",
    );
    for entry in entries {
        let canonical = canonicalize_site_url(&entry.url, site_domain);
        let title = entry.title.as_str();
        rss.push_str(&format!("<item><title>{title}</title><link>{canonical}</link></item>\n"));
    }
    rss.push_str("</channel></rss>\n");
    fs::write(output_dir.join("rss.xml"), rss)
        .with_context(|| format!("failed to write {}", output_dir.join("rss.xml").display()))?;
    Ok(())
}
