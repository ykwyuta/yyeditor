# yytoolbox録画の上下反転修正

2026-10-29、Windows実機、Rust 1.99.0。基準HEADは`3bfb7b1`、録画途中保存などを含む未コミット作業上で検証。

原因はキャプチャ用DIBの正の高さ（下端から格納）と、Media Foundation入力の正のstride（上端から格納）の不一致。DIBの高さを負にし、上端からの格納へ統一した。追加の全フレーム反転コピーは行わない。

通常試験はキャプチャ用DIBへGDIで上半分を赤・下半分を青に描画し、H.264/AACで保存したMP4を再デコードして上下を確認する。旧試験の人工配列だけでは実キャプチャDIBの向きの不一致を検出できなかった。デコード結果もstrideに従って読み、先頭アドレスを無条件に下端とみなす仮定を削除した。修正前に上端の色の不一致で失敗し、修正後に成功した。

実機試験`real_desktop_video_keeps_top_and_bottom`では上下で明暗を分けた160×120の可視ウィンドウを2秒間録画し、MP4の30フレームすべてで明るい上端・暗い下端を確認した。DPIコンテキストと元の前面ウィンドウを復元し、試験ウィンドウを破棄する。出力は`target/recording-hardware/orientation.mp4`。

```powershell
cargo +1.99.0 test -p yytoolbox --bin yytoolbox --locked native_mp4_h264_aac_encodes_and_decodes_synthetic_frames_and_audio
cargo +1.99.0 test -p yytoolbox --bin yytoolbox --locked real_desktop_video_keeps_top_and_bottom -- --ignored --nocapture --test-threads=1
```

根拠: [Microsoftのstride仕様](https://learn.microsoft.com/en-us/windows/win32/medfound/mf-mt-default-stride-attribute)。ログは`target/recording-orientation-before.log`、`target/recording-orientation-after.log`、`target/recording-orientation-hardware.log`。既に保存された動画の変換は行っていない。

追加の通常試験では、録画の符号化・音声・途中保存・キー競合の4件とコア14件が成功した一方、UI試験が外部のEsc登録との競合（`0x80070581`）で失敗した。利用者のプロセスは停止せず、画面除外・復元を確認するUI試験を、製品同様に代替キーを実際に確保する方式へ変更した。Escそのものの実入力試験とは分ける。初回結果は`target/recording-orientation-tests.log`に保持し、修正後の直列実行結果は`target/recording-orientation-tests-final.log`へ記録する。

ルート全体の書式・全target ClippyはWindows・Linuxとも成功。録画本体はWindows専用のため、Linuxの成功を録画の検証とは扱わない。

最終のWindows直列実行はコア14件・録画/UI 5件の計19件成功。実機専用5件はこの通常コマンドでは未実行で、そのうち今回の上下方向の実機試験1件を別途実行して成功した。残る実機専用試験を今回すべて実行したとは扱わない。
