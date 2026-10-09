# 20. yybrowser の広告ブロック

タブブラウザ yybrowser（19 章）に、**広告・追跡のブロック** を組み込みます（2026-10-09 提案。実装の状況は 9 節）。
拡張機能は使わず、広く使われているフィルタリスト（Adblock Plus 形式）を yybrowser が自分でダウンロードし、
WebView2 が出す要求を照らして止めます。プロキシと同じく、**このブラウザの中だけ** で効き、OS やほかのブラウザ
には影響しません。

19 章 1 節で対象外としていた「広告の除去」を、この章で対象にします。

## 1. 要求

| # | 要求 | 対応 |
|---|------|------|
| A1 | 広告・追跡の要求（画像・スクリプト・iframe・XHR など）を止める | 4 |
| A2 | 止めた後に残る広告の枠を隠す（要素の非表示） | 5 |
| A3 | 有名なフィルタリストを選べる。既定は EasyList・EasyPrivacy・AdGuard 日本語フィルタ | 3 |
| A4 | フィルタリストは自動で更新する。好きな URL・ローカルのファイルも足せる（社内のミラー・インターネットに出られない環境） | 3 |
| A5 | プロファイルごとに入・切できる。サイトごとに「このサイトでは止めない」を選べる | 6 |
| A6 | 止めた件数をツールバーに出す | 6 |
| A7 | フィルタは同梱しない（利用者の PC がダウンロードする） | 3.3 |

対象外: 動画の本編と同じ通信で流れる広告（止められない）、uBlock Origin 独自のスクリプトの差し込み
（scriptlet。`##+js(...)`）、手続き的な非表示（`:has-text()` など）、要素を選んで隠す画面、フィルタの書き方の検査。

## 2. 全体構成

```
yy-win browser/
  mod.rs      ツールバーの「🛡 件数」ボタン、タブごとの件数、イベントの登録
  adblock.rs  要求の照合（WebResourceRequested）、要素の非表示（DOMContentLoaded・WebMessageReceived）、
              フィルタのダウンロード（WinINet。プロファイルの経路で）と、エンジンの作り直し（別のスレッド）
  filterdlg.rs フィルタリストの一覧の編集（入・切・追加・削除・今すぐ更新・状態）
yy-adblock（新規・OS に依存しない。Linux で試験する）
  lists    既定のフィルタリスト、キャッシュ（本文と更新の記録）、更新の間隔（`! Expires:`）
  engine   adblock クレート（Brave の広告ブロックの中核。MPL-2.0）の包み: 要求の照合・非表示の CSS
yy-browser
  profiles フィルタリストの一覧（browser.toml の [adblock]）、プロファイルの入・切と止めないサイト
  rules    ダウンロードの経路（プロファイルの規則・やり方から、直接・プロキシ・OS と同じ）
```

## 3. フィルタリスト

### 3.1 既定の一覧

| 名前 | URL | 既定 |
|---|---|---|
| EasyList | `https://easylist.to/easylist/easylist.txt` | 有効 |
| EasyPrivacy | `https://easylist.to/easylist/easyprivacy.txt` | 有効 |
| AdGuard 日本語フィルタ | `https://filters.adtidy.org/extension/ublock/filters/7.txt` | 有効 |
| uBlock filters | `https://ublockorigin.github.io/uAssets/filters/filters.txt` | 無効 |
| EasyList Cookie List | `https://secure.fanboy.co.nz/fanboy-cookiemonster.txt` | 無効 |

- 一覧は `browser.toml` の `[adblock]` に保存する（全プロファイルで共通）。利用者は入・切・追加・削除できる。
- URL は `http://`・`https://`、またはローカルのファイルのパス（`C:\filters\my.txt`・`file:///…`）。ローカルの
  ファイルは毎回読む（ダウンロードしない）。自作の規則（`@@||intra.example.jp^` で止めない、など）にも使う。
- 280blocker など、利用条件で組み込みを禁じているリストは既定に入れない。

### 3.2 更新

- キャッシュは `%LOCALAPPDATA%\yyeditor\yybrowser\filters\` に、URL ごとに本文（`<印>.txt`）と記録
  （`<印>.meta`: 取得した時刻・次に更新する時刻・規則の数・最後の失敗）を置く。
- 更新の間隔は、リストの先頭の `! Expires: 4 days` に従う（1 時間〜14 日に収める。なければ 4 日）。
- 起動するとまずキャッシュからエンジンを作り（すぐ効く）、期限の切れたリストを別のスレッドでダウンロード
  してから作り直す。「今すぐ更新」で期限に関係なく取り直す。
- ダウンロードしたものは、HTML でないこと・規則らしい行があることを確かめ、だめならキャッシュを残して失敗を
  記録する（取り違え・ログインの画面などで規則を失わない）。上限は 1 つ 30 MB。
- ダウンロードは、**今のプロファイルの経路** で行う（WinINet）。プロファイルの規則（19 章 3.5）がリストの
  ホストに当てはまればそのプロキシか直接、なければやり方（直接・指定・OS と同じ）。SOCKS5 は WinINet が扱え
  ないので、OS と同じ経路にする（状態に出す）。

### 3.3 ライセンス

- リストは同梱せず、利用者の PC がダウンロードする。再配布にあたらないので、リストのライセンス（GPLv3・
  CC BY-SA）は yybrowser の配布に影響しない。
- adblock クレートは MPL-2.0（ファイル単位のコピーレフト。変更せずに使う）。

## 4. 要求を止める

- タブの WebView2 に `AddWebResourceRequestedFilterWithRequestSourceKinds("*", ALL, ALL)`（`ICoreWebView2_22`。
  iframe・Service Worker の要求も含める。古いランタイムでは `AddWebResourceRequestedFilter`）を登録し、
  `WebResourceRequested` で照らす。広告ブロックが切りのプロファイルでは登録しない（要求ごとの往復をしない）。
- 照合: 要求の URL・ページの URL（第三者かどうかの判断）・種類（`ResourceContext` → image・script・stylesheet・
  font・media・xhr・fetch → xmlhttprequest・websocket・ping・sub_frame・other）。トップのページの読み込み
  （`NavigationStarting` の URL と同じ document）は止めない。
- 止めるときは、本文なしの 403 の応答（`CreateWebResourceResponse`）を返す。
- 照合は数マイクロ秒（EasyList ほか 3 つで 1 要求 10〜70 µs を確かめた）。エンジンは 3 つで約 27 MB・作るのに
  約 0.1 秒（リリースのビルド）。
- エンジンは別のスレッドで作り、できたら UI のスレッドに渡す（`Send`。adblock の `single-thread` の機能は外す）。

## 5. 広告の枠を隠す

- `DOMContentLoaded`（`ICoreWebView2_2`）で、そのページのホスト向けの非表示の規則（`example.jp##.ad`）を CSS
  （`display: none !important`）にして差し込む。規則 1 つを 1 つの CSS の規則にする（書けない規則が 1 つあっても、
  ほかが効くように）。
- 汎用の規則（`##.ad-banner` のようにホストを限らないもの）は数が多いので、ページにある class・id を集める
  小さなスクリプトを差し込み、`chrome.webview.postMessage` で送らせて、当てはまる規則だけを CSS にして返す
  （uBlock Origin と同じやり方）。後から足された要素は `MutationObserver` で、新しい class・id だけを間引いて送る。
- メッセージは `yyab` で始まる素のテキスト（class・id を空白で区切る）にする。ページのスクリプトが偽の
  メッセージを送っても、できるのは「フィルタの規則にある要素を隠す」ことだけで、害はない。
- `generichide`・例外（`#@#`）は adblock クレートの判断に従う。

## 6. 画面

- ツールバーのアドレスバーの右に **「🛡 件数」** のボタン（今のタブで止めた数。切りなら「🛡 切」）。押すと:
  - 「広告ブロック（このプロファイル）」の入・切
  - 「このサイトでは止めない（ホスト名）」の入・切（プロファイルの `adblock_allow` に保存）
  - 「フィルタを今すぐ更新」
  - 「フィルタリスト...」（一覧の画面: チェックで入・切、追加・削除、状態の列に規則の数・更新した日時・失敗）
- プロファイル: `adblock_off`（切）と `adblock_allow`（止めないサイト。ドメインとサブドメイン）。既定は入。
- 状態表示: 止めたときに「広告ブロック: n 件（EasyList ほか）」、更新の結果。

## 7. 試験

- yy-adblock の単体試験（Linux）: 既定の一覧、`! Expires:` の読み取り、期限の判断、キャッシュの読み書き、
  ダウンロードしたものの確かめ、照合（第三者・種類・例外）、非表示の CSS（ホスト向け・汎用・例外）。
- yy-browser: ダウンロードの経路（規則・やり方）、設定の保存と読み込み。
- Windows の CI（本物の WebView2）: 試験の中で HTTP サーバーを立て、`/ads/banner.png` を読み込むページと
  `.ad-box` の要素を出し、規則 `/ads/banner.png` `##.ad-box` のエンジンで、サーバーに `/ads/` の要求が届かない
  こと・`.ad-box` が `display: none` になること・普通の画像は届くことを確かめる。

## 8. 実装の段階

| 段階 | 内容 |
|---|---|
| A1 | yy-adblock（一覧・キャッシュ・更新の間隔・エンジンの包み）と設定 |
| A2 | 要求を止める・件数・入切・止めないサイト |
| A3 | 要素の非表示（ホスト向け・汎用・MutationObserver） |
| A4 | ダウンロード（経路・確かめ・キャッシュ）とフィルタリストの画面 |

## 9. 実装の状況（2026-10-09）

A1〜A4 を実装しました（中核は `crates/yy-adblock`、設定は `crates/yy-browser`（`profiles`・`rules::download_route`）、
画面は `crates/yy-win/src/browser/adblock.rs`・`filterdlg.rs`、利用ガイドは `crates/yy-win/help/browser.md`）。

- adblock 0.13（`single-thread` を外す）。EasyList・EasyPrivacy・AdGuard 日本語フィルタで、エンジンを作るのに約 0.1 秒・
  約 27 MB、1 要求の照合は 10〜70 µs（Linux のリリースのビルドで計った）。
- 要求の照合は `ICoreWebView2_22` があれば iframe・Service Worker の要求も受ける。止めた数はタブごとに数え、🛡 の
  ボタンに出す。広告ブロックを切にしたプロファイルではフィルタを登録しない（入・切のときに付け外す）。
- 非表示: ホスト向けは DOMContentLoaded で、汎用は class・id のメッセージの往復で差し込む。
- 試験: yy-adblock・yy-browser の単体試験（Linux）。Windows の CI で、本物の WebView2 で `/ads/banner.png` の要求が
  サーバーに届かないこと、ホスト向け（`127.0.0.1##.side-ad`）と汎用（`##.ad-box`）の規則で要素が `display: none`
  になること、普通の画像と要素はそのままであることを確かめる。

まだのもの: 止めた要求の一覧の画面、規則の書き方の検査、`redirect`（代わりの空のスクリプトを返す）。
