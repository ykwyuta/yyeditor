# YY3Designer のサンプル

`examples.yy3d`には、椅子・テーブル・子供・車・猫の5オブジェクトを収録しています。「開く」で読み込み、オブジェクトの一覧から切り替えて編集してください。各作例は6方向のドット絵、寸法、丸み、厚みの削り情報を保存しています。脚の間などの空間も保持し、厚みを削る・戻すツールで調整できます。

| 作例 | 個別プロジェクト | 3Dプレビュー |
| --- | --- | --- |
| 椅子 | [chair.yy3d](chair.yy3d) | [chair.png](chair.png) |
| テーブル | [table.yy3d](table.yy3d) | [table.png](table.png) |
| 子供 | [child.yy3d](child.yy3d) | [child.png](child.png) |
| 車 | [car.yy3d](car.yy3d) | [car.png](car.png) |
| 猫 | [cat.yy3d](cat.yy3d) | [cat.png](cat.png) |

![作例一覧（上段：椅子・テーブル・子供、下段：車・猫）](examples.png)

`robots.yy3d`をアプリで開きます。「ピクセルロボ」と「角丸ロボ」の2オブジェクトを切り替え、方向ごとのドット絵と厚み・丸みを調整できます。各オブジェクトに6方向の画像を保存しています。

`robot.png`は32×32の原画です。「写真・イラストをドット絵へ」で取り込む場合、幅32・高さ32、最近傍、減色なしを選ぶと元のドットを保持します。別の寸法や色数で縮小・減色を試すこともできます。`preview.png`は角丸ロボの3D出力例です。

サンプルは外部素材を使わず、次のプログラムで再生成します。

```powershell
cargo run --manifest-path apps/yy3designer/Cargo.toml -p yy3-core --example make_demo --locked -- demo/yy3designer
cargo run --manifest-path apps/yy3designer/Cargo.toml -p yy3-core --example scenes --locked -- demo/yy3designer
```
