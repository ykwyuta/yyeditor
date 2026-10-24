# 保存形式・プロトコルの内部仕様

利用者が編集・交換する項目や制限は同梱ヘルプで説明し、この文書は実装の所在を示します。

| 対象 | 実装・定義 | 利用者向け仕様 |
| --- | --- | --- |
| 文字コード・不正バイト保持・外部対応表 | [YYLibrary](https://github.com/ykwyuta/YYLibrary/tree/master/crates/yy-encoding/src) | [文字コードのヘルプ](../../crates/yy-win/help/help.md) |
| リモート通信 | [yy-proto](../../crates/yy-proto/src/lib.rs) | [SSH操作](../../crates/yy-win/help/help.md) |
| yysheetの保存と固定長 | [yy-sheet](../../crates/yy-sheet/src/lib.rs) / [yy-cobol](../../crates/yy-cobol/src/lib.rs) | [シートヘルプ](../../crates/yy-win/help/sheet.md) |
| yydraw文書・素材 | [yy-draw](../../crates/yy-draw/src/lib.rs) | [保存・素材パック](../../crates/yy-win/help/draw.md) |
| ERモデルYAML・計画ファイル | [yy-db-core](../../apps/yydbmanager/crates/yy-db-core/src/lib.rs) | [ER YAML・計画のヘルプ](../../apps/yydbmanager/help/help.md) |
| 比較規則・作業セッション・同期記録 | [yy-merge-core](../../apps/yymerge/crates/yy-merge-core/src/lib.rs) | [yymergeヘルプ](../../apps/yymerge/help/help.md) |
| YY3Designer文書・PNG・OBJ | [yy3-core](../../apps/yy3designer/crates/yy3-core/src/lib.rs) | [座標・保存形式](../../apps/yy3designer/help/help.md) |

形式を変更するときは版・読み込み互換・保存/再読込・不正入力・既存ファイルの保全を検証し、
利用者が手書きする仕様はヘルプにも反映します。歴史的提案に記載された形式名や版を現行仕様として転記しません。
