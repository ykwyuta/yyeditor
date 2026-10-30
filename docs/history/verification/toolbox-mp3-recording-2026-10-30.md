# yytoolbox Pure Rust MP3録音の検証（2026-10-30）

## 方式

音声のみの保存をWAVからMP3へ変更。「保存する内容」は「音声のみ（MP3）」、拡張子は`.mp3`。
入力は従来どおりWASAPIの既定PC再生音・マイク。48kHz・ステレオ、VBR品質2で保存する。
独自の録音ゲインや自動補正は追加していない。単一音源はそのまま、両音源は各50%の平均。

`rusty_mp3 = 1.0.5`を完全なバージョンに固定し、root Cargo.lockも更新した。
Apache-2.0、通常依存・ビルド依存なし、C/C++・FFI・build.rsを使わないエンコーダーであることを公開パッケージのソースで確認。
一次資料：[パッケージ](https://docs.rs/crate/rusty_mp3/1.0.5)、[ソースリポジトリ](https://github.com/Remade-With-Rust/remade_ffmpeg_rs)。

アプリ側にプラットフォーム非依存の `yytoolbox::mp3::Encoder` を追加した。
16bitステレオPCMの入力検証、フレームごとのパケット回収、ファイルへの逐次出力、終了時のXing/LAMEタグ更新、I/Oエラー伝播を扱う。
ライブラリのCBRはビットリザーバー用フレームを終了まで保持するため、長時間収録では使わず、逐次出力するVBRを選んだ。
一度に渡すPCMを1152フレーム以下に区切り、呼び出し元が大きいバッファを渡してもMP3パケットキューに全件を溜めない。
一時ファイル確定後の置換、開始前の中止・デバイス/書き込みエラーでの既存ファイル保持は継続する。

## 自動試験

- エンコード途中からMP3パケットが書き出されることを確認。
- 異なる周波数・振幅の左右チャンネルを往復し、各チャンネルのSNRが18dBを上回ることを確認。
- Xingのフレーム数・ファイルサイズ、LAMEの遅延・末尾パディングから元の48000フレームを復元できることを確認。
- 1/31/1151/1152/1153フレームの極短音声、無音、最大・最小振幅、部分PCMの拒否、ディスク書き込みエラー、先頭プレフィックスを壊さないタグ更新を確認。
- Windows標準Media FoundationデコーダーでMP3録音をPCMへ変換し、テスト音・時間・音声のみのストリームを確認。自己実装デコーダーの往復だけでは互換性の根拠にしない。
- MP3途中保存・開始前中止・エラー時の既存ファイル保持と一時ファイル回収、拡張子・音源条件の検証を実施。
- MP3はフレーム単位の遅延・余白があり、Xing/LAMEによる補正の扱いはプレイヤーに依存する。

## 実機

`recording::hardware_tests::real_audio_only_system_mp3` を明示実行し成功。
実際にPCで再生したテスト音を2秒録音し、Windowsの独立デコーダーで7サンプルバッファ、最終時刻2.016秒、RMS 1396.69を確認した。
専用一時ディレクトリの音声ファイルは試験終了時に削除した。

この環境の既定マイクは初期WAV検証で `0x80070490`（要素が見つからない）。マイク単独・混合のMP3実機検証は未完了であり、成功とは扱わない。
合成PCMの混合演算・符号とオーバーフロー防止はWindows自動試験で確認した。

音量の説明の根拠：[WASAPI Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)、[IAudioEndpointVolume](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nn-endpointvolume-iaudioendpointvolume)。
出力音量やミュートを変更した実機測定は行っておらず、機器非依存の挙動とは断定しない。

## 最終検証結果

- Windows Rust 1.99.0：ルートの `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings` 成功。
- `cargo test -p yytoolbox --locked -- --test-threads=1` 成功（lib 19件、Windows 11件、結合7件、計37件）。実機専用9件は既定ではignoredであり、上記PC音声試験1件のみを別途明示実行した。
- 全ツールの画面サイズ・ズーム・文字測定、録音モード切替時の設定欄・ボタン、録画/録音バー両方のDPI別文字測定・停止通知が成功。
- `cargo build -p yytoolbox --locked` 成功。動作確認用は `target/debug/yytoolbox.exe`。
- Linux Rust stable 1.99.1：ルート書式・全ターゲットClippy、yytoolboxの試験（lib 19件・結合7件）成功。
- `cargo tree -p rusty_mp3 --edges normal,build` は当該パッケージ1個のみ。依存追加はルートのyytoolboxとCargo.lockだけで、独立アプリのワークスペースは変更していない。
- MSRV検査で検出した1.87以降の `is_multiple_of` 呼び出しは、1.85で使用可能な剰余比較へ修正し、Windows/Linux両方のClippyを再実行して成功。
- ログ：`target/toolbox-mp3-{codec-tests,record-tests,real,all-tests,clippy,linux,build}.log`。

この記録は公開前のローカル検証であり、CI実行・リリース公開の結果ではない。
