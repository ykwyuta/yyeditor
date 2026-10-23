use crate::*;
use std::{
    io::{self, Read, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"YY3D\x01\r\n\0";
const LIMIT: usize = 64 * 1024 * 1024;
pub fn encode(project: &Project) -> io::Result<Vec<u8>> {
    project.validate()?;
    let json = serde_json::to_vec(project).map_err(io::Error::other)?;
    if json.len() > LIMIT {
        return Err(crate::invalid("プロジェクトが保存上限を超えました"));
    }
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend(miniz_oxide::deflate::compress_to_vec_zlib(&json, 6));
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> io::Result<Project> {
    if bytes.len() < 12 || bytes.len() > LIMIT || &bytes[..8] != MAGIC {
        return Err(crate::invalid("YY3Designer形式・バージョンが違います"));
    }
    let expected = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if expected > LIMIT {
        return Err(crate::invalid("展開後のサイズが上限を超えます"));
    }
    let json = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&bytes[12..], LIMIT)
        .map_err(|e| crate::invalid(format!("破損した圧縮データ: {e:?}")))?;
    if json.len() != expected {
        return Err(crate::invalid("保存データの長さが一致しません"));
    }
    let project: Project = serde_json::from_slice(&json).map_err(io::Error::other)?;
    project.validate()?;
    Ok(project)
}
pub fn save(path: &Path, project: &Project) -> io::Result<()> {
    let bytes = encode(project)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn load(path: &Path) -> io::Result<Project> {
    let file = std::fs::File::open(path)?;
    let mut bytes = vec![];
    file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    decode(&bytes)
}
