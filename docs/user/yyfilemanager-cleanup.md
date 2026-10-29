# キャッシュ・大容量データの整理候補を探す

yyfilemanager の「容量の整理」タブ（Ctrl+7）、または「整理 → キャッシュ・大容量データを探す」で開きます。
依存パッケージ・キャッシュ・古いビルド成果物・未参照コンテナイメージに加え、仮想マシン・携帯バックアップ・DB・メール・動画・ダンプ等を容量順に一覧化します。大容量データは不要とは判定せず、所有アプリで整理・圧縮できる可能性を案内します。
探索は読み取りのみです。フォルダの削除やコンテナのpruneは自動実行しません。

## 探索条件

- **場所**：プロジェクトを置いているフォルダを指定します。「追加」で複数指定でき、`;`で区切ります。フォルダのドロップにも対応します。
- **環境**：従来の7環境にC/C++・ゲーム開発、Go、Ruby、PHP、携帯、仮想マシン・WSL、ブラウザー・アプリ、DB・メール、動画・音声、システム・ログ、その他大容量データを追加した18分類。
- **最小 MiB**：候補に表示する最小容量。初期値は100 MiB。0なら小さい候補も表示します。
- **ビルド経過日**：ビルド成果物の配下で最も新しい更新からの日数。初期値30日、0なら経過日を問いません。依存パッケージ・キャッシュ・イメージには適用しません。最終アクセス日時の判定ではありません。
- **共通キャッシュ・管理データ**：既定のキャッシュ、Cargo・Go等の依存、iPhone/iPadバックアップ、Android仮想端末、VirtualBox/VMware/WSL・Docker Desktop/Podmanの管理データ、クラッシュダンプ等も対象にします。初期値は有効です。
- **Docker / Podman も調査**：PATH上のCLIを使い、現在設定されている接続先を調査します。プロジェクトの探索場所とは独立です。初期値は有効です。

「候補を探す」で開始します。Escまたは「中止」で停止できます。中止時の途中結果は採用しません。
場所を指定せず、共通キャッシュやコンテナだけを調べることもできます。
環境をDocker／Podmanにした場合は「Docker / Podman も調査」も有効にしてください。

## 対応する配置

| 環境 | 主な候補 |
| --- | --- |
| Python | `__pycache__`、`.pytest_cache`、`.mypy_cache`、`.ruff_cache`、`.tox`、`.nox`、`pyvenv.cfg`を持つ`.venv`／`venv`、Pythonプロジェクトの`build`／`dist`、pip・uvキャッシュ |
| Node.js / JavaScript | `node_modules`、`.npm`、`.pnpm-store`、`.yarn-cache`、Nodeプロジェクトの`.next`・`.nuxt`・`.svelte-kit`・`.parcel-cache`・`.turbo`・`.vite`・`build`・`dist`、npm・pnpm・Yarn共通キャッシュ |
| Java | `pom.xml`があるプロジェクトの`target`、`build.gradle`／`build.gradle.kts`があるプロジェクトの`build`・`.gradle`、Mavenの`.m2/repository`、Gradleの`caches`・`wrapper/dists` |
| Rust | `Cargo.toml`があるプロジェクトの`target` |
| .NET | `csproj`／`fsproj`／`vbproj`があるプロジェクトの`bin`・`obj`、NuGetの`.nuget/packages` |
| Docker / Podman | 停止中を含む全コンテナから参照されていないイメージ。Dockerのビルドキャッシュ合計も表示 |
| 開発キャッシュの追加 | Poetry・モデルキャッシュ、Cargo registry/git、sccache・ccache・Bazel、Go modules/build、Composer、RubyGems、sbt/Ivy/Coursier、Dart/Flutter、CMake、JS解析・カバレッジ・配布成果物 |
| 仮想マシン | VHD/VHDX/AVHDX、VDI、VMDK、QCOW2、VMメモリー・スナップショット、OVA等。ゲスト内整理・バックアップ・完全停止後に所有アプリの圧縮を検討。差分・スナップショットを単体削除しない |
| 携帯 | Apple MobileSyncのバックアップ、Android AVD・SDKシステムイメージ、IPSW・AB、Smart Switch/HiSuiteの既知の管理フォルダ。端末・世代・復元の必要性を確認して所有アプリで整理 |
| DB・メール | PST/OST/MBOX、SQLite/DB、MDB/ACCDB、MDF/LDF/IBD等。アプリで不要データ整理後に圧縮・VACUUM・ログ保持期間を検討。稼働中のDBやWALは直接削除しない |
| バックアップ・メディア | BAK/BACKUP/TIBX/MRIMG/VBK、ISO/WIM/ESD/DMG、圧縮アーカイブ、インストーラー、動画・音声・制作データ。旧世代の確認、外部媒体への移動、別ファイルへの再書き出しを検討 |
| 診断・その他 | ログ、DMP/HPROF/ETL/EVTX、Windows管理ファイル。未知の形式も1 GiB以上なら表示（最小容量条件も適用）。システム管理ファイルはWindows設定から整理 |
| データセット・AIモデル | CSV/TSV/JSON/Parquet/Arrow/HDF5等とGGUF/Safetensors/ONNX/PT等は100 MiB以上なら表示（最小容量条件も適用）。学習済みの独自モデルや原データを保持すべきか確認し、所有アプリで世代整理・圧縮・外部媒体への移動を検討 |

一般的な`build`・`dist`・`target`・`bin`・`obj`は、対応するプロジェクト定義を同じ親フォルダで確認してから候補にします。
カスタム出力先・共有ワークスペース等のすべてを自動判定するものではありません。
Python仮想環境や依存関係には手作業の変更・ローカルだけのパッケージが含まれる場合があります。

共通キャッシュでは`USERPROFILE`／`LOCALAPPDATA`／`APPDATA`の既定配置に加え、`PIP_CACHE_DIR`・`npm_config_cache`／`NPM_CONFIG_CACHE`・`GRADLE_USER_HOME`を参照します。
存在しない既定配置は無視し、カスタム設定ファイルや各ツールの設定コマンドによる追加探索はしません。

## 候補の確認と整理

- **詳細**または行のダブルクリック：場所・ID、理由、再生成方法や整理コマンドを表示します。
- **フォルダで確認**：候補フォルダ、または候補ファイルの保存先フォルダを開きます。大容量ファイルを自動で実行・マウントしません。
- **整理方法をコピー**：選択行の説明やコマンドをコピーします。コマンドは実行しません。
- **CSV出力**：現在の候補全件をUTF-8 BOM付きCSVとして保存します。数値のバイト数も出力します。

一覧の容量は配下のファイルの**論理サイズ**です。ハードリンク・圧縮・疎ファイル等により、実際のディスク占有量や解放量とは異なります。
同じ候補の入れ子と重なる探索場所は重複集計しません。リンク・ジャンクションをたどらず、Git等の管理領域・隔離フォルダ・コンテナ内部ストレージの通常の再帰探索を除外します。共通の管理データを有効にしたときだけ、既知のDocker/Podman VMフォルダ全体を読み取り集計します。個々の内部レイヤーを削除候補とは判定しません。
一部を読み取れない候補には「一部未集計」と`≥`を表示し、記録タブに理由を残します。警告の詳細は最大200件です。

コンテナ情報の取得は`image ls`・`container ls`・`container inspect`・`system df`だけを使います。
CLI未導入・エンジン停止・接続エラーは記録に残し、通常のフォルダ探索は続けます。コンテナの参照状況が不明な場合、イメージ候補を未使用と推定しません。
1コマンドは最大60秒、標準出力・標準エラーは各8 MiBまでです。

イメージはIDごとにまとめますが、イメージ間で共有するレイヤーの容量は差し引きません。
コンテナを削除しても後で再利用したいイメージは残してください。整理コマンドは**探索時と同じ接続先**で内容を確認して実行してください。
ボリューム・稼働中コンテナ・VMディスクの直接削除や、VMディスクの自動縮小は行いません。

縮小方法は提案で、回収可能量やデータの不要性を実測・保証するものではありません。[動的拡張VHDの圧縮条件](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/compact-vdisk)、[Appleのバックアップ管理](https://support.apple.com/en-gb/108809)も参照してください。

## 署名のないアプリを開く

「フォルダ」タブやアーカイブから展開して開くEXE/COMは、署名を検証できない場合に実行ファイルのパスと確認結果を表示します。「はい」でそのファイルの実行を承認した場合だけ起動します。初期選択は「いいえ」です。署名確認から実行開始までファイルの書き換え・削除を防ぎ、起動失敗も表示します。管理者権限が必要なアプリはWindowsのUAC確認へ進みます。Windowsのアプリ制御・管理ポリシー自体は解除しません。

CLI仕様：[Docker image ls](https://docs.docker.com/reference/cli/docker/image/ls/)、[Docker system df](https://docs.docker.com/reference/cli/docker/system/df/)、[Podman images](https://docs.podman.io/en/latest/markdown/podman-images.1.html)、[Podman system df](https://docs.podman.io/en/latest/markdown/podman-system-df.1.html)。
