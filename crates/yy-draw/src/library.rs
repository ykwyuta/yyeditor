//! Local, immutable versions of asset packs. Installing a version never edits placed artwork.
use crate::*;
use std::{
    io::Read,
    path::{Component, Path},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    pub id: String,
    pub version: String,
    pub source: String,
    pub terms: String,
    pub digest: String,
    pub assets: Vec<Id>,
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    version: String,
    source: String,
    #[serde(default)]
    terms: String,
    icons: Vec<Icon>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Icon {
    name: String,
    file: String,
}
pub fn digest(doc: &Document, ids: &[Id]) -> String {
    let mut hash = blake3::Hasher::new();
    for id in ids {
        if let Some(a) = doc.assets.iter().find(|a| a.id == *id) {
            hash.update(a.name.as_bytes());
            hash.update(&[0]);
            hash.update(a.svg.as_bytes());
        }
    }
    hash.finalize().to_hex().to_string()
}
pub fn import_manifest(path: &Path, renderer: &render::Renderer) -> Result<Document> {
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(2 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| e.to_string())?;
    if manifest.icons.len() > 5000 {
        return Err("素材は 5000 個までです".into());
    }
    let root = path
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut doc = Document::default();
    for icon in manifest.icons {
        let relative = Path::new(&icon.file);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("素材のパスはパック内の相対パスで指定してください".into());
        }
        let file = root
            .join(relative)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !file.starts_with(&root) {
            return Err("素材がパックの外を参照しています".into());
        }
        let mut bytes = vec![];
        std::fs::File::open(file)
            .map_err(|e| e.to_string())?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let id = doc.id();
        doc.assets.push(renderer.normalize_asset(
            &bytes,
            icon.name,
            manifest.source.clone(),
            id,
        )?);
    }
    let ids = doc.assets.iter().map(|a| a.id).collect::<Vec<_>>();
    let digest = digest(&doc, &ids);
    doc.packs.push(Pack {
        id: manifest.id,
        version: manifest.version,
        source: manifest.source,
        terms: manifest.terms,
        digest,
        assets: ids,
        enabled: true,
    });
    doc.validate()?;
    Ok(doc)
}
pub fn install(library: &mut Document, bundle: &Document, activate: bool) -> Result<()> {
    bundle.validate()?;
    let before = library.clone();
    for pack in &bundle.packs {
        if digest(bundle, &pack.assets) != pack.digest {
            return Err("素材パックの内容と版の記録が一致しません".into());
        }
        if let Some(old) = library
            .packs
            .iter()
            .find(|p| p.id == pack.id && p.version == pack.version)
            && old.digest != pack.digest
        {
            return Err("同じ版の素材パックは内容を変更できません".into());
        }
    }
    let mut data = bundle.clone();
    data.nodes.clear();
    data.connectors.clear();
    data.pages = vec![Page::default()];
    data.active_page = 0;
    library.paste(&data, Point::default());
    if activate {
        for pack in &bundle.packs {
            activate_version(library, &pack.id, &pack.version)?;
        }
    }
    if let Err(e) = library.validate() {
        *library = before;
        return Err(e);
    }
    Ok(())
}
pub fn activate_version(library: &mut Document, id: &str, version: &str) -> Result<()> {
    if !library
        .packs
        .iter()
        .any(|p| p.id == id && p.version == version)
    {
        return Err("素材パックの版が見つかりません".into());
    }
    for pack in &mut library.packs {
        if pack.id == id {
            pack.enabled = pack.version == version;
        }
    }
    Ok(())
}
pub fn is_visible(doc: &Document, id: Id) -> bool {
    !doc.packs.iter().any(|p| p.assets.contains(&id))
        || doc
            .packs
            .iter()
            .any(|p| p.enabled && p.assets.contains(&id))
}
