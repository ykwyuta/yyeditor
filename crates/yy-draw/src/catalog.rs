//! Bundled official and original artwork. Only placed assets enter editable documents.
use crate::*;
use std::{io::Read, sync::OnceLock};

#[derive(Deserialize)]
pub struct Entry {
    pub vendor: String,
    pub category: String,
    pub name: String,
    #[serde(default)]
    pub keywords: String,
    pub source: String,
    pub file: String,
    pub svg: String,
}
impl Entry {
    pub fn category_path(&self) -> String {
        format!("{} / {}", self.vendor, self.category)
    }
    pub fn asset(&self, id: Id) -> Asset {
        Asset {
            id,
            name: format!("{} / {} / {}", self.vendor, self.category, self.name),
            source: self.source.clone(),
            svg: self.svg.clone(),
        }
    }
}
pub fn all() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut bytes = vec![];
        flate2::read::GzDecoder::new(&include_bytes!("../assets/catalog.json.gz")[..])
            .read_to_end(&mut bytes)
            .expect("bundled catalog gzip");
        let mut entries: Vec<Entry> = serde_json::from_slice(&bytes).expect("bundled catalog JSON");
        // Keep official indices stable; the independent generator cannot overwrite cloud artwork.
        entries.extend(
            serde_json::from_str::<Vec<Entry>>(include_str!("../assets/business-icons.json"))
                .expect("bundled business/IT catalog JSON"),
        );
        entries
    })
}

/// Insert one asset, deduplicated by its original SVG. Never attach the full catalog.
pub fn place(doc: &mut Document, index: usize) -> Result<Id> {
    let entry = all().get(index).ok_or("アイコンが見つかりません")?;
    if let Some(asset) = doc.assets.iter().find(|a| a.svg == entry.svg) {
        return Ok(asset.id);
    }
    let id = doc.id();
    doc.assets.push(entry.asset(id));
    Ok(id)
}
