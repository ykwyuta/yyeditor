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
    /// 検索の URL（`%s` を検索語に。空なら設定ファイルの `[browser] search_url`。19 章 4.4）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub search_url: String,
    /// ダウンロードの保存先（空なら Windows の「ダウンロード」。19 章 4.3）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub download_dir: String,
    /// ダウンロードのたびに保存先を尋ねる
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub download_ask: bool,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<ProxyProfile>,
    /// 広告ブロックのフィルタリスト（全プロファイルで共通。20 章 3）
    #[serde(default)]
    pub adblock: AdblockConfig,
}

/// 広告ブロックのフィルタリスト 1 つ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterList {
    pub name: String,
    /// `http://`・`https://` か、ローカルのファイルのパス（`C:\…`・`file:///…`）
    pub url: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl FilterList {
    pub fn new(name: &str, url: &str, enabled: bool) -> FilterList {
        FilterList {
            name: name.to_owned(),
            url: url.to_owned(),
            enabled,
        }
    }

    /// ローカルのファイルか（ダウンロードしない）。
    pub fn is_local(&self) -> bool {
        let u = self.url.trim().to_ascii_lowercase();
        !(u.starts_with("http://") || u.starts_with("https://"))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("フィルタリストの名前を入力してください".into());
        }
        let u = self.url.trim();
        if u.is_empty() {
            return Err(format!(
                "「{}」の URL かファイルを入力してください",
                self.name
            ));
        }
        if u.chars().any(char::is_control) {
            return Err(format!("「{}」の URL に改行などは使えません", self.name));
        }
        let lower = u.to_ascii_lowercase();
        if lower.contains("://")
            && !(lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("file:///"))
        {
            return Err(format!(
                "「{}」の URL は http://・https://・file:/// かファイルのパスにしてください",
                self.name
            ));
        }
        Ok(())
    }
}

/// 広告ブロックの設定（`browser.toml` の `[adblock]`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdblockConfig {
    #[serde(default = "default_filter_lists", rename = "list")]
    pub lists: Vec<FilterList>,
}

impl Default for AdblockConfig {
    fn default() -> Self {
        AdblockConfig {
            lists: default_filter_lists(),
        }
    }
}

/// 既定のフィルタリスト（20 章 3.1）。同梱はせず、利用者の PC がダウンロードする。
pub fn default_filter_lists() -> Vec<FilterList> {
    vec![
        FilterList::new(
            "EasyList",
            "https://easylist.to/easylist/easylist.txt",
            true,
        ),
        FilterList::new(
            "EasyPrivacy",
            "https://easylist.to/easylist/easyprivacy.txt",
            true,
        ),
        FilterList::new(
            "AdGuard 日本語フィルタ",
            "https://filters.adtidy.org/extension/ublock/filters/7.txt",
            true,
        ),
        FilterList::new(
            "uBlock filters",
            "https://ublockorigin.github.io/uAssets/filters/filters.txt",
            false,
        ),
        FilterList::new(
            "EasyList Cookie List",
            "https://secure.fanboy.co.nz/fanboy-cookiemonster.txt",
            false,
        ),
    ]
}

impl Default for ProfileList {
    /// 初めて使うとき: 「OS と同じ」と「直接」。
    fn default() -> Self {
        ProfileList {
            default: "OS と同じ".into(),
            search_url: String::new(),
            download_dir: String::new(),
            download_ask: false,
            profiles: vec![
                ProxyProfile::new("OS と同じ", ProxyMode::System),
                ProxyProfile::new("直接", ProxyMode::Direct),
            ],
            adblock: AdblockConfig::default(),
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
            list.profiles = ProfileList::default().profiles;
            list.default = ProfileList::default().default;
        }
        Ok(list)
    }

    /// 保存する（どのプロファイルも確かめてから。一時ファイルから置き換える）。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        for p in &self.profiles {
            p.validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        }
        if !self.search_url.is_empty() {
            crate::history::validate_search_url(&self.search_url)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        }
        for l in &self.adblock.lists {
            l.validate()
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

    /// 使う検索の URL（`browser.toml` で選んだもの、なければ設定ファイルのもの）。
    pub fn effective_search_url<'a>(&'a self, config: &'a str) -> &'a str {
        if self.search_url.trim().is_empty() {
            config
        } else {
            &self.search_url
        }
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
        // 広告ブロックの一覧も保存する（切ったもの・足したもの）
        list.adblock.lists[0].enabled = false;
        list.adblock
            .lists
            .push(FilterList::new("自作", "C:\\filters\\my.txt", true));
        assert!(list.adblock.lists.last().unwrap().is_local());
        assert!(!list.adblock.lists[0].is_local());
        list.save(&p).unwrap();
        assert_eq!(ProfileList::load(&p).unwrap().adblock, list.adblock);
        // 検索の URL・ダウンロードの設定も保存する
        list.search_url = "https://duckduckgo.com/?q=%s".into();
        list.download_dir = "D:\\dl".into();
        list.download_ask = true;
        list.save(&p).unwrap();
        let back = ProfileList::load(&p).unwrap();
        assert_eq!(
            back.effective_search_url("x"),
            "https://duckduckgo.com/?q=%s"
        );
        assert_eq!(back.download_dir, "D:\\dl");
        assert!(back.download_ask);
        list.search_url = "https://bad/".into();
        assert!(list.save(&p).is_err());
        list.search_url.clear();
        assert_eq!(list.effective_search_url("cfg"), "cfg");
        // [adblock] がない古いファイルは既定の一覧
        std::fs::write(
            &p,
            "default = \"x\"\n[[profile]]\nname = \"x\"\nmode = \"direct\"\n",
        )
        .unwrap();
        let old = ProfileList::load(&p).unwrap();
        assert_eq!(old.adblock.lists, default_filter_lists());
        assert!(old.profiles[0].adblock_on("example.com"));
        assert!(
            FilterList::new("x", "javascript://x", true)
                .validate()
                .is_err()
        );
        assert!(
            FilterList::new("x", "file:///c:/a.txt", true)
                .validate()
                .is_ok()
        );
        std::fs::write(&p, "default = 1\n[[profile]]\nname = 3").unwrap();
        assert!(ProfileList::load(&p).is_err());
    }
}
