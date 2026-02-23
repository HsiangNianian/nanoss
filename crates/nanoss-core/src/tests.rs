use super::*;
use tempfile::tempdir;

#[test]
fn page_template_prefers_site_over_theme() -> Result<()> {
    let site = tempdir().context("failed to create site dir")?;
    let theme = tempdir().context("failed to create theme dir")?;
    fs::write(site.path().join("page.html"), "site").context("failed to write site template")?;
    fs::create_dir_all(theme.path().join("templates")).context("failed to make theme templates")?;
    fs::write(theme.path().join("templates/page.html"), "theme")
        .context("failed to write theme template")?;
    let chosen = render::load_page_template(Some(site.path()), Some(theme.path()))?;
    assert_eq!(chosen, "site");
    Ok(())
}

#[test]
fn copy_theme_static_skips_existing_output() -> Result<()> {
    let theme = tempdir().context("failed to create theme dir")?;
    let out = tempdir().context("failed to create output dir")?;
    fs::create_dir_all(theme.path().join("static/assets")).context("failed to make static dir")?;
    fs::write(theme.path().join("static/assets/logo.txt"), "theme")
        .context("failed to write theme asset")?;
    fs::create_dir_all(out.path().join("assets")).context("failed to make out asset dir")?;
    fs::write(out.path().join("assets/logo.txt"), "site").context("failed to write site asset")?;

    render::copy_theme_static_assets(Some(theme.path()), out.path())?;
    let final_asset = fs::read_to_string(out.path().join("assets/logo.txt"))
        .context("failed to read merged asset")?;
    assert_eq!(final_asset, "site");
    Ok(())
}

#[test]
fn copy_site_static_assets_overwrites_existing_output() -> Result<()> {
    let static_dir = tempdir().context("failed to create static dir")?;
    let out = tempdir().context("failed to create output dir")?;
    fs::create_dir_all(static_dir.path().join("assets"))
        .context("failed to make static assets dir")?;
    fs::create_dir_all(out.path().join("assets")).context("failed to make output assets dir")?;
    fs::write(static_dir.path().join("assets/logo.txt"), "site")
        .context("failed to write static asset")?;
    fs::write(out.path().join("assets/logo.txt"), "old")
        .context("failed to write existing output asset")?;

    render::copy_site_static_assets(static_dir.path(), out.path())?;
    let final_asset = fs::read_to_string(out.path().join("assets/logo.txt"))
        .context("failed to read merged site static asset")?;
    assert_eq!(final_asset, "site");
    Ok(())
}

#[test]
fn template_hash_includes_theme_templates() -> Result<()> {
    let query_db = QueryDb::default();
    let site_templates = tempdir().context("failed to create site templates dir")?;
    let theme = tempdir().context("failed to create theme dir")?;
    fs::create_dir_all(theme.path().join("templates"))
        .context("failed to create theme templates")?;
    fs::write(site_templates.path().join("page.html"), "site-v1")
        .context("failed to write site template")?;
    fs::write(theme.path().join("templates/page.html"), "theme-v1")
        .context("failed to write theme template")?;

    let before = utils::compute_template_dependency_hash(
        &query_db,
        Some(site_templates.path()),
        Some(theme.path()),
    )?;
    fs::write(theme.path().join("templates/page.html"), "theme-v2")
        .context("failed to update theme template")?;
    let after = utils::compute_template_dependency_hash(
        &query_db,
        Some(site_templates.path()),
        Some(theme.path()),
    )?;

    assert_ne!(before, after);
    Ok(())
}

#[test]
fn organization_pages_use_theme_template() -> Result<()> {
    let root = tempdir().context("failed to create project root")?;
    let content = root.path().join("content");
    let static_dir = root.path().join("static");
    let output = root.path().join("public");
    let theme = root.path().join("theme");

    fs::create_dir_all(&content).context("failed to create content dir")?;
    fs::create_dir_all(&static_dir).context("failed to create static dir")?;
    fs::create_dir_all(theme.join("templates")).context("failed to create theme templates dir")?;

    fs::write(
        content.join("post-a.md"),
        "---\ntitle: Post A\ntags: [rust]\n---\n\nHello A",
    )
    .context("failed to write post-a")?;
    fs::write(
        content.join("post-b.md"),
        "---\ntitle: Post B\ncategories: [notes]\n---\n\nHello B",
    )
    .context("failed to write post-b")?;
    fs::write(
        theme.join("templates/page.html"),
        "<!doctype html><html><body><div id=\"theme-marker\">{{ title }}</div>{{ content | safe }}</body></html>",
    )
    .context("failed to write theme page template")?;

    let config = BuildConfig {
        content_dir: content,
        static_dir,
        output_dir: output.clone(),
        template_dir: None,
        theme_dir: Some(theme),
        plugin_paths: Vec::new(),
        plugin_init_config_json: "{}".to_string(),
        plugin_timeout_ms: 2_000,
        plugin_memory_limit_mb: 128,
        check_external_links: false,
        fail_on_broken_links: false,
        js_backend: JsBackend::Passthrough,
        tailwind: None,
        enable_ai_index: false,
        max_frontmatter_bytes: 64 * 1024,
        max_file_bytes: 10 * 1024 * 1024,
        max_total_files: 100_000,
        command_timeout_secs: 120,
        base_path: "/".to_string(),
        site_domain: None,
        images: ImageBuildConfig::default(),
        remote_data_sources: BTreeMap::new(),
        i18n: I18nConfig::default(),
        build_scope: BuildScope::Full,
        metrics: None,
    };

    build_site(&config)?;

    let posts_html = fs::read_to_string(output.join("posts/index.html"))
        .context("failed to read posts index")?;
    let tags_html = fs::read_to_string(output.join("tags/rust/index.html"))
        .context("failed to read tags index")?;
    let categories_html = fs::read_to_string(output.join("categories/notes/index.html"))
        .context("failed to read categories index")?;

    assert!(posts_html.contains("theme-marker"));
    assert!(tags_html.contains("theme-marker"));
    assert!(categories_html.contains("theme-marker"));
    Ok(())
}

#[test]
fn compile_islands_injects_runtime_script() {
    let (html, has_islands) = render::compile_islands(
        r#"<!doctype html><html><body><p>x</p><island name="counter" props='{"step":1}'></island></body></html>"#,
    );
    assert!(has_islands);
    assert!(html.contains("data-island=\"counter\""));
    assert!(html.contains("/_nanoss/islands-runtime.js"));
}

#[test]
fn route_segment_rejects_traversal() {
    assert!(validation::validate_route_segment("../etc", "slug").is_err());
    assert!(validation::validate_route_segment("ok-slug_1", "slug").is_ok());
}

#[test]
fn build_cache_schema_mismatch_resets() -> Result<()> {
    let dir = tempdir().context("failed to create tempdir")?;
    let cache_file = dir.path().join(BUILD_CACHE_FILE);
    fs::write(
        &cache_file,
        r#"{"schema_version":1,"pages":{"k":{"hash":"h","output":"o"}}}"#,
    )
    .context("failed to write cache fixture")?;
    let cache = cache::load_build_cache(&cache_file)?;
    assert_eq!(cache.schema_version, BUILD_CACHE_SCHEMA_VERSION);
    assert!(cache.pages.is_empty());
    Ok(())
}

#[test]
fn data_context_supports_json_yaml_toml() -> Result<()> {
    let dir = tempdir().context("failed to create tempdir")?;
    fs::create_dir_all(dir.path().join("data")).context("failed to create data dir")?;
    fs::write(dir.path().join("data/site.json"), r#"{"name":"nanoss"}"#).context("write json")?;
    fs::write(dir.path().join("data/theme.yaml"), "kind: blog").context("write yaml")?;
    fs::write(dir.path().join("data/build.toml"), "mode = 'fast'").context("write toml")?;
    let data = data::load_data_context(dir.path(), dir.path(), &BTreeMap::new())?;
    let obj = data.as_object().context("expected object data")?;
    assert!(obj.contains_key("site"));
    assert!(obj.contains_key("theme"));
    assert!(obj.contains_key("build"));
    Ok(())
}

#[test]
fn output_path_respects_i18n_default_locale_prefix_strategy() -> Result<()> {
    let root = tempdir().context("failed to create root")?;
    let content = root.path().join("content");
    let output = root.path().join("public");
    fs::create_dir_all(&content).context("failed to create content")?;
    let source = content.join("hello.md");
    fs::write(&source, "# Hello").context("failed to write source")?;

    let i18n = I18nConfig {
        locales: vec!["en".to_string(), "zh".to_string()],
        default_locale: Some("en".to_string()),
        prefix_default_locale: false,
    };
    let en = path::output_path_for(&source, &content, &output, Some("hello"), Some("en"), &i18n)?;
    let zh = path::output_path_for(&source, &content, &output, Some("hello"), Some("zh"), &i18n)?;
    assert_eq!(en, output.join("hello").join("index.html"));
    assert_eq!(zh, output.join("zh").join("hello").join("index.html"));

    let prefixed = I18nConfig {
        locales: vec!["en".to_string(), "zh".to_string()],
        default_locale: Some("en".to_string()),
        prefix_default_locale: true,
    };
    let en_prefixed = path::output_path_for(
        &source,
        &content,
        &output,
        Some("hello"),
        Some("en"),
        &prefixed,
    )?;
    assert_eq!(
        en_prefixed,
        output.join("en").join("hello").join("index.html")
    );
    Ok(())
}

#[test]
fn process_image_asset_generates_webp_variant() -> Result<()> {
    let root = tempdir().context("failed to create root")?;
    let content = root.path().join("content");
    let output = root.path().join("public");
    fs::create_dir_all(&content).context("failed to create content dir")?;
    fs::create_dir_all(&output).context("failed to create output dir")?;
    let source = content.join("cover.png");
    let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]));
    img.save(&source).context("failed to save source image")?;

    let record = assets::process_image_asset(
        &source,
        &content,
        &output,
        &ImageBuildConfig {
            enabled: true,
            generate_webp: true,
            generate_avif: false,
            widths: vec![8],
        },
    )?;
    assert_eq!(record.width, Some(16));
    assert!(record
        .variants
        .iter()
        .any(|v| v.format == "webp" && v.width == Some(8)));
    for variant in &record.variants {
        assert!(PathBuf::from(&variant.output).exists());
    }
    Ok(())
}
