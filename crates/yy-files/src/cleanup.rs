//! Read-only discovery of reproducible development data. No deletion is performed here.
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use crate::fs::{Fs, Meta};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ecosystem {
    Python,
    Node,
    Java,
    Rust,
    DotNet,
    Docker,
    Podman,
}

impl Ecosystem {
    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::Node => "Node.js / JS",
            Self::Java => "Java",
            Self::Rust => "Rust",
            Self::DotNet => ".NET",
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    Folder(PathBuf),
    Image { engine: Ecosystem, id: String },
    BuildCache,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub ecosystem: Ecosystem,
    pub kind: String,
    pub location: Location,
    /// Logical bytes; image layers can be shared. This is not a reclaimable-space estimate.
    pub bytes: u64,
    pub modified: i64,
    pub files: u64,
    pub incomplete: bool,
    pub reason: String,
    pub advice: String,
}

impl Candidate {
    pub fn address(&self) -> String {
        match &self.location {
            Location::Folder(path) => path.display().to_string(),
            Location::Image { engine, id } => format!("{}: {id}", engine.label()),
            Location::BuildCache => format!("{}: build cache", self.ecosystem.label()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub min_bytes: u64,
    pub build_age_days: u32,
    pub now_nanos: i64,
    pub ecosystem: Option<Ecosystem>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_bytes: 100 * 1024 * 1024,
            build_age_days: 30,
            now_nanos: crate::fs::to_nanos(std::time::SystemTime::now()),
            ecosystem: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub items: Vec<Candidate>,
    pub warnings: Vec<String>,
    pub warning_count: u64,
    pub visited: u64,
}

impl Report {
    pub fn warn(&mut self, text: String) {
        self.warning_count += 1;
        if self.warnings.len() < 200 {
            self.warnings.push(text);
        }
    }

    pub fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| a.address().cmp(&b.address()))
        });
    }

    pub fn csv(&self) -> String {
        fn quote(s: &str) -> String {
            // CSV is opened in spreadsheets too: do not interpret paths or names as formulas.
            let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{s}")
            } else {
                s.to_owned()
            };
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        let mut text = String::from(
            "\u{feff}環境,種類,論理バイト数,更新日時(UNIXナノ秒),ファイル数,一部未集計,場所・ID,理由,整理方法\r\n",
        );
        for c in &self.items {
            let values = [
                c.ecosystem.label().to_owned(),
                c.kind.clone(),
                c.bytes.to_string(),
                c.modified.to_string(),
                c.files.to_string(),
                c.incomplete.to_string(),
                c.address(),
                c.reason.clone(),
                c.advice.clone(),
            ];
            text.push_str(
                &values
                    .iter()
                    .map(|s| quote(s))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            text.push_str("\r\n");
        }
        text
    }
}

#[derive(Clone, Debug)]
pub struct CacheRoot {
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
    pub name: &'static str,
}

/// Conventional user caches, plus explicit pip/npm/Gradle cache overrides. No tools are executed.
pub fn user_caches(env: impl Fn(&str) -> Option<PathBuf>) -> Vec<CacheRoot> {
    let home = env("USERPROFILE").or_else(|| env("HOME"));
    let mut roots = Vec::new();
    let mut add = |path: Option<PathBuf>, ecosystem, name| {
        if let Some(path) = path {
            roots.push(CacheRoot {
                path,
                ecosystem,
                name,
            });
        }
    };
    let at = |base: &Option<PathBuf>, suffix: &str| base.as_ref().map(|p| p.join(suffix));
    let local = env("LOCALAPPDATA");
    let roaming = env("APPDATA");
    add(
        env("PIP_CACHE_DIR")
            .or_else(|| at(&local, "pip/Cache"))
            .or_else(|| at(&home, ".cache/pip")),
        Ecosystem::Python,
        "pip キャッシュ",
    );
    add(at(&home, ".cache/uv"), Ecosystem::Python, "uv キャッシュ");
    add(at(&local, "uv/cache"), Ecosystem::Python, "uv キャッシュ");
    add(
        env("npm_config_cache")
            .or_else(|| env("NPM_CONFIG_CACHE"))
            .or_else(|| at(&local, "npm-cache"))
            .or_else(|| at(&home, ".npm")),
        Ecosystem::Node,
        "npm キャッシュ",
    );
    add(at(&local, "pnpm/store"), Ecosystem::Node, "pnpm ストア");
    add(at(&local, "Yarn/Cache"), Ecosystem::Node, "Yarn キャッシュ");
    add(at(&roaming, "npm-cache"), Ecosystem::Node, "npm キャッシュ");
    let gradle = env("GRADLE_USER_HOME").or_else(|| at(&home, ".gradle"));
    add(at(&gradle, "caches"), Ecosystem::Java, "Gradle キャッシュ");
    add(
        at(&gradle, "wrapper/dists"),
        Ecosystem::Java,
        "Gradle 配布キャッシュ",
    );
    add(
        at(&home, ".m2/repository"),
        Ecosystem::Java,
        "Maven ローカルリポジトリ",
    );
    add(
        at(&home, ".nuget/packages"),
        Ecosystem::DotNet,
        "NuGet キャッシュ",
    );
    roots
}

#[derive(Clone, Copy)]
struct Rule {
    ecosystem: Ecosystem,
    name: &'static str,
    build: bool,
    advice: &'static str,
}

fn rule(path: &Path, meta: Meta, names: &HashSet<String>) -> Option<Rule> {
    if !meta.dir {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let has = |s: &str| names.contains(s);
    let python = has("pyproject.toml") || has("setup.py") || has("setup.cfg");
    let node = has("package.json");
    let gradle = has("build.gradle") || has("build.gradle.kts");
    let dotnet = names
        .iter()
        .any(|s| s.ends_with(".csproj") || s.ends_with(".fsproj") || s.ends_with(".vbproj"));
    let (ecosystem, name, build, advice) = match name {
        "node_modules" => (
            Ecosystem::Node,
            "依存パッケージ",
            false,
            "削除前に作業を停止し、npm ci / pnpm install / yarn install で再取得できることを確認。手修正したファイルにも注意。",
        ),
        ".npm" | ".pnpm-store" | ".yarn-cache" => (
            Ecosystem::Node,
            "パッケージキャッシュ",
            false,
            "再ダウンロードが必要になります。利用中のパッケージマネージャーを終了してから整理。",
        ),
        ".next" | ".nuxt" | ".svelte-kit" | ".parcel-cache" | ".turbo" | ".vite" if node => (
            Ecosystem::Node,
            "JS ビルド・キャッシュ",
            true,
            "開発サーバーを停止してから整理。npm run build 等で再生成。",
        ),
        "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" => (
            Ecosystem::Python,
            "Python キャッシュ",
            false,
            "実行・テスト・解析時に再生成されます。利用中のプロセスを停止してから整理。",
        ),
        ".tox" | ".nox" if python || has("tox.ini") || has("noxfile.py") => (
            Ecosystem::Python,
            "テスト環境",
            false,
            "tox / nox の設定と再生成可能性を確認してから整理。",
        ),
        ".gradle" if gradle => (
            Ecosystem::Java,
            "Gradle プロジェクトキャッシュ",
            false,
            "Gradle の処理・デーモンを停止してから整理。次回のビルドで再生成。",
        ),
        "target" if has("pom.xml") => (
            Ecosystem::Java,
            "Maven ビルド成果物",
            true,
            "未配布の成果物が不要か確認。mvn package 等で再生成。",
        ),
        "target" if has("Cargo.toml") => (
            Ecosystem::Rust,
            "Cargo ビルド成果物",
            true,
            "必要な実行ファイルを退避し、cargo build で再生成できることを確認。",
        ),
        "build" if gradle => (
            Ecosystem::Java,
            "Gradle ビルド成果物",
            true,
            "未配布の成果物が不要か確認。gradle build 等で再生成。",
        ),
        "bin" | "obj" if dotnet => (
            Ecosystem::DotNet,
            ".NET ビルド成果物",
            true,
            "実行中のアプリを停止し、dotnet build で再生成できることを確認。",
        ),
        "build" | "dist" if python => (
            Ecosystem::Python,
            "Python 配布・ビルド成果物",
            true,
            "未公開のwheel・配布物を退避。python -m build 等で再生成。",
        ),
        "build" | "dist" if node => (
            Ecosystem::Node,
            "JS 配布・ビルド成果物",
            true,
            "配布先で使用中でないか、手作業の成果物を含まないか確認。npm run build 等で再生成。",
        ),
        _ => return None,
    };
    Some(Rule {
        ecosystem,
        name,
        build,
        advice,
    })
}

fn protected(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    parts
        .iter()
        .any(|s| s == ".git" || s == ".svn" || s == ".hg" || s == ".yyfm-trash")
        || parts.windows(2).any(|p| {
            (p[0] == "containers" && p[1] == "storage")
                || (p[0] == "docker"
                    && [
                        "overlay2",
                        "volumes",
                        "image",
                        "containers",
                        "wsl",
                        "windowsfilter",
                        "buildkit",
                        "containerd",
                    ]
                    .contains(&p[1].as_str()))
                || (p[0] == "podman" && p[1] == "machine")
        })
        || parts.windows(3).any(|p| {
            p[0] == "var" && p[1] == "lib" && ["docker", "containerd"].contains(&p[2].as_str())
        })
        || parts
            .windows(2)
            .any(|p| p[0] == "programdata" && p[1] == "docker")
}

fn tick(
    report: &mut Report,
    path: &Path,
    progress: &impl Fn(u64, &Path) -> bool,
) -> io::Result<()> {
    report.visited += 1;
    if !progress(report.visited, path) {
        return Err(crate::cancelled());
    }
    Ok(())
}

fn measure(
    fs: &dyn Fs,
    path: &Path,
    report: &mut Report,
    progress: &impl Fn(u64, &Path) -> bool,
) -> io::Result<(u64, u64, i64, bool)> {
    let mut stack = vec![path.to_owned()];
    let (mut bytes, mut files, mut latest, mut incomplete) = (0u64, 0u64, 0i64, false);
    while let Some(path) = stack.pop() {
        tick(report, &path, progress)?;
        let meta = match fs.metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                incomplete = true;
                report.warn(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if meta.link {
            continue;
        }
        latest = latest.max(meta.mtime);
        if !meta.dir {
            bytes = bytes.saturating_add(meta.size);
            files += 1;
        } else {
            match fs.read_dir(&path) {
                Ok(entries) => stack.extend(
                    entries
                        .into_iter()
                        .filter(|e| !e.meta.link)
                        .map(|e| path.join(e.name)),
                ),
                Err(e) => {
                    incomplete = true;
                    report.warn(format!("{}: {e}", path.display()));
                }
            }
        }
    }
    Ok((bytes, files, latest, incomplete))
}

/// Scan chosen roots and optional user caches. Candidate subtrees are measured once, without nested duplicates.
pub fn scan(
    fs: &dyn Fs,
    roots: &[PathBuf],
    caches: &[CacheRoot],
    options: &Options,
    progress: &impl Fn(u64, &Path) -> bool,
) -> io::Result<Report> {
    let mut report = Report::default();
    let mut normalized = Vec::new();
    for root in roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        if fs.metadata(root).is_ok_and(|m| m.link) {
            continue;
        }
        match root.canonicalize() {
            Ok(root) if !normalized.contains(&root) => normalized.push(root),
            Ok(_) => {}
            Err(e) => report.warn(format!("{}: {e}", root.display())),
        }
    }
    // Parent roots first; overlapping roots must never produce duplicate candidates.
    normalized.sort_by_key(|p| p.components().count());
    let mut unique: Vec<PathBuf> = Vec::new();
    for root in normalized {
        if !unique.iter().any(|p| root.starts_with(p)) {
            unique.push(root);
        }
    }
    let mut forced = BTreeMap::new();
    for cache in caches {
        if options.ecosystem.is_some_and(|e| e != cache.ecosystem) {
            continue;
        }
        if !cache.path.exists() {
            continue;
        }
        // Conventional cache discovery must not follow an unexpected junction outside the profile.
        if cache
            .path
            .ancestors()
            .any(|p| fs.metadata(p).is_ok_and(|m| m.link))
        {
            continue;
        }
        match cache.path.canonicalize() {
            Ok(path) => {
                forced.insert(path.clone(), Rule { ecosystem: cache.ecosystem, name: cache.name, build: false, advice: "利用中のツールを停止してから整理。ローカルのみのパッケージが含まれないか確認。再取得にはネットワークが必要です。" });
                if !unique.iter().any(|p| path.starts_with(p)) {
                    unique.push(path);
                }
            }
            Err(e) => report.warn(format!("{}: {e}", cache.path.display())),
        }
    }
    unique.sort_by_key(|p| p.components().count());
    let mut bases: Vec<PathBuf> = Vec::new();
    for root in unique {
        if !bases.iter().any(|p| root.starts_with(p)) {
            bases.push(root);
        }
    }
    let mut stack: Vec<_> = bases
        .into_iter()
        .map(|p| {
            let siblings = p
                .parent()
                .and_then(|p| fs.read_dir(p).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|e| !e.meta.dir && !e.meta.link)
                .map(|e| e.name)
                .collect();
            (p, siblings)
        })
        .collect();
    let mut visited_roots = HashSet::new();
    let cutoff = options
        .now_nanos
        .saturating_sub(i64::from(options.build_age_days).saturating_mul(86_400_000_000_000));
    while let Some((path, siblings)) = stack.pop() {
        tick(&mut report, &path, progress)?;
        if protected(&path) || !visited_roots.insert(path.clone()) {
            continue;
        }
        let meta = match fs.metadata(&path) {
            Ok(m) if m.dir && !m.link => m,
            Ok(_) => continue,
            Err(e) => {
                report.warn(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let mut found = forced
            .get(&path)
            .copied()
            .or_else(|| rule(&path, meta, &siblings));
        if found.is_none()
            && matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(".venv" | "venv")
            )
            && fs
                .metadata(&path.join("pyvenv.cfg"))
                .is_ok_and(|m| !m.dir && !m.link)
        {
            found = Some(Rule {
                ecosystem: Ecosystem::Python,
                name: "Python 仮想環境",
                build: false,
                advice: "利用中の環境・独自パッケージがないか確認。python -m venv と依存定義で再作成。",
            });
        }
        if let Some(rule) = found {
            if options.ecosystem.is_some_and(|e| e != rule.ecosystem) {
                continue;
            }
            let (bytes, files, modified, incomplete) = measure(fs, &path, &mut report, progress)?;
            if bytes >= options.min_bytes
                && (!rule.build
                    || options.build_age_days == 0
                    || (!incomplete && modified > 0 && modified <= cutoff))
            {
                report.items.push(Candidate {
                    ecosystem: rule.ecosystem,
                    kind: rule.name.into(),
                    location: Location::Folder(path),
                    bytes,
                    files,
                    modified,
                    incomplete,
                    reason: if rule.build {
                        format!(
                            "プロジェクト定義あり・配下の最新更新から {} 日以上",
                            options.build_age_days
                        )
                    } else {
                        "再生成可能な依存関係・キャッシュの標準配置".into()
                    },
                    advice: rule.advice.into(),
                });
            }
            continue;
        }
        match fs.read_dir(&path) {
            Ok(entries) => {
                let names: HashSet<_> = entries
                    .iter()
                    .filter(|e| !e.meta.dir && !e.meta.link)
                    .map(|e| e.name.clone())
                    .collect();
                for entry in entries.into_iter().filter(|e| e.meta.dir && !e.meta.link) {
                    stack.push((path.join(entry.name), names.clone()));
                }
            }
            Err(e) => report.warn(format!("{}: {e}", path.display())),
        }
    }
    report.sort();
    Ok(report)
}

fn image_id(value: &str) -> Option<&str> {
    let id = value.strip_prefix("sha256:").unwrap_or(value);
    (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// CLI sizes use SI units (MB), not the binary units used for the UI's MiB threshold.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let value: f64 = text[..split].parse().ok()?;
    let multiplier = match text[split..].trim().to_ascii_lowercase().as_str() {
        "b" | "" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1048576.0,
        "gib" => 1073741824.0,
        _ => return None,
    };
    let bytes = value * multiplier;
    (bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64).then_some(bytes as u64)
}

/// Query tools through an injected runner. Only list/inspect/df commands are issued.
pub fn containers(
    engine: Ecosystem,
    min_bytes: u64,
    run: &impl Fn(&str, &[&str]) -> io::Result<String>,
) -> io::Result<Report> {
    let (tool, format) = match engine {
        Ecosystem::Docker => (
            "docker",
            "{{.ID}}\t{{.Repository}}:{{.Tag}}\t{{.Size}}\t{{.CreatedAt}}",
        ),
        Ecosystem::Podman => (
            "podman",
            "{{.Id}}\t{{.Repository}}:{{.Tag}}\t{{.Size}}\t{{.CreatedAt}}",
        ),
        _ => return Err(io::Error::other("コンテナエンジンを指定してください")),
    };
    let images = run(
        tool,
        &["image", "ls", "--all", "--no-trunc", "--format", format],
    )?;
    let mut report = Report::default();
    let usage = (|| -> io::Result<HashSet<String>> {
        let ids = run(tool, &["container", "ls", "--all", "--quiet", "--no-trunc"])?;
        let ids: Vec<_> = ids
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if ids.iter().any(|s| image_id(s).is_none()) {
            return Err(io::Error::other("不正なコンテナID"));
        }
        let mut used = HashSet::new();
        for batch in ids.chunks(64) {
            let mut args = vec!["container", "inspect", "--format", "{{.Image}}"];
            args.extend_from_slice(batch);
            let output = run(tool, &args)?;
            let parsed: Vec<_> = output
                .lines()
                .filter(|s| !s.trim().is_empty())
                .map(|s| image_id(s.trim()))
                .collect();
            if parsed.len() != batch.len() || parsed.iter().any(Option::is_none) {
                return Err(io::Error::other("コンテナ参照の応答が不完全です"));
            }
            used.extend(parsed.into_iter().flatten().map(str::to_ascii_lowercase));
        }
        Ok(used)
    })();
    if usage.as_ref().is_err_and(crate::is_cancelled) {
        return Err(crate::cancelled());
    }
    match usage {
        Err(e) => report.warn(format!(
            "{tool}: 利用状況が不明なためイメージ候補を抽出しません: {e}"
        )),
        Ok(used) => {
            let mut by_id: BTreeMap<String, Candidate> = BTreeMap::new();
            for line in images.lines().filter(|s| !s.trim().is_empty()) {
                let fields: Vec<_> = line.split('\t').collect();
                let parsed = fields
                    .first()
                    .and_then(|s| image_id(s))
                    .zip(fields.get(2).and_then(|s| parse_size(s)));
                let Some((id, bytes)) = parsed.filter(|_| fields.len() == 4) else {
                    report.warn(format!("{tool}: イメージ情報の形式を解釈できません"));
                    continue;
                };
                let id = id.to_ascii_lowercase();
                if used.contains(&id) || bytes < min_bytes {
                    continue;
                }
                if let Some(existing) = by_id.get_mut(&id) {
                    existing.reason.push_str(&format!(" / {}", fields[1]));
                    continue;
                }
                by_id.insert(
                    id.clone(),
                    Candidate {
                        ecosystem: engine,
                        kind: "未参照イメージ".into(),
                        location: Location::Image {
                            engine,
                            id: id.clone(),
                        },
                        bytes,
                        modified: 0,
                        files: 0,
                        incomplete: false,
                        reason: format!(
                            "停止中を含むコンテナ参照なし。{} / 作成 {}。共有レイヤー含む",
                            fields[1], fields[3]
                        ),
                        advice: format!("{tool} image rm {id}"),
                    },
                );
            }
            report.items.extend(by_id.into_values());
        }
    }
    match run(
        tool,
        &[
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
        ],
    ) {
        Ok(output) => {
            for line in output.lines() {
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() == 3
                    && fields[0] == "Build Cache"
                    && let Some(bytes) = parse_size(fields[1]).filter(|n| *n >= min_bytes)
                {
                    report.items.push(Candidate {
                        ecosystem: engine,
                        kind: "ビルドキャッシュ（合計）".into(),
                        location: Location::BuildCache,
                        bytes,
                        modified: 0,
                        files: 0,
                        incomplete: false,
                        reason: format!(
                            "エンジンが報告する再利用可能量: {}。全容量が解放されるとは限りません",
                            fields[2]
                        ),
                        advice: format!("{tool} builder prune"),
                    });
                }
            }
        }
        Err(e) => {
            if crate::is_cancelled(&e) {
                return Err(e);
            }
            report.warn(format!("{tool}: キャッシュ容量を取得できません: {e}"));
        }
    }
    report.sort();
    Ok(report)
}
