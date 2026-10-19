//! Versioned binary envelope, bounded decoding and atomic replacement.
use crate::*;
use std::{
    io::{Read, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"YYDRAW\0\0";
const LIMIT: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub kind: [u8; 4],
    pub flags: u32,
    pub id: u64,
    pub data: Vec<u8>,
}
pub fn encode(doc: &Document) -> Result<Vec<u8>> {
    doc.validate()?;
    let mut model = doc.clone();
    model.assets.clear();
    model.extra_chunks.clear();
    let mut chunks = vec![
        Chunk {
            kind: *b"DOCS",
            flags: 1,
            id: 0,
            data: postcard::to_allocvec(&model).map_err(|e| e.to_string())?,
        },
        Chunk {
            kind: *b"ASTS",
            flags: 1,
            id: 0,
            data: postcard::to_allocvec(&doc.assets).map_err(|e| e.to_string())?,
        },
    ];
    let corners: Vec<_> = doc
        .connectors
        .iter()
        .chain(doc.pages.iter().flat_map(|p| &p.connectors))
        .filter(|c| c.corner_radius > 0.)
        .map(|c| (c.id, c.corner_radius))
        .collect();
    if !corners.is_empty() {
        chunks.push(Chunk {
            kind: *b"CRNR",
            flags: 1,
            id: 0,
            data: postcard::to_allocvec(&corners).map_err(|e| e.to_string())?,
        });
    }
    for chunk in &doc.extra_chunks {
        if chunk.flags & 1 != 0
            || (chunk.id == 0 && matches!(&chunk.kind, b"DOCS" | b"ASTS" | b"CRNR"))
        {
            return Err("未知の必須チャンクは保存できません".into());
        }
        chunks.push(chunk.clone());
    }
    if chunks.len() > 10000 {
        return Err("チャンク数が多すぎます".into());
    }
    let directory = 4 + chunks.len() * 64;
    let mut offset = directory;
    let mut body = Vec::new();
    body.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    let mut ids = std::collections::HashSet::new();
    for c in &chunks {
        if !ids.insert((c.kind, c.id)) {
            return Err("チャンクの ID が重複しています".into());
        }
        body.extend_from_slice(&c.kind);
        body.extend_from_slice(&c.flags.to_le_bytes());
        body.extend_from_slice(&c.id.to_le_bytes());
        body.extend_from_slice(&(offset as u64).to_le_bytes());
        body.extend_from_slice(&(c.data.len() as u64).to_le_bytes());
        body.extend_from_slice(blake3::hash(&c.data).as_bytes());
        offset = offset
            .checked_add(c.data.len())
            .ok_or("ファイルが大きすぎます")?;
    }
    for c in chunks {
        body.extend(c.data);
    }
    if body.len() > LIMIT {
        return Err("ファイルは 64 MiB までです".into());
    }
    let mut bytes = Vec::with_capacity(body.len() + 52);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&1u16.to_le_bytes());
    let extended = doc
        .connectors
        .iter()
        .chain(doc.pages.iter().flat_map(|p| p.connectors.iter()))
        .any(|c| {
            [c.start, c.end]
                .iter()
                .any(|e| matches!(e,Endpoint::Attached{side,..} if *side>=4))
        });
    let minor: u16 = if !corners.is_empty() {
        3
    } else if extended {
        2
    } else {
        1
    };
    bytes.extend_from_slice(&minor.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u64).to_le_bytes());
    bytes.extend_from_slice(blake3::hash(&body).as_bytes());
    bytes.extend(body);
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> Result<Document> {
    if bytes.len() < 52 || &bytes[..8] != MAGIC {
        return Err("yydraw 形式ではありません".into());
    }
    if bytes[8..10] != [1, 0] || u16::from_le_bytes(bytes[10..12].try_into().unwrap()) > 3 {
        return Err("未対応のファイル版です".into());
    }
    let len = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    if len > LIMIT as u64 || len != bytes.len() as u64 - 52 {
        return Err("ファイルの長さが不正です".into());
    }
    if blake3::hash(&bytes[52..]).as_bytes() != &bytes[20..52] {
        return Err("ファイルが破損しています".into());
    }
    let doc = if bytes[10..12] == [0, 0] {
        let legacy: Legacy = postcard::from_bytes(&bytes[52..]).map_err(|e| e.to_string())?;
        legacy.into_document()
    } else {
        decode_chunks(&bytes[52..])?
    };
    doc.validate()?;
    Ok(doc)
}
fn decode_chunks(body: &[u8]) -> Result<Document> {
    if body.len() < 4 {
        return Err("チャンク目次がありません".into());
    }
    let count = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
    if count > 10000 || 4 + count * 64 > body.len() {
        return Err("チャンク目次の長さが不正です".into());
    }
    let directory = 4 + count * 64;
    let mut doc = None;
    let mut assets = None;
    let mut corners = Vec::<(Id, f64)>::new();
    let mut extras = vec![];
    let mut ranges = vec![];
    let mut ids = std::collections::HashSet::new();
    for entry in body[4..directory].chunks_exact(64) {
        let kind: [u8; 4] = entry[..4].try_into().unwrap();
        let flags = u32::from_le_bytes(entry[4..8].try_into().unwrap());
        let id = u64::from_le_bytes(entry[8..16].try_into().unwrap());
        let offset = u64::from_le_bytes(entry[16..24].try_into().unwrap());
        let len = u64::from_le_bytes(entry[24..32].try_into().unwrap());
        let end = offset.checked_add(len).ok_or("チャンクの範囲が不正です")?;
        if offset < directory as u64 || end > body.len() as u64 || !ids.insert((kind, id)) {
            return Err("チャンクの範囲・ID が不正です".into());
        }
        ranges.push((offset, end));
        let data = &body[offset as usize..end as usize];
        if blake3::hash(data).as_bytes() != &entry[32..64] {
            return Err("チャンクが破損しています".into());
        }
        match (&kind, id) {
            (b"DOCS", 0) => {
                doc = Some(postcard::from_bytes::<Document>(data).map_err(|e| e.to_string())?)
            }
            (b"ASTS", 0) => {
                assets = Some(postcard::from_bytes::<Vec<Asset>>(data).map_err(|e| e.to_string())?)
            }
            (b"CRNR", 0) => {
                corners = postcard::from_bytes(data).map_err(|e| e.to_string())?;
            }
            _ => {
                if flags & 1 != 0 {
                    return Err("未知の必須機能を含むファイルです".into());
                }
                extras.push(Chunk {
                    kind,
                    flags,
                    id,
                    data: data.to_vec(),
                });
            }
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|p| p[0].1 > p[1].0) {
        return Err("チャンクの範囲が重複しています".into());
    }
    let mut doc = doc.ok_or("文書チャンクがありません")?;
    if !doc.assets.is_empty() {
        return Err("素材の保存領域が重複しています".into());
    }
    doc.assets = assets.ok_or("素材チャンクがありません")?;
    doc.extra_chunks = extras;
    let mut styles = std::collections::HashMap::new();
    for (id, radius) in corners {
        if !radius.is_finite()
            || !(0.0..=10000.).contains(&radius)
            || styles.insert(id, radius).is_some()
        {
            return Err("接続線の角の設定が不正です".into());
        }
    }
    for c in doc
        .connectors
        .iter_mut()
        .chain(doc.pages.iter_mut().flat_map(|p| &mut p.connectors))
    {
        c.corner_radius = styles.remove(&c.id).unwrap_or(0.);
    }
    if !styles.is_empty() {
        return Err("角の設定に対応する接続線がありません".into());
    }
    Ok(doc)
}
#[derive(Deserialize)]
struct Legacy {
    nodes: Vec<Node>,
    connectors: Vec<Connector>,
    assets: Vec<Asset>,
    font_family: String,
    font_size: f64,
    grid: f64,
    show_grid: bool,
    snap: bool,
    jumps: bool,
    jump_radius: f64,
    next_id: Id,
}
impl Legacy {
    fn into_document(self) -> Document {
        Document {
            nodes: self.nodes,
            connectors: self.connectors,
            assets: self.assets,
            font_family: self.font_family,
            font_size: self.font_size,
            grid: self.grid,
            show_grid: self.show_grid,
            snap: self.snap,
            jumps: self.jumps,
            jump_radius: self.jump_radius,
            next_id: self.next_id,
            ..Default::default()
        }
    }
}
pub fn load(path: &Path) -> Result<Document> {
    let mut bytes = vec![];
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((LIMIT + 53) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode(&bytes)
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_control(path, bytes, &jobs::Control::default())
}
pub fn atomic_write_control(path: &Path, bytes: &[u8], control: &jobs::Control) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    for (i, chunk) in bytes.chunks(65536).enumerate() {
        control.checkpoint(
            75 + (i * 65536 * 20 / bytes.len().max(1)) as u32,
            "ファイルを書き込み中",
        )?;
        temp.write_all(chunk).map_err(|e| e.to_string())?;
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    control.begin_commit()?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn save_control(path: &Path, doc: &Document, control: &jobs::Control) -> Result<()> {
    control.checkpoint(5, "文書を検証・変換中")?;
    let bytes = encode(doc)?;
    control.checkpoint(70, "保存の準備")?;
    atomic_write_control(path, &bytes, control)
}
pub fn save(path: &Path, doc: &Document) -> Result<()> {
    atomic_write(path, &encode(doc)?)
}
