use yy_draw::*;
#[test]
fn bundled_catalog_is_complete_static_and_renderable() {
    let entries = catalog::all();
    assert_eq!(entries.len(), 2430);
    for (vendor, count) in [
        ("AWS", 859),
        ("Azure", 714),
        ("Google Cloud", 261),
        ("業務・IT", 376),
        ("イラスト", 220),
    ] {
        assert_eq!(entries.iter().filter(|e| e.vendor == vendor).count(), count);
    }
    let renderer = render::Renderer::new(None);
    let mut names = std::collections::HashSet::new();
    for entry in entries {
        assert!(
            names.insert((&entry.vendor, &entry.file)),
            "duplicate {}",
            entry.file
        );
        assert!(!entry.category.is_empty());
        render::validate_svg(&entry.svg).unwrap_or_else(|e| panic!("{}: {e}", entry.file));
        let image = renderer
            .thumbnail(&entry.svg, 24)
            .unwrap_or_else(|e| panic!("{}: {e}", entry.file));
        assert!(
            image.data().chunks_exact(4).any(|p| p[3] != 0),
            "empty {}",
            entry.file
        );
    }
}
#[test]
fn business_icons_are_unique_categorized_searchable_and_embedded_without_the_catalog() {
    let entries = catalog::all();
    let mut categories = std::collections::BTreeMap::new();
    let mut artwork = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut d = Document::default();
    for (i, e) in entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.vendor == "業務・IT")
    {
        *categories.entry(&e.category).or_insert(0) += 1;
        assert!(!e.keywords.is_empty());
        assert_eq!(e.source, "yydraw:business-icons/v1");
        assert!(artwork.insert(&e.svg));
        assert!(names.insert(&e.name));
        assert!(
            !e.svg.contains("<text"),
            "artwork must not depend on external fonts"
        );
        let asset = catalog::place(&mut d, i).unwrap();
        assert_eq!(catalog::place(&mut d, i).unwrap(), asset);
        d.add_node(
            Shape::Icon,
            Point::new((i % 10) as f64 * 120., (i / 10) as f64 * 120.),
            Some(asset),
        );
    }
    assert_eq!(categories.len(), 38);
    for (category, count) in &categories {
        assert_eq!(
            *count,
            if category.as_str() == "21 メッセージング製品" {
                6
            } else {
                10
            }
        );
    }
    assert_eq!(d.assets.len(), 376);
    let bytes = format::encode(&d).unwrap();
    assert!(bytes.len() < 600_000);
    assert_eq!(format::decode(&bytes).unwrap(), d);
    for (query, file) in [
        ("スマートフォン", "smartphone"),
        ("Android", "android-phone"),
        ("iPhone", "iphone"),
        ("Java", "java"),
        ("Rust", "rust"),
        ("PostgreSQL", "postgresql"),
        ("AI", "artificial-intelligence"),
        ("コールセンター", "call-center"),
        ("子供", "child"),
        ("老人", "senior-person"),
        ("COBOL", "cobol"),
        ("固定長ファイル", "fixed-length-file"),
        ("CSV", "csv-file"),
        ("TSV", "tsv-file"),
        ("帳票", "business-report"),
        ("メール", "email"),
        ("WEB広告", "web-advertising"),
        ("テレビ", "television"),
        ("衛星", "satellite"),
        ("SQLite", "sqlite"),
        ("MySQL", "mysql"),
        ("MQ", "ibm-mq"),
        ("Kafka", "apache-kafka"),
        ("OpenAI", "openai"),
        ("Gemini", "gemini"),
        ("Claude", "claude"),
        ("Claude Code", "claude-code"),
        ("キュー", "message-queue"),
        ("トピック", "message-topic"),
        ("バックアップ", "full-backup"),
        ("暗号化", "encryption-at-rest"),
        ("クラスタ", "server-cluster"),
        ("犯罪者", "criminal-person"),
        ("セキュリティスペシャリスト", "security-specialist"),
        ("被害者", "victim-person"),
        ("被害者者", "victim-person"),
        ("海外諜報機関", "foreign-intelligence-agency"),
        ("テロリスト", "terrorist-person"),
        ("病院", "hospital"),
        ("先生", "teacher"),
        ("児童", "school-child"),
        ("保護者", "guardian"),
        ("医師", "physician"),
        ("看護師", "nurse"),
        ("外交官", "diplomat"),
        ("営業マン", "sales-representative"),
        ("受付嬢", "receptionist"),
        ("取締役", "board-director"),
        ("CxO", "chief-officer"),
        ("CEO", "chief-executive-officer"),
        ("CISO", "chief-information-security-officer"),
    ] {
        let entry = entries
            .iter()
            .find(|e| e.file == format!("business/{file}.svg"))
            .unwrap();
        assert!(
            format!("{} {}", entry.name, entry.keywords)
                .to_lowercase()
                .contains(&query.to_lowercase()),
            "{query} must find {}",
            entry.name
        );
    }
}
#[test]
fn friendly_icons_are_unique_categorized_searchable_and_embedded_without_the_catalog() {
    let entries = catalog::all();
    let mut categories = std::collections::BTreeMap::new();
    let mut artwork = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut d = Document::default();
    for (i, e) in entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.vendor == "イラスト")
    {
        *categories.entry(&e.category).or_insert(0) += 1;
        assert!(!e.keywords.is_empty());
        assert_eq!(e.source, "yydraw:friendly-icons/v1");
        assert!(artwork.insert(&e.svg));
        assert!(names.insert(&e.name));
        assert!(
            !e.svg.contains("<text"),
            "artwork must not depend on external fonts"
        );
        let asset = catalog::place(&mut d, i).unwrap();
        assert_eq!(catalog::place(&mut d, i).unwrap(), asset);
        d.add_node(
            Shape::Icon,
            Point::new((i % 10) as f64 * 120., (i / 10) as f64 * 120.),
            Some(asset),
        );
    }
    assert_eq!(categories.len(), 22);
    for count in categories.values() {
        assert_eq!(*count, 10);
    }
    assert_eq!(d.assets.len(), 220);
    let bytes = format::encode(&d).unwrap();
    assert!(bytes.len() < 400_000);
    assert_eq!(format::decode(&bytes).unwrap(), d);
    for (query, file) in [
        ("笑顔", "smile-person"),
        ("困る", "troubled-person"),
        ("お辞儀", "bowing-person"),
        ("OK", "ok-sign-person"),
        ("雑談", "conversation-pair"),
        ("プレゼン", "presentation-scene"),
        ("テレワーク", "remote-work-home"),
        ("買い物", "shopping-bag"),
        ("犬", "dog-guide"),
        ("猫", "cat-smile"),
        ("うさぎ", "rabbit-cheer"),
        ("くま", "bear-bow"),
        ("花丸", "hanamaru-stamp"),
        ("電球", "lightbulb-flash"),
        ("プレゼント", "gift-box-ribbon"),
        ("だるま", "new-year-daruma"),
        ("おにぎり", "onigiri-riceball"),
        ("お弁当", "bento-lunchbox"),
        ("電車", "commuter-train"),
        ("マイホーム", "cozy-house"),
        ("ビジネスマン", "biz-smile"),
        ("ビジネスウーマン", "biz-woman-smile"),
        ("外回り", "biz-sales-visit"),
        ("商談", "biz-negotiation"),
        ("報連相", "biz-horensou"),
        ("1on1", "biz-one-on-one"),
        ("社長", "biz-president-ceo"),
        ("新入社員", "biz-new-employee"),
        ("プログラマー", "biz-programmer-coding"),
        ("経理", "biz-accounting-calc"),
        ("稟議", "biz-approval-hanko"),
        ("現場作業員", "biz-factory-helmet"),
        ("医師", "biz-doctor-explain"),
        ("満足", "user-satisfied"),
        ("クレーム", "user-dissatisfied"),
        ("ワークライフバランス", "biz-work-life-balance"),
    ] {
        let entry = entries
            .iter()
            .find(|e| e.file == format!("friendly/{file}.svg"))
            .unwrap();
        assert!(
            format!("{} {}", entry.name, entry.keywords)
                .to_lowercase()
                .contains(&query.to_lowercase()),
            "{query} must find {}",
            entry.name
        );
    }
}
#[test]
fn product_artwork_keeps_full_names_instead_of_invented_abbreviations() {
    for (file, name) in [
        ("python", "Python"),
        ("javascript", "JavaScript"),
        ("typescript", "TypeScript"),
        ("kotlin", "Kotlin"),
        ("swift", "Swift"),
        ("postgresql", "PostgreSQL"),
        ("mysql", "MySQL"),
        ("mariadb", "MariaDB"),
        ("sqlite", "SQLite"),
        ("sql-server", "SQL Server"),
        ("oracle-database", "Oracle Database"),
        ("redis", "Redis"),
        ("mongodb", "MongoDB"),
        ("elasticsearch", "Elasticsearch"),
        ("ibm-mq", "IBM MQ"),
        ("apache-kafka", "Apache Kafka"),
        ("openai", "OpenAI"),
        ("chatgpt", "ChatGPT"),
        ("openai-api", "OpenAI API"),
        ("openai-codex", "OpenAI Codex"),
        ("gemini", "Gemini"),
        ("google-ai-studio", "Google AI Studio"),
        ("claude", "Claude"),
        ("claude-code", "Claude Code"),
        ("github-copilot", "GitHub Copilot"),
        ("microsoft-copilot", "Microsoft Copilot"),
    ] {
        let e = catalog::all()
            .iter()
            .find(|e| e.file == format!("business/{file}.svg"))
            .unwrap();
        assert!(e.svg.contains(&format!("<title>{name}</title>")));
        assert!(!e.svg.contains("..."));
    }
}
#[test]
fn official_cloud_catalog_is_not_duplicated_by_original_service_icons() {
    let originals: Vec<_> = catalog::all()
        .iter()
        .filter(|e| e.vendor == "業務・IT")
        .collect();
    for file in [
        "amazon-sqs",
        "amazon-sns",
        "azure-service-bus",
        "google-pubsub",
    ] {
        assert!(
            originals
                .iter()
                .all(|e| e.file != format!("business/{file}.svg"))
        );
    }
    assert_eq!(
        originals
            .iter()
            .filter(|e| e.category == "31 AIサービス・開発支援")
            .count(),
        10
    );
}
#[test]
fn placement_embeds_only_used_artwork_and_deduplicates_it() {
    let mut d = Document::default();
    let index = catalog::all()
        .iter()
        .position(|e| e.name.contains("EC2"))
        .unwrap();
    let first = catalog::place(&mut d, index).unwrap();
    assert_eq!(first, catalog::place(&mut d, index).unwrap());
    d.add_node(Shape::Icon, Point::new(8., 8.), Some(first));
    assert_eq!(d.assets.len(), 1);
    let bytes = format::encode(&d).unwrap();
    assert!(bytes.len() < 200_000);
    assert_eq!(format::decode(&bytes).unwrap(), d);
}
#[test]
fn cropped_icon_layers_match_full_svg_for_every_category() {
    let renderer = render::Renderer::new(None);
    let mut categories = std::collections::HashSet::new();
    let samples: Vec<_> = catalog::all()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| categories.insert((&e.vendor, &e.category)).then_some(i))
        .collect();
    for batch in samples.chunks(16) {
        let mut d = Document::default();
        for (i, index) in batch.iter().enumerate() {
            let asset = catalog::place(&mut d, *index).unwrap();
            d.add_node(
                Shape::Icon,
                Point::new((i % 4) as f64 * 80., (i / 4) as f64 * 72.),
                Some(asset),
            );
            let n = d.nodes.last_mut().unwrap();
            n.rect.w = 48.;
            n.rect.h = 64.;
            n.label.clear();
        }
        let o = render::Export::document(&d);
        let actual = renderer.raster(&d, &o, 1.).unwrap();
        let svg = renderer.svg(&d, &o).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut reference = resvg::tiny_skia::Pixmap::new(actual.width(), actual.height()).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut reference.as_mut(),
        );
        let difference: u64 = actual
            .data()
            .iter()
            .zip(reference.data())
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        assert!(
            difference as f64 / (actual.data().len() as f64) < 1.,
            "category batch {batch:?} differs"
        );
    }
}
