# yytoolboxの構成と検証

ルートのCargoワークスペースにある独立したWindows GUIです。yy-win、WebView2、外部CLIを必要としません。
利用説明は[yytoolbox](../user/yytoolbox.md)と[同梱ヘルプ](../../apps/yytoolbox/help/help.md)を参照してください。

## 境界

- `lib.rs` がツールの項目・入力例・上限・処理を管理します。UIの結果は非同期ワーカーから受け取り、入力変更時に失効させます。
- `ui_appearance.rs` はネイティブ描画・フォント計測・配置を管理します。モニターDPIとアプリ倍率を掛け合わせ、条件はラベルを上に配置した1〜2列、入出力は幅に応じて縦横を切り替えます。説明・ステータスは実フォントで高さを測定し、画面全体のスクロール範囲とフォーカス追従を更新します。初期ウィンドウはモニターの作業領域に収めます。
- `webtools.rs` はQR、URL、HTML/CSS/JavaScript整形。QRはqrcode、整形はmarkup_fmt/Malva/Biome。JavaScriptはPure Rustのパーサーとフォーマッターを組み込み、C・アセンブラの補助ライブラリやNode.jsを必要としません。スクリプトは実行せず、文字列置換だけで整形しません。
- `formats.rs` は文字列キーの型付き共通ツリーを使います。「構造保持」ではXML/CSV/INIに専用のノード表現を出力し、YAMLとの全組合せで値を往復できます。標準形式で表現できない構造は拒否します。
- `images.rs` は明示的に渡されたファイルだけを読み、一時ファイルから `persist_noclobber` で保存します。名前の衝突は番号で解消し、元と既存出力を上書きしません。デコード・画素数・件数を制限します。
- `recording.rs` はWindows専用。GDIの矩形をRGB32へ取得し、Media FoundationのSink WriterでH.264/AAC MP4を生成します。ffmpeg等の子プロセスは起動しません。
- 音声はWASAPI shared-modeで既定のPC再生音（loopback）とマイクを48 kHz/stereo/16-bit PCMとして取得します。キューは上限付き。選択した入力を平均してミックスし、無音部分を補い、デバイス取得・許可の失敗を通知します。
- 録画時にはyytoolboxの可視トップレベル画面の状態を保存し、キャプチャ除外属性を設定して非表示にします。DWMの反映を待ってから録画し、Escのグローバルホットキーで中止できます。終了・エラーでは画面と属性を戻し、ホットキー・COM・音声スレッド・GDI・MF資源を解放します。
- MP4は保存先フォルダの一時ファイルへ書き、Finalizeと解放の後で置換します。中止時は一時ファイルを破棄します。録画情報を表示するテキスト結果とMP4本体の保存先は区別します。

## 検証

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p yytoolbox --locked
cargo build -p yytoolbox --locked
```

コア試験はOS非依存です。WindowsのUI試験は全22ツール、75/100/125/150/200%のアプリ倍率、480/640/1100論理px幅、96/120/144/192 DPIの組合せで、実GDIフォントと子コントロールを使用して配置を検査します。DPIはテスト専用の入力で再現するため、実際のモニター間移動の検証とは区別します。ラベル・初期値・全選択肢の文字幅、横方向の収まり、全要素のスクロール到達性、フォーカス追従を確認します。QR・YAML CSV・一括画像処理の非同期結果は期限付きで待ちます。
録画試験は実デスクトップ・実マイクを収録せず、合成した映像と音声をH.264/AACへ符号化し、Media Foundation Source Readerでフレーム数・上下方向・長さ・音声の非無音を確認します。
Source ReaderのRGB32変換には `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING` を使用します。
Windows Server CIでは必要ならServer-Media-Foundationを導入し、DLLと実コーデックの動作を試験します。必要なランタイムがない試験を成功扱いでスキップしません。
実機試験は明示的に実行します。実デスクトップを短時間収録し、PC再生音試験では小さい確認音を再生、マイク試験では既定の入力デバイスを収録します。システム音量・既定デバイス設定は変更しません。出力は `target/recording-hardware` に保存されます。実画面・実音声を含むため、必要に応じて確認後に削除してください。

```powershell
cargo test -p yytoolbox --bin yytoolbox --locked recording::hardware_tests -- --ignored --test-threads=1 --nocapture
```

全画面と指定矩形のMP4をデコードし、実WASAPIパケット、AACの音量、キャプチャからの自ウィンドウ消失・復元、実Esc入力のグローバル通知と録画中止・既存出力の保持を確認します。マイクがない環境では試験は失敗し、成功としてスキップしません。矩形選択のマウス操作、保護されたコンテンツ、複数デバイスの組合せは追加確認が必要です。実行環境によってグローバルホットキーの権限も必要です。

参考: [Microsoft Source Reader](https://learn.microsoft.com/en-us/windows/win32/medfound/processing-media-data-with-the-source-reader)、[WASAPI loopback](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)。
