//! Official starter icons, used exclusively for architecture diagrams.
use crate::*;
pub const STARTER: &[(&str, &str, &str)] = &[
    (
        "AWS / Amazon EC2",
        "https://aws.amazon.com/architecture/icons/",
        include_str!("../assets/aws-ec2.svg"),
    ),
    (
        "AWS / Amazon S3",
        "https://aws.amazon.com/architecture/icons/",
        include_str!("../assets/aws-s3.svg"),
    ),
    (
        "AWS / Lambda",
        "https://aws.amazon.com/architecture/icons/",
        include_str!("../assets/aws-lambda.svg"),
    ),
    (
        "Azure / Virtual Machine",
        "https://learn.microsoft.com/en-us/azure/architecture/icons/",
        include_str!("../assets/azure-vm.svg"),
    ),
    (
        "Azure / Virtual Network",
        "https://learn.microsoft.com/en-us/azure/architecture/icons/",
        include_str!("../assets/azure-network.svg"),
    ),
    (
        "Azure / Storage Account",
        "https://learn.microsoft.com/en-us/azure/architecture/icons/",
        include_str!("../assets/azure-storage.svg"),
    ),
    (
        "Google Cloud / Compute Engine",
        "https://cloud.google.com/icons",
        include_str!("../assets/google-compute.svg"),
    ),
    (
        "Google Cloud / Cloud Storage",
        "https://cloud.google.com/icons",
        include_str!("../assets/google-storage.svg"),
    ),
    (
        "Google Cloud / Cloud SQL",
        "https://cloud.google.com/icons",
        include_str!("../assets/google-sql.svg"),
    ),
];
pub fn add_starter(doc: &mut Document) {
    for &(name, source, svg) in STARTER {
        if !doc.assets.iter().any(|a| a.name == name) {
            let id = doc.id();
            doc.assets.push(Asset {
                id,
                name: name.into(),
                source: source.into(),
                svg: svg.into(),
            });
        }
    }
    for (id, prefix, version) in [
        ("aws", "AWS /", "2026-07-31"),
        ("azure", "Azure /", "V24"),
        ("google-cloud", "Google Cloud /", "core-products-2026-10-20"),
    ] {
        if !doc.packs.iter().any(|p| p.id == id && p.version == version) {
            let ids = doc
                .assets
                .iter()
                .filter(|a| a.name.starts_with(prefix))
                .map(|a| a.id)
                .collect::<Vec<_>>();
            let source = doc
                .assets
                .iter()
                .find(|a| ids.contains(&a.id))
                .map(|a| a.source.clone())
                .unwrap_or_default();
            let digest = library::digest(doc, &ids);
            doc.packs.push(library::Pack {
                id: id.into(),
                version: version.into(),
                source,
                terms: include_str!("../assets/NOTICE.md").into(),
                digest,
                assets: ids,
                enabled: true,
            });
        }
    }
}
