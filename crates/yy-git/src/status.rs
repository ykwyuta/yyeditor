//! `git status --porcelain=v2 --branch -z` を読む。

/// 変更の一覧の分け方（VS Code のソース管理と同じ）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// マージの競合
    Conflict,
    /// ステージした変更
    Staged,
    /// 作業ツリーの変更（追跡していないファイルを含む）
    Unstaged,
}

/// 1 つのファイルの変更。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// 作業ツリーの起点からの相対パス（区切りは `/`）
    pub path: String,
    /// 名前の変更・コピーの元
    pub orig: Option<String>,
    /// インデックスの状態（`M`・`A`・`D`・`R`・`C`・`T`・`U`、変わっていなければ `.`、追跡外は `?`）
    pub x: char,
    /// 作業ツリーの状態（同じ）
    pub y: char,
}

impl Change {
    pub fn untracked(&self) -> bool {
        self.x == '?'
    }

    pub fn conflict(&self) -> bool {
        matches!(
            (self.x, self.y),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    /// ステージした変更があるか。
    pub fn staged(&self) -> bool {
        !self.conflict() && !self.untracked() && self.x != '.'
    }

    /// 作業ツリーの変更があるか（追跡していないファイルを含む）。
    pub fn unstaged(&self) -> bool {
        !self.conflict() && (self.untracked() || self.y != '.')
    }

    /// 入る分け方（ステージした変更と作業ツリーの変更の両方にあるファイルは 2 つ）。
    pub fn groups(&self) -> Vec<Group> {
        if self.conflict() {
            return vec![Group::Conflict];
        }
        let mut v = Vec::new();
        if self.staged() {
            v.push(Group::Staged);
        }
        if self.unstaged() {
            v.push(Group::Unstaged);
        }
        v
    }

    /// 一覧に出す 1 文字（VS Code と同じ: M 変更・A 追加・D 削除・R 名前の変更・U 追跡外・C 競合）。
    pub fn letter(&self, group: Group) -> char {
        match group {
            Group::Conflict => '!',
            Group::Staged => match self.x {
                'R' => 'R',
                'C' => 'C',
                'A' => 'A',
                'D' => 'D',
                'T' => 'T',
                _ => 'M',
            },
            Group::Unstaged => match self.y {
                _ if self.untracked() => 'U',
                'D' => 'D',
                'T' => 'T',
                'A' => 'A',
                _ => 'M',
            },
        }
    }

    /// 状態の説明。
    pub fn describe(&self, group: Group) -> &'static str {
        match self.letter(group) {
            '!' => "競合",
            'R' => "名前の変更",
            'C' => "コピー",
            'A' => "追加",
            'D' => "削除",
            'T' => "種類の変更",
            'U' => "追跡されていない",
            _ => "変更",
        }
    }
}

/// リポジトリの状態。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// 今のブランチ（HEAD が切り離されていれば `None`）
    pub branch: Option<String>,
    /// HEAD のコミット（まだなければ `None`）
    pub head: Option<String>,
    /// 上流（`origin/main`）
    pub upstream: Option<String>,
    /// 上流より先（プッシュしていない）コミットの数
    pub ahead: u32,
    /// 上流より遅れている（プルしていない）コミットの数
    pub behind: u32,
    pub changes: Vec<Change>,
}

impl Status {
    /// 分け方 `group` に入る変更（パスの順）。
    pub fn in_group(&self, group: Group) -> Vec<&Change> {
        let mut v: Vec<&Change> = self
            .changes
            .iter()
            .filter(|c| c.groups().contains(&group))
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    /// ステージした変更があるか。
    pub fn has_staged(&self) -> bool {
        self.changes.iter().any(Change::staged)
    }

    /// ブランチの表示（`main ↑1 ↓2`・`(HEAD 1a2b3c4)`）。
    pub fn branch_label(&self) -> String {
        let mut s = match (&self.branch, &self.head) {
            (Some(b), _) => b.clone(),
            (None, Some(h)) => format!("(HEAD {})", &h[..h.len().min(7)]),
            (None, None) => "(HEAD)".into(),
        };
        if self.upstream.is_some() && (self.ahead > 0 || self.behind > 0) {
            s.push_str(&format!(" ↑{} ↓{}", self.ahead, self.behind));
        }
        s
    }
}

/// `git status --porcelain=v2 --branch -z` の出力を読む。
pub fn parse_status(out: &[u8]) -> Result<Status, String> {
    let mut st = Status::default();
    let text = String::from_utf8_lossy(out);
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let bad = || format!("git status の出力を読めません: {rec}");
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, value) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" => st.head = (value != "(initial)").then(|| value.to_string()),
                "branch.head" => st.branch = (value != "(detached)").then(|| value.to_string()),
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for p in value.split(' ') {
                        if let Some(n) = p.strip_prefix('+') {
                            st.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = p.strip_prefix('-') {
                            st.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let (kind, rest) = rec.split_at(1);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let xy = |s: &str| {
            let mut c = s.chars();
            (c.next().unwrap_or('.'), c.next().unwrap_or('.'))
        };
        match kind {
            // 1 XY sub mH mI mW hH hI path
            "1" => {
                let f: Vec<&str> = rest.splitn(8, ' ').collect();
                if f.len() < 8 {
                    return Err(bad());
                }
                let (x, y) = xy(f[0]);
                st.changes.push(Change {
                    path: f[7].to_string(),
                    orig: None,
                    x,
                    y,
                });
            }
            // 2 XY sub mH mI mW hH hI Xscore path NUL origPath
            "2" => {
                let f: Vec<&str> = rest.splitn(9, ' ').collect();
                if f.len() < 9 {
                    return Err(bad());
                }
                let (x, y) = xy(f[0]);
                let orig = records.next().map(str::to_string);
                st.changes.push(Change {
                    path: f[8].to_string(),
                    orig,
                    x,
                    y,
                });
            }
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            "u" => {
                let f: Vec<&str> = rest.splitn(10, ' ').collect();
                if f.len() < 10 {
                    return Err(bad());
                }
                let (x, y) = xy(f[0]);
                st.changes.push(Change {
                    path: f[9].to_string(),
                    orig: None,
                    x,
                    y,
                });
            }
            "?" => st.changes.push(Change {
                path: rest.to_string(),
                orig: None,
                x: '?',
                y: '?',
            }),
            // 無視したファイル
            "!" => {}
            _ => return Err(bad()),
        }
    }
    Ok(st)
}
