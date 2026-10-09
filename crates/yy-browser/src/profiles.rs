//! プロキシのプロファイルの一覧（設定のフォルダの `browser.toml`。19 章 3.2）。

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::proxy::{ProxyMode, ProxyProfile};

/// プロファイルの一覧。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileList {
    /// 起動するときのプロファイルの名前
    #[serde(default)]
    pub default: String,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<ProxyProfile>,
}

impl Default for ProfileList {
    /// 初めて使うとき: 「OS と同じ」と「直接」。
    fn default() -> Self {
        ProfileList {
            default: "OS と同じ".into(),
            profiles: vec![
                ProxyProfile::new("OS と同じ", ProxyMode::System),
                ProxyProfile::new("直接", ProxyMode::Direct),
            ],
        }
    }
}

impl ProfileList {
    /// 読む（なければ既定の一覧）。
    pub fn load(path: &Path) -> io::Result<ProfileList> {
        let mut list = match std::fs::read_to_string(path) {
            Ok(t) => toml::from_str::<ProfileList>(&t)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ProfileList::default()),
            Err(e) => return Err(e),
        };
        if list.profiles.is_empty() {
            list = ProfileList::default();
        }
        Ok(list)
    }

    /// 保存する（どのプロファイルも確かめてから。一時ファイルから置き換える）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        for p in &self.profiles {
            p.validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    pub fn get(&self, name: &str) -> Option<&ProxyProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// 起動するときのプロファイル（既定の名前がなければ先頭）。
    pub fn startup(&self) -> ProxyProfile {
        self.get(&self.default)
            .or(self.profiles.first())
            .cloned()
            .unwrap_or_else(|| ProxyProfile::new("OS と同じ", ProxyMode::System))
    }

    /// 足す・置き換える（`old` の名前のものを `p` にする。`old` が `None` なら足す）。同じ名前があればエラー。
    pub fn put(&mut self, old: Option<&str>, p: ProxyProfile) -> Result<(), String> {
        p.validate()?;
        let clash = self
            .profiles
            .iter()
            .any(|q| q.name == p.name && Some(q.name.as_str()) != old);
        if clash {
            return Err(format!(
                "「{}」という名前のプロファイルは既にあります",
                p.name
            ));
        }
        match old.and_then(|o| self.profiles.iter().position(|q| q.name == o)) {
            Some(i) => {
                if self.default == self.profiles[i].name {
                    self.default = p.name.clone();
                }
                self.profiles[i] = p;
            }
            None => self.profiles.push(p),
        }
        Ok(())
    }

    /// 消す（最後の 1 つは消さない）。
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if self.profiles.len() <= 1 {
            return Err("最後のプロファイルは消せません".into());
        }
        self.profiles.retain(|p| p.name != name);
        if self.default == name {
            self.default = self.profiles[0].name.clone();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_loads_and_edits_profiles() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("browser.toml");
        let mut list = ProfileList::load(&p).unwrap();
        assert_eq!(list, ProfileList::default());
        assert_eq!(list.startup().mode, ProxyMode::System);
        let fiddler = ProxyProfile {
            name: "検証用 Fiddler".into(),
            mode: ProxyMode::Manual,
            server: "127.0.0.1:8888".into(),
            bypass: "<local>".into(),
            ..ProxyProfile::default()
        };
        list.put(None, fiddler.clone()).unwrap();
        list.default = fiddler.name.clone();
        assert!(list.put(None, fiddler.clone()).is_err()); // 同じ名前
        list.save(&p).unwrap();
        let back = ProfileList::load(&p).unwrap();
        assert_eq!(back, list);
        assert_eq!(back.startup(), fiddler);
        // 名前を変えると既定も追う
        let mut renamed = fiddler.clone();
        renamed.name = "Fiddler".into();
        list.put(Some("検証用 Fiddler"), renamed).unwrap();
        assert_eq!(list.default, "Fiddler");
        // 正しくないものは足さない・保存しない
        let bad = ProxyProfile {
            name: "bad".into(),
            mode: ProxyMode::Manual,
            server: "nope".into(),
            ..ProxyProfile::default()
        };
        assert!(list.put(None, bad.clone()).is_err());
        let mut broken = list.clone();
        broken.profiles.push(bad);
        assert!(broken.save(&p).is_err());
        // 消す（既定を消したら先頭へ。最後の 1 つは消さない）
        list.remove("Fiddler").unwrap();
        assert_eq!(list.default, "OS と同じ");
        list.remove("直接").unwrap();
        assert!(list.remove("OS と同じ").is_err());
        std::fs::write(&p, "default = 1\n[[profile]]\nname = 3").unwrap();
        assert!(ProfileList::load(&p).is_err());
    }
}
