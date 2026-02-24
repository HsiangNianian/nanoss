use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::ContentEntry;

pub(crate) fn generate_sitemap_and_feed(
    entries: &[ContentEntry],
    output_dir: &Path,
    site_domain: Option<&str>,
) -> Result<()> {
    const SITEMAP_CHUNK_SIZE: usize = 5000;
    if entries.len() <= SITEMAP_CHUNK_SIZE {
        let mut sitemap = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        for entry in entries {
            let canonical = crate::path::canonicalize_site_url(&entry.url, site_domain);
            sitemap.push_str(&format!("  <url><loc>{canonical}</loc></url>\n"));
        }
        sitemap.push_str("</urlset>\n");
        fs::write(output_dir.join("sitemap.xml"), sitemap).with_context(|| {
            format!(
                "failed to write {}",
                output_dir.join("sitemap.xml").display()
            )
        })?;
    } else {
        let mut index = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        for (idx, chunk) in entries.chunks(SITEMAP_CHUNK_SIZE).enumerate() {
            let file_name = format!("sitemap-{}.xml", idx + 1);
            let mut sitemap = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
            );
            for entry in chunk {
                let canonical = crate::path::canonicalize_site_url(&entry.url, site_domain);
                sitemap.push_str(&format!("  <url><loc>{canonical}</loc></url>\n"));
            }
            sitemap.push_str("</urlset>\n");
            fs::write(output_dir.join(&file_name), sitemap).with_context(|| {
                format!("failed to write {}", output_dir.join(&file_name).display())
            })?;
            let loc = crate::path::canonicalize_site_url(&format!("/{file_name}"), site_domain);
            index.push_str(&format!("  <sitemap><loc>{loc}</loc></sitemap>\n"));
        }
        index.push_str("</sitemapindex>\n");
        fs::write(output_dir.join("sitemap.xml"), index).with_context(|| {
            format!(
                "failed to write {}",
                output_dir.join("sitemap.xml").display()
            )
        })?;
    }

    let mut rss = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"><channel>\n<title>Nanoss feed</title>\n",
    );
    for entry in entries {
        let canonical = crate::path::canonicalize_site_url(&entry.url, site_domain);
        let title = entry.title.as_str();
        rss.push_str(&format!(
            "<item><title>{title}</title><link>{canonical}</link></item>\n"
        ));
    }
    rss.push_str("</channel></rss>\n");
    fs::write(output_dir.join("rss.xml"), rss)
        .with_context(|| format!("failed to write {}", output_dir.join("rss.xml").display()))?;
    Ok(())
}

pub(crate) fn generate_robots_txt(
    output_dir: &Path,
    site_domain: Option<&str>,
    base_path: &str,
    include_drafts: bool,
) -> Result<()> {
    let mut robots = String::from("User-agent: *\n");
    if include_drafts {
        robots.push_str("Disallow: /\n");
    } else {
        robots.push_str("Allow: /\n");
    }
    if let Some(domain) = site_domain {
        let sitemap_path = crate::path::with_base_path("/sitemap.xml", base_path);
        robots.push_str(&format!(
            "Sitemap: {}\n",
            crate::path::canonicalize_site_url(&sitemap_path, Some(domain))
        ));
    }
    fs::write(output_dir.join("robots.txt"), robots).with_context(|| {
        format!(
            "failed to write {}",
            output_dir.join("robots.txt").display()
        )
    })?;
    Ok(())
}
