//! Read-only identification. Managed data is never labelled disposable.
use super::*;

pub(super) fn advice(kind: Ecosystem) -> &'static str {
    match kind {
        Ecosystem::VirtualMachine => {
            "ゲスト内の不要データを整理し、バックアップ・スナップショットの関係を確認してVMを完全停止。Hyper-Vのディスク編集（圧縮）、VirtualBox/VMwareのディスク圧縮、WSLの仮想ディスク整理を使用。動的拡張ディスクでも縮小可能量は未測定です。差分ディスクを単体削除しないでください。"
        }
        Ecosystem::Docker | Ecosystem::Podman => {
            "エンジンの使用量・イメージ・ビルドキャッシュを確認して不要分を整理し、その後Docker Desktop等のディスク領域回収機能を使用。稼働中の仮想ディスクやvolumeを直接削除しないでください。未使用volumeにも永続データがあります。"
        }
        Ecosystem::Mobile => {
            "Apple Devices/iTunesの「バックアップを管理」、Android StudioのDevice Manager、端末メーカーのバックアップ管理から、対象端末・世代と復元の必要性を確認して整理。写真・認証情報等の唯一のコピーを含む場合があります。再生成可能なキャッシュとは異なります。"
        }
        Ecosystem::Database => {
            "所有アプリで不要データを整理してから、SQLiteのVACUUM、メールアプリのデータファイル圧縮、DBの正規のバックアップ・ログ管理等を検討。バックアップと作業空き容量を確保し、稼働中のDB/WAL/トランザクションログを直接削除しないでください。"
        }
        Ecosystem::Media => {
            "所有アプリで不要なプロジェクト・書き出し・レンダーキャッシュを確認。動画・音声は解像度や品質を調整した別ファイルへの書き出しで縮小可能な場合があります（不可逆の劣化に注意）。原本・再取得手段を確保してから整理。"
        }
        Ecosystem::Browser => {
            "ブラウザーや所有アプリの設定からキャッシュを整理。履歴・ログイン・オフラインデータを含むプロファイル全体は削除しないでください。アプリ停止後に対象を確認。"
        }
        Ecosystem::System => {
            "Windowsの設定→システム→ストレージ→一時ファイル、記憶域センサー、所有アプリのログローテーション・保持期間設定で整理。システム管理ファイルは直接削除せず、必要な診断ログは退避。"
        }
        Ecosystem::Other => {
            "用途・作成元・バックアップを確認する大容量データです。不要とは判定していません。所有アプリの管理機能で旧版・バックアップの世代・保持期間を整理し、圧縮や外部媒体への移動を検討。"
        }
        _ => {
            "利用中のツールを停止してから整理。ローカルのみの依存・未配布成果物を退避し、定義ファイルから再取得・再生成できることを確認してください。"
        }
    }
}

fn item(ecosystem: Ecosystem, name: &'static str, build: bool) -> Rule {
    Rule {
        ecosystem,
        name,
        build,
        advice: advice(ecosystem),
    }
}

pub(super) fn directory(path: &Path, names: &HashSet<String>) -> Option<Rule> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let full = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let has = |marker: &str| names.iter().any(|n| n.eq_ignore_ascii_case(marker));
    let (owner, label, build) = match name.as_str() {
        ".hypothesis" | ".pytype" | ".pyre" | ".pyright" | ".ipynb_checkpoints" => {
            (Ecosystem::Python, "Python 解析・Notebookキャッシュ", false)
        }
        ".eggs" if has("setup.py") || has("pyproject.toml") => {
            (Ecosystem::Python, "Python ビルド依存", true)
        }
        ".angular" | ".eslintcache" | ".stylelintcache" | ".nyc_output" | ".rpt2_cache"
            if has("package.json") =>
        {
            (Ecosystem::Node, "JS 解析・テストキャッシュ", false)
        }
        "coverage" | ".output" | ".astro" | "storybook-static" | "jspm_packages"
            if has("package.json") =>
        {
            (Ecosystem::Node, "JS テスト・配布成果物", true)
        }
        ".bloop" | ".metals" | ".scala-build" | ".kotlin" => {
            (Ecosystem::Java, "JVM 言語・解析キャッシュ", false)
        }
        ".cpcache" if has("deps.edn") => (Ecosystem::Java, "Clojure クラスパスキャッシュ", false),
        "vendor" if has("go.mod") => (Ecosystem::Go, "Go vendor依存", false),
        "vendor" if has("composer.json") => (Ecosystem::Php, "Composer依存", false),
        ".phpunit.cache" | ".php-cs-fixer.cache" if has("composer.json") => {
            (Ecosystem::Php, "PHP テスト・解析キャッシュ", false)
        }
        "cmakefiles" | ".cache-clangd" | "deriveddatacache" => {
            (Ecosystem::Native, "C/C++・ゲーム生成キャッシュ", false)
        }
        ".vs"
            if names
                .iter()
                .any(|n| n.ends_with(".sln") || n.ends_with(".slnx")) =>
        {
            (Ecosystem::Native, "Visual Studio生成キャッシュ", false)
        }
        "build" | "out" if has("CMakeLists.txt") || has("meson.build") => {
            (Ecosystem::Native, "C/C++ ビルド成果物", true)
        }
        n if n.starts_with("cmake-build-") && has("CMakeLists.txt") => {
            (Ecosystem::Native, "CMake ビルド成果物", true)
        }
        ".dart_tool" | ".pub-cache" => (Ecosystem::Mobile, "Dart/Flutterキャッシュ", false),
        "pods" if has("Podfile") => (Ecosystem::Mobile, "CocoaPods依存", false),
        ".build" if has("Package.swift") => (Ecosystem::Mobile, "Swift ビルド成果物", true),
        "mobilesync" | "smart switch" | "smart_switch" | "hisuite" => (
            Ecosystem::Mobile,
            "携帯バックアップ・管理データ（要確認）",
            false,
        ),
        "backup" if full.contains("/mobilesync/") => {
            (Ecosystem::Mobile, "iPhone/iPadバックアップ", false)
        }
        "avd" if full.contains("/.android/") => (Ecosystem::Mobile, "Android仮想端末", false),
        "virtualbox vms" | "virtual machines" | "parallels" => {
            (Ecosystem::VirtualMachine, "仮想マシン管理データ", false)
        }
        "shadercache" if full.contains("steam") => {
            (Ecosystem::Native, "ゲームシェーダーキャッシュ", false)
        }
        "cache" | "code cache" | "gpucache" | "cachestorage" | "media cache"
        | "media cache files"
            if [
                "chrome",
                "edge",
                "firefox",
                "bravesoftware",
                "discord",
                "slack",
                "teams",
                "spotify",
                "adobe",
            ]
            .iter()
            .any(|s| full.contains(s)) =>
        {
            (Ecosystem::Browser, "アプリ生成キャッシュ", false)
        }
        "crashdumps" | "minidump" | "wer" => (Ecosystem::System, "クラッシュ・診断データ", false),
        _ => return None,
    };
    Some(item(owner, label, build))
}

pub(super) fn file(path: &Path, bytes: u64) -> Option<Rule> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let (owner, label) = match ext.as_str() {
        "vhd" | "vhdx" | "avhd" | "avhdx" | "vdi" | "vmdk" | "qcow" | "qcow2" | "hdd" | "vmem"
        | "vmemstore" | "vmss" | "vmsn" | "ova" | "ovf" => (
            Ecosystem::VirtualMachine,
            "仮想ディスク・VM状態・配布イメージ",
        ),
        "pst" | "ost" | "mbox" | "db" | "sqlite" | "sqlite3" | "mdb" | "accdb" | "mdf" | "ldf"
        | "ibd" | "ibdata" | "nsf" => (
            Ecosystem::Database,
            "DB・メール管理ファイル（アプリで整理）",
        ),
        "bak" | "backup" | "tib" | "tibx" | "mrimg" | "vib" | "vbk" => {
            (Ecosystem::Other, "バックアップ・世代データ（要確認）")
        }
        "ipsw" | "ab" => (Ecosystem::Mobile, "端末バックアップ・復元イメージ"),
        "dmp" | "mdmp" | "hdmp" | "hprof" | "etl" | "evtx" | "log" => {
            (Ecosystem::System, "ログ・ダンプ・診断データ")
        }
        "iso" | "img" | "wim" | "esd" | "dmg" | "zip" | "7z" | "rar" | "tar" | "gz" | "bz2"
        | "xz" | "zst" | "msi" | "msix" | "msixbundle" | "cab" => {
            (Ecosystem::Media, "配布・圧縮・ディスクイメージ（要確認）")
        }
        "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "mxf" | "wav" | "flac" | "aiff"
        | "psd" | "psb" | "blend" => (Ecosystem::Media, "動画・音声・制作データ（縮小を検討）"),
        "csv" | "tsv" | "json" | "jsonl" | "ndjson" | "xml" | "parquet" | "arrow" | "feather"
        | "orc" | "avro" | "npy" | "npz" | "h5" | "hdf5" | "safetensors" | "gguf" | "pt"
        | "pth" | "onnx" | "ckpt"
            if bytes >= 100 * 1024 * 1024 =>
        {
            (Ecosystem::Other, "大容量データセット・AIモデル（要確認）")
        }
        _ if matches!(
            name.as_str(),
            "hiberfil.sys" | "pagefile.sys" | "swapfile.sys" | "memory.dmp"
        ) =>
        {
            (Ecosystem::System, "Windows管理ファイル（直接削除不可）")
        }
        _ if bytes >= 1024 * 1024 * 1024 => (Ecosystem::Other, "大容量ファイル（要確認）"),
        _ => return None,
    };
    Some(item(owner, label, false))
}

pub(super) fn common_roots(env: &impl Fn(&str) -> Option<PathBuf>, roots: &mut Vec<CacheRoot>) {
    let home = env("USERPROFILE").or_else(|| env("HOME"));
    let local = env("LOCALAPPDATA");
    let roaming = env("APPDATA");
    if let Some(cargo) = env("CARGO_HOME").or_else(|| home.as_ref().map(|p| p.join(".cargo"))) {
        for suffix in ["registry/cache", "registry/src", "git/db", "git/checkouts"] {
            roots.push(CacheRoot {
                path: cargo.join(suffix),
                ecosystem: Ecosystem::Rust,
                name: "Cargo依存キャッシュ（手修正に注意）",
            });
        }
    }
    for (base, entries) in [
        (
            &home,
            &[
                (".cache/go-build", Ecosystem::Go, "Go ビルドキャッシュ"),
                ("go/pkg/mod", Ecosystem::Go, "Go モジュールキャッシュ"),
                (".cache/pypoetry", Ecosystem::Python, "Poetry キャッシュ"),
                (
                    ".cache/huggingface",
                    Ecosystem::Python,
                    "Hugging Faceモデルキャッシュ",
                ),
                (".cache/torch", Ecosystem::Python, "PyTorchモデルキャッシュ"),
                (".cache/sccache", Ecosystem::Rust, "sccache"),
                (".cache/ccache", Ecosystem::Native, "ccache"),
                (".cache/bazel", Ecosystem::Native, "Bazel キャッシュ"),
                (".cache/composer", Ecosystem::Php, "Composer キャッシュ"),
                (
                    ".local/share/gem",
                    Ecosystem::Ruby,
                    "RubyGems ローカルパッケージ",
                ),
                (".bundle/cache", Ecosystem::Ruby, "Bundler キャッシュ"),
                (
                    ".ollama/models",
                    Ecosystem::Other,
                    "Ollama モデル（要確認）",
                ),
                (".sbt/boot", Ecosystem::Java, "sbt キャッシュ"),
                (".ivy2/cache", Ecosystem::Java, "Ivy キャッシュ"),
                (".cache/coursier", Ecosystem::Java, "Coursier キャッシュ"),
                (".pub-cache", Ecosystem::Mobile, "Dart パッケージキャッシュ"),
                (".android/avd", Ecosystem::Mobile, "Android仮想端末"),
                (
                    "Apple/MobileSync/Backup",
                    Ecosystem::Mobile,
                    "iPhone/iPadバックアップ",
                ),
                (
                    "VirtualBox VMs",
                    Ecosystem::VirtualMachine,
                    "VirtualBox 仮想マシン",
                ),
                (
                    "Documents/Virtual Machines",
                    Ecosystem::VirtualMachine,
                    "VMware 仮想マシン",
                ),
            ][..],
        ),
        (
            &local,
            &[
                ("pip/Cache", Ecosystem::Python, "pip キャッシュ"),
                ("pypoetry/Cache", Ecosystem::Python, "Poetry キャッシュ"),
                ("go-build", Ecosystem::Go, "Go ビルドキャッシュ"),
                ("wsl", Ecosystem::VirtualMachine, "WSL 管理データ"),
                (
                    "Android/Sdk/system-images",
                    Ecosystem::Mobile,
                    "Android SDK仮想端末イメージ",
                ),
                ("sccache", Ecosystem::Rust, "sccache"),
                ("NuGet/v3-cache", Ecosystem::DotNet, "NuGet HTTPキャッシュ"),
                ("CrashDumps", Ecosystem::System, "アプリクラッシュダンプ"),
                (
                    "Microsoft/Windows/WER",
                    Ecosystem::System,
                    "Windowsエラー報告",
                ),
                (
                    "Docker/wsl",
                    Ecosystem::Docker,
                    "Docker Desktop仮想ディスク",
                ),
                (
                    "containers/podman/machine",
                    Ecosystem::Podman,
                    "Podman仮想マシン",
                ),
            ][..],
        ),
        (
            &roaming,
            &[
                (
                    "Apple Computer/MobileSync/Backup",
                    Ecosystem::Mobile,
                    "iTunesバックアップ",
                ),
                ("Composer/cache", Ecosystem::Php, "Composer キャッシュ"),
            ][..],
        ),
    ] {
        if let Some(base) = base {
            roots.extend(entries.iter().map(|(suffix, ecosystem, name)| CacheRoot {
                path: base.join(suffix),
                ecosystem: *ecosystem,
                name,
            }));
        }
    }
}
