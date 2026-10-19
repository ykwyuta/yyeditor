"""Generate yydraw's original, offline business/IT vector catalog and inventory."""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/yy-draw/assets/business-icons.json"
INVENTORY = ROOT / "docs/yydraw-business-icons.md"
INK = "#29445f"
ACCENT = "#247ca8"
PALE = "#e4f2fa"


def path(d, fill="none", stroke=INK, width=2):
    return f'<path d="{d}" fill="{fill}" stroke="{stroke}" stroke-width="{width}"/>'


def rect(x, y, w, h, fill=None, radius=3):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill or PALE}" stroke="{INK}" stroke-width="2"/>'


def circle(x, y, r, fill=None):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill or PALE}" stroke="{INK}" stroke-width="2"/>'


def dot(x, y, r=2):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{ACCENT}"/>'


def arrow(x, y, dx, dy):
    ex, ey = x + dx, y + dy
    length = (dx * dx + dy * dy) ** 0.5
    ux, uy = dx / length, dy / length
    return path(f'M{x} {y} L{ex} {ey} M{ex-ux*5-uy*4} {ey-uy*5+ux*4} L{ex} {ey} L{ex-ux*5+uy*4} {ey-uy*5-ux*4}', stroke=ACCENT)


def person(x, y, scale=1):
    return f'<g transform="translate({x} {y}) scale({scale})">' + circle(0, -7, 5, "#fff") + path('M-10 13 V8 C-10-3 10-3 10 8 V13 Z', PALE) + '</g>'


def server(x=16, y=10, w=32, h=12):
    return rect(x, y, w, h) + dot(x + 5, y + h / 2) + path(f'M{x+w-13} {y+h/2} h7')


def database(x=14, y=12, w=36, h=38):
    ry = 6
    return path(f'M{x} {y+ry} V{y+h-ry} C{x} {y+h+2} {x+w} {y+h+2} {x+w} {y+h-ry} V{y+ry}', PALE) + f'<ellipse cx="{x+w/2}" cy="{y+ry}" rx="{w/2}" ry="{ry}" fill="{PALE}" stroke="{INK}" stroke-width="2"/>' + path(f'M{x} {y+h/2} C{x} {y+h/2+8} {x+w} {y+h/2+8} {x+w} {y+h/2}')


def document():
    return path('M16 6 H39 L50 17 V58 H16 Z', PALE) + path('M39 6 V17 H50 M23 26 H42 M23 33 H42 M23 40 H35')


def shield():
    return path('M32 6 L53 14 V30 C53 43 44 51 32 58 C20 51 11 43 11 30 V14 Z', PALE)


def monitor():
    return rect(7, 9, 50, 34) + path('M26 43 V53 H38 V43 M18 54 H46 M8 36 H56')


def building():
    return rect(12, 8, 40, 49) + ''.join(rect(x, y, 6, 6, "#fff", 0) for y in (16, 29) for x in (20, 37)) + rect(27, 44, 10, 13, "#fff", 0)


def check(x=24, y=34):
    return path(f'M{x-5} {y} l5 5 11-12', stroke=ACCENT, width=3)


def gear(x=32, y=32, r=12):
    return circle(x, y, r) + circle(x, y, 4, "#fff") + path(f'M{x} {y-r-6} v6 M{x} {y+r} v6 M{x-r-6} {y} h6 M{x+r} {y} h6 M{x-r-4} {y-r-4} l5 5 M{x+r-1} {y+r-1} l5 5 M{x+r+4} {y-r-4} l-5 5 M{x-r+1} {y+r-1} l-5 5', stroke=ACCENT, width=3)


def chart_axes():
    return path('M10 9 V54 H57')


def make_catalog():
    entries = []
    categories = []

    def category(name, accent, pale, items):
        global ACCENT, PALE
        ACCENT, PALE = accent, pale
        rows = []
        for slug, name_ja, keywords, draw in items:
            svg = '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><g stroke-linecap="round" stroke-linejoin="round">' + draw() + '</g></svg>'
            entries.append(dict(vendor="業務・IT", category=name, name=name_ja,
                                keywords=keywords, source="yydraw:business-icons/v1",
                                file=f"business/{slug}.svg", svg=svg))
            rows.append((name_ja, keywords, slug))
        categories.append((name, rows))

    category("01 コンピューティング", "#247ca8", "#e4f2fa", [
        ("server", "サーバー", "server サーバ", lambda: server(12, 8, 40, 14)+server(12, 25, 40, 14)+server(12, 42, 40, 14)),
        ("rack", "ラック", "rack datacenter データセンター", lambda: rect(13, 5, 38, 52, "#fff")+server(18, 11, 28, 10)+server(18, 25, 28, 10)+server(18, 39, 28, 10)+path('M18 57 V60 M46 57 V60')),
        ("desktop", "デスクトップPC", "desktop workstation client 端末", lambda: monitor()+rect(17, 57, 30, 4, "#fff", 1)),
        ("laptop", "ノートPC", "laptop notebook mobile パソコン", lambda: rect(12, 10, 40, 30)+rect(17, 15, 30, 20, "#fff", 1)+path('M12 40 L5 52 Q5 56 10 56 H54 Q59 56 59 52 L52 40 Z', PALE)+path('M26 50 H38')),
        ("virtual-machine", "仮想マシン", "virtual machine vm 仮想化", lambda: rect(8, 8, 35, 35, "#fff")+rect(21, 21, 35, 35)+path('M28 33 H48 M28 40 H48 M28 47 H39')+path('M9 49 V56 H16', stroke=ACCENT)),
        ("container", "コンテナ", "container docker pod kubernetes k8s", lambda: path('M8 17 L32 6 L56 17 V47 L32 58 L8 47 Z', PALE)+path('M8 17 L32 28 L56 17 M32 28 V58 M20 12 L44 23 M20 23 V51 M44 23 V51')),
        ("application", "アプリケーション", "application app software system ソフトウェア", lambda: rect(7, 9, 50, 46)+path('M7 20 H57')+dot(13, 15)+dot(20, 15)+path('M26 29 L19 36 L26 43 M38 29 L45 36 L38 43 M34 27 L30 45', stroke=ACCENT)),
        ("batch", "バッチ処理", "batch job bulk 一括処理", lambda: rect(9, 7, 34, 38, "#fff")+rect(16, 14, 34, 38)+path('M23 25 H43 M23 32 H37')+arrow(23, 42, 19, 0)+circle(48, 48, 10, "#fff")+path('M48 42 V48 L52 51', stroke=ACCENT)),
        ("mainframe", "メインフレーム", "mainframe host legacy 汎用機 ホスト", lambda: rect(6, 9, 24, 47)+rect(34, 9, 24, 47)+path('M11 19 H25 M11 26 H25 M11 33 H25 M39 19 H53 M39 26 H53 M39 33 H53')+circle(18, 45, 4, "#fff")+circle(46, 45, 4, "#fff")),
        ("edge-device", "エッジ機器", "edge iot device sensor センサー", lambda: rect(16, 19, 32, 32)+rect(23, 26, 18, 18, "#fff")+path('M24 13 V19 M32 13 V19 M40 13 V19 M24 51 V57 M32 51 V57 M40 51 V57 M10 27 H16 M10 35 H16 M10 43 H16 M48 27 H54 M48 35 H54 M48 43 H54')+dot(32, 35, 4)),
    ])
    category("02 ネットワーク", "#207f84", "#e0f3f0", [
        ("router", "ルーター", "router routing ルータ", lambda: rect(8, 19, 48, 27)+arrow(19, 27, 26, 0)+arrow(45, 38, -26, 0)+path('M17 46 V55 M47 46 V55 M12 55 H52')),
        ("switch", "スイッチ", "switch switching hub ハブ", lambda: rect(6, 18, 52, 30)+''.join(rect(x, 28, 7, 9, "#fff", 0) for x in (12, 23, 34, 45))+path('M13 12 V18 M51 12 V18')+dot(13, 42)+dot(23, 42)),
        ("firewall", "ファイアウォール", "firewall fw 境界防御", lambda: rect(9, 11, 46, 43)+path('M9 25 H55 M9 40 H55 M24 11 V25 M41 11 V25 M17 25 V40 M34 25 V40 M49 25 V40 M24 40 V54 M41 40 V54')+arrow(2, 32, 8, 0)),
        ("load-balancer", "ロードバランサー", "load balancer lb 負荷分散", lambda: rect(5, 25, 14, 14)+path('M19 32 H31 M31 13 V51')+arrow(31, 13, 13, 0)+arrow(31, 32, 13, 0)+arrow(31, 51, 13, 0)+rect(46, 6, 12, 14)+rect(46, 25, 12, 14)+rect(46, 44, 12, 14)),
        ("vpn", "VPN", "vpn tunnel encrypted 専用接続 トンネル", lambda: rect(5, 22, 15, 22)+rect(44, 22, 15, 22)+path('M20 27 H44 M20 39 H44', stroke=ACCENT)+path('M27 27 V21 C27 14 37 14 37 21 V27')+rect(24, 27, 16, 16, "#fff")+dot(32, 34)),
        ("gateway", "ゲートウェイ", "gateway gw proxy プロキシ", lambda: rect(21, 7, 22, 50)+path('M27 15 H37 V49 H27 Z', "#fff")+arrow(3, 25, 25, 0)+arrow(36, 40, 25, 0)),
        ("internet", "インターネット", "internet wan globe 外部ネットワーク", lambda: circle(32, 32, 25)+path('M7 32 H57 M12 18 H52 M12 46 H52 M32 7 C16 17 16 47 32 57 C48 47 48 17 32 7 M32 7 V57')),
        ("wifi", "無線LAN", "wifi wireless wlan access point AP", lambda: path('M6 20 Q32-2 58 20 M13 29 Q32 12 51 29 M21 38 Q32 28 43 38', stroke=ACCENT, width=4)+dot(32, 48, 5)),
        ("dns", "DNS", "dns domain name resolver 名前解決", lambda: circle(20, 21, 13)+path('M7 21 H33 M20 8 C11 15 11 27 20 34 C29 27 29 15 20 8')+rect(31, 35, 27, 19)+arrow(22, 42, 9, 0)+path('M38 42 H51 M38 48 H46')),
        ("subnet", "サブネット", "subnet vlan segment lan セグメント", lambda: rect(5, 5, 54, 54, "#fff")+rect(11, 12, 16, 15)+rect(37, 12, 16, 15)+rect(24, 40, 16, 12)+path('M19 27 V33 H45 V27 M32 33 V40')),
    ])
    category("03 データ・連携", "#6457a3", "#eeeafa", [
        ("database", "データベース", "database db sql rdb データ管理", lambda: database()),
        ("data-warehouse", "データウェアハウス", "data warehouse dwh bi 集約", lambda: path('M5 24 L32 7 L59 24 V56 H5 Z', PALE)+path('M5 24 H59')+rect(14, 32, 14, 16, "#fff", 1)+rect(36, 32, 14, 16, "#fff", 1)+path('M14 40 H28 M36 40 H50')),
        ("data-lake", "データレイク", "data lake raw unstructured 非構造化", lambda: path('M7 23 L18 9 L29 19 L41 8 L57 23 V47 Q32 61 7 47 Z', PALE)+path('M7 31 Q20 23 32 31 T57 31 M7 41 Q20 33 32 41 T57 41 M17 49 Q32 43 47 49', stroke=ACCENT)),
        ("file-storage", "ファイルストレージ", "file storage nas folder 共有フォルダー", lambda: path('M7 15 H26 L32 22 H57 V52 H7 Z', PALE)+path('M7 27 H57')+rect(17, 33, 20, 13, "#fff", 1)+path('M23 38 H31 M23 42 H29')),
        ("backup", "バックアップ", "backup restore recovery 復元", lambda: database(7, 12, 30, 37)+path('M37 21 C55 16 64 39 49 49', stroke=ACCENT, width=3)+path('M49 40 V49 H58', stroke=ACCENT, width=3)+dot(19, 42)),
        ("object-storage", "オブジェクトストレージ", "object blob bucket オブジェクト", lambda: path('M10 14 H54 L49 55 H15 Z', PALE)+path('M10 14 Q32 4 54 14 Q32 24 10 14')+path('M25 29 L33 25 L41 29 V39 L33 43 L25 39 Z', "#fff")+path('M25 29 L33 33 L41 29 M33 33 V43')),
        ("event-stream", "イベントストリーム", "event stream queue message kafka メッセージ キュー", lambda: arrow(4, 15, 55, 0)+arrow(4, 49, 55, 0)+rect(8, 25, 10, 14)+rect(27, 25, 10, 14)+rect(46, 25, 10, 14)+path('M18 32 H27 M37 32 H46')),
        ("etl", "ETL・データ変換", "etl elt integration transform データ連携 変換", lambda: database(5, 19, 17, 30)+path('M24 12 H48 L39 28 V38 L32 43 V28 Z', PALE)+arrow(41, 45, 15, 0)+rect(49, 51, 10, 7, "#fff", 1)),
        ("data-mart", "データマート", "data mart department analytics 部門データ", lambda: database(6, 10, 23, 27)+path('M29 25 H38 V47')+database(37, 7, 20, 20)+database(37, 35, 20, 20)+path('M38 17 H37 M38 47 H37')),
        ("data-catalog", "データカタログ", "data catalog metadata lineage メタデータ", lambda: rect(9, 8, 42, 47)+path('M18 8 V55 M14 17 H22 M14 29 H22 M14 41 H22')+circle(37, 29, 8, "#fff")+path('M43 35 L52 44', stroke=ACCENT, width=3)),
    ])
    category("04 セキュリティ", "#287c63", "#e1f2e9", [
        ("identity", "ユーザー認証", "identity authentication login idp 認証 sso", lambda: rect(5, 10, 54, 44)+person(22, 31, .8)+check(43, 33)),
        ("key", "暗号鍵", "key encryption kms secret 暗号 秘密", lambda: circle(19, 23, 12)+circle(19, 23, 4, "#fff")+path('M28 32 L50 54 L57 47 L51 41 L47 45 L42 40 L46 36 L39 29 L35 33', PALE)),
        ("security-shield", "セキュリティ保護", "shield protection secure 防御", lambda: shield()+check(29, 33)),
        ("lock", "アクセス制限", "lock access control restriction acl ロック", lambda: path('M20 28 V19 C20 3 44 3 44 19 V28', stroke=ACCENT, width=4)+rect(13, 28, 38, 29)+circle(32, 40, 4, "#fff")+path('M32 44 V49')),
        ("certificate", "電子証明書", "certificate tls ssl pki 証明書", lambda: document()+circle(42, 45, 10, "#fff")+path('M36 53 L34 61 L42 57 L49 61 L48 53', PALE)+check(41, 44)),
        ("mfa", "多要素認証", "mfa otp two factor 2fa 二要素", lambda: rect(7, 13, 26, 38)+person(20, 29, .65)+rect(38, 7, 19, 49, "#fff")+path('M43 13 H52 M44 50 H51')+dot(44, 29)+dot(51, 29)+check(46, 37)),
        ("audit", "監査", "audit compliance evidence コンプライアンス 証跡", lambda: document()+circle(43, 42, 11, "#fff")+path('M50 50 L59 59', stroke=ACCENT, width=4)+check(42, 40)),
        ("vulnerability", "脆弱性", "vulnerability bug cve security risk 脆弱性診断", lambda: shield()+circle(32, 32, 9, "#fff")+path('M32 23 V41 M27 20 L24 16 M37 20 L40 16 M23 28 H18 M41 28 H46 M23 36 H18 M41 36 H46')),
        ("soc", "SOC・脅威監視", "soc siem threat security operations 脅威", lambda: monitor()+path('M11 27 L20 27 L25 18 L31 33 L37 23 L43 27 H53', stroke=ACCENT)+dot(45, 16, 3)),
        ("authorization", "権限・ロール", "authorization permission role iam rbac アクセス権", lambda: person(18, 24, .9)+path('M18 37 V47 H41')+rect(37, 11, 20, 15, "#fff")+rect(37, 39, 20, 15)+path('M41 47 V26')+check(46, 18)),
    ])
    category("05 開発・運用", "#367b9a", "#e3f0f6", [
        ("monitoring", "稼働監視", "monitoring observability health ヘルスチェック", lambda: monitor()+path('M12 28 H21 L27 17 L33 32 L40 23 L45 28 H53', stroke=ACCENT)),
        ("alert", "アラート", "alert notification alarm 通知 警報", lambda: path('M13 44 L18 37 V25 C18 7 46 7 46 25 V37 L51 44 Z', PALE)+path('M26 51 Q32 59 38 51')+path('M32 20 V31', stroke=ACCENT, width=3)+dot(32, 37)),
        ("logs", "ログ", "logs logging log 記録", lambda: rect(8, 7, 48, 50)+path('M8 19 H56 M15 28 H19 M25 28 H49 M15 37 H19 M25 37 H44 M15 46 H19 M25 46 H47')+dot(14, 13)),
        ("metrics", "メトリクス", "metrics performance gauge 性能 指標", lambda: path('M9 48 A25 25 0 1 1 55 48 Z', PALE)+path('M16 36 L20 38 M23 20 L25 25 M41 20 L39 25 M48 36 L44 38')+path('M32 43 L43 28', stroke=ACCENT, width=3)+circle(32, 43, 4, "#fff")),
        ("tracing", "分散トレーシング", "trace tracing span apm 分散追跡", lambda: path('M10 10 V54 M10 17 H17 M10 31 H25 M10 45 H36')+rect(17, 12, 35, 10)+rect(25, 26, 27, 10)+rect(36, 40, 20, 10)),
        ("scheduler", "ジョブスケジューラー", "scheduler cron job schedule 定期実行", lambda: rect(7, 11, 41, 43)+path('M7 23 H48 M17 6 V17 M37 6 V17')+circle(43, 43, 15, "#fff")+path('M43 33 V43 L50 47', stroke=ACCENT)+dot(17, 32)+dot(28, 32)),
        ("ci-cd", "CI・CD", "ci cd cicd pipeline deployment devops デプロイ", lambda: path('M30 20 C13-1 1 21 15 34 C29 47 35 17 49 30 C64 43 47 65 33 45', stroke=ACCENT, width=4)+path('M25 14 L30 20 L23 23 M39 41 L33 45 L40 50', stroke=ACCENT, width=3)+dot(12, 24, 3)+dot(52, 41, 3)),
        ("testing", "テスト・品質保証", "test qa quality verification 品質", lambda: rect(8, 9, 40, 46)+path('M16 20 H39 M16 30 H28 M16 40 H26')+circle(45, 43, 13, "#fff")+check(43, 42)),
        ("incident", "障害対応", "incident outage problem 障害 インシデント", lambda: path('M32 6 L60 54 H4 Z', PALE)+path('M32 23 V37', stroke=ACCENT, width=4)+dot(32, 45, 2.5)),
        ("helpdesk", "ヘルプデスク", "helpdesk support service desk サポート", lambda: person(32, 28, 1.2)+path('M12 31 V24 C12 0 52 0 52 24 V31', stroke=ACCENT, width=3)+rect(8, 25, 8, 16, "#fff")+rect(48, 25, 8, 16, "#fff")+path('M52 41 V46 H39')+dot(37, 46, 3)),
    ])
    category("06 組織・拠点", "#a16b30", "#fff0dc", [
        ("employee", "担当者", "person employee staff user 担当 社員", lambda: person(32, 29, 1.7)),
        ("team", "チーム", "team group organization 部署 組織", lambda: person(15, 32, .9)+person(49, 32, .9)+person(32, 34, 1.25)),
        ("headquarters", "本社", "headquarters office company enterprise 会社 オフィス", lambda: building()+path('M23 3 H41 M32 3 V8', stroke=ACCENT)),
        ("branch-office", "支店・営業所", "branch office site location 拠点", lambda: rect(6, 9, 21, 31)+rect(38, 31, 20, 27)+path('M12 17 H20 M12 24 H20 M44 40 H52 M44 47 H52 M16 40 V51 H38', stroke=ACCENT)),
        ("factory", "工場", "factory manufacturing plant 製造", lambda: path('M6 28 L21 18 V28 L36 18 V28 H56 V57 H6 Z', PALE)+path('M44 28 V7 H52 V28')+rect(13, 38, 8, 10, "#fff", 0)+rect(28, 38, 8, 10, "#fff", 0)+rect(43, 38, 7, 10, "#fff", 0)),
        ("store", "店舗", "store shop retail 小売 店", lambda: rect(9, 25, 46, 32)+path('M5 25 L12 9 H52 L59 25 Z', PALE)+path('M17 9 L14 25 M27 9 L26 25 M37 9 L38 25 M47 9 L50 25')+rect(16, 36, 15, 21, "#fff", 0)+rect(39, 36, 10, 10, "#fff", 0)),
        ("home-office", "在宅勤務", "home office remote work 在宅 テレワーク", lambda: path('M5 28 L32 6 L59 28 M12 23 V56 H52 V23', PALE)+rect(20, 30, 24, 16, "#fff")+path('M27 46 V51 M21 51 H43')),
        ("remote-site", "遠隔拠点", "remote site geographic location リモート 地域", lambda: path('M18 7 C-1 7-1 30 18 45 C37 30 37 7 18 7 Z', PALE)+circle(18, 20, 5, "#fff")+path('M47 26 C31 26 31 44 47 58 C63 44 63 26 47 26 Z', PALE)+circle(47, 37, 4, "#fff")+path('M14 49 Q23 62 32 48 Q35 40 37 43', stroke=ACCENT)),
        ("partner", "パートナー企業", "partner vendor supplier alliance 取引先 ベンダー", lambda: path('M5 22 L15 13 L28 19 L38 14 L59 28 L49 48 L37 52 L17 40 Z', PALE)+path('M15 13 L9 29 L17 40 M38 14 L27 27 L31 32 L39 26 L53 36 M21 35 L35 47 M27 31 L42 43', stroke=ACCENT)),
        ("customer", "顧客", "customer client consumer 顧客 利用者", lambda: person(24, 30, 1.3)+path('M48 10 L51 18 L59 18 L53 24 L55 32 L48 27 L41 32 L43 24 L37 18 L45 18 Z', PALE)),
    ])
    category("07 業務プロセス", "#a36b3e", "#fff0e2", [
        ("task", "作業・タスク", "task work activity 作業", lambda: rect(9, 10, 46, 45)+rect(18, 6, 28, 10, "#fff")+path('M19 28 H45 M19 37 H40 M19 46 H34')),
        ("workflow", "ワークフロー", "workflow process flow 業務フロー", lambda: rect(6, 8, 20, 14)+rect(38, 42, 20, 14)+path('M16 22 V34 H48')+arrow(48, 34, 0, 8)+path('M26 15 H48 V27', stroke=ACCENT)),
        ("approval", "承認", "approval signoff permission 決裁 稟議", lambda: document()+path('M31 48 V40 H48 V48 M28 48 H52 V55 H28 Z', "#fff")+check(38, 32)),
        ("decision", "判断・分岐", "decision branch condition 判断 分岐 条件", lambda: path('M32 5 L53 24 L32 43 L11 24 Z', PALE)+path('M32 43 V57 M11 24 H4 V48', stroke=ACCENT)+path('M28 18 Q28 11 35 14 Q42 18 33 23 V27', stroke=ACCENT)+dot(33, 33)),
        ("business-document", "業務文書", "document paper specification 文書 仕様書", lambda: document()+path('M23 47 H43 M23 53 H37', stroke=ACCENT)),
        ("form", "申請フォーム", "form request input 申請 入力", lambda: rect(8, 7, 48, 50)+rect(15, 15, 34, 8, "#fff", 1)+rect(15, 29, 13, 8, "#fff", 1)+rect(36, 29, 13, 8, "#fff", 1)+rect(31, 44, 18, 7, None, 1)+path('M16 47 H24')),
        ("contract", "契約", "contract agreement legal 契約書 合意", lambda: document()+path('M23 48 Q28 38 30 48 T38 48 T46 46', stroke=ACCENT, width=2.5)+path('M48 30 L55 25 L59 29 L49 39 L43 41 Z', "#fff")),
        ("checklist", "チェックリスト", "checklist todo completion 点検 完了", lambda: rect(9, 7, 46, 50)+path('M16 18 L19 21 L24 14 M16 32 L19 35 L24 28 M16 46 L19 49 L24 42', stroke=ACCENT)+path('M31 18 H47 M31 32 H47 M31 46 H47')),
        ("automation", "業務自動化", "automation rpa robot 自動化", lambda: rect(10, 17, 44, 34)+path('M32 8 V17 M24 57 H40 M19 51 V57 M45 51 V57 M5 28 V41 M59 28 V41')+dot(32, 7, 3)+circle(23, 30, 4, "#fff")+circle(41, 30, 4, "#fff")+path('M23 42 H41', stroke=ACCENT)),
        ("handover", "業務引き継ぎ", "handover transfer handoff 引継ぎ 移管", lambda: person(13, 28, .85)+person(51, 39, .85)+rect(26, 22, 13, 19, "#fff", 1)+arrow(20, 11, 26, 0)+arrow(44, 55, -26, 0)),
    ])
    category("08 プロジェクト", "#845c9f", "#f2e9f8", [
        ("roadmap", "ロードマップ", "roadmap planning plan 計画", lambda: path('M16 58 V46 Q16 33 32 33 Q49 33 49 20 V5', stroke=ACCENT, width=4)+circle(16, 48, 5, "#fff")+circle(32, 33, 5, "#fff")+circle(49, 16, 5, "#fff")),
        ("gantt", "ガントチャート", "gantt schedule timeline 工程表", lambda: rect(6, 8, 52, 48, "#fff")+path('M6 19 H58 M17 8 V56')+rect(21, 24, 19, 6, None, 1)+rect(29, 35, 24, 6, None, 1)+rect(40, 46, 13, 6, None, 1)),
        ("milestone", "マイルストーン", "milestone checkpoint flag 節目", lambda: path('M5 48 H59 M17 48 V8 M18 10 H48 L42 20 L48 30 H18', PALE)+circle(17, 48, 5, "#fff")+dot(39, 48, 3)),
        ("kanban", "カンバン", "kanban agile scrum board アジャイル", lambda: rect(5, 8, 54, 48, "#fff")+path('M23 8 V56 M41 8 V56 M5 19 H59')+rect(10, 25, 8, 8, None, 1)+rect(10, 39, 8, 8, None, 1)+rect(28, 25, 8, 8, None, 1)+rect(46, 25, 8, 8, None, 1)+check(49, 44)),
        ("resources", "リソース配分", "resource allocation staffing capacity 要員 工数", lambda: person(15, 19, .7)+person(48, 19, .7)+rect(6, 40, 20, 15)+rect(38, 40, 20, 15)+path('M15 30 V40 M48 30 V40 M26 47 H38', stroke=ACCENT)+dot(12, 47)+dot(20, 47)+dot(44, 47)),
        ("budget", "予算・コスト", "budget cost finance money 費用 予算", lambda: rect(6, 17, 38, 34)+path('M6 26 H44')+circle(43, 39, 15, "#fff")+path('M36 31 L43 40 L50 31 M43 40 V50 M36 40 H50 M36 45 H50', stroke=ACCENT)),
        ("risk", "リスク管理", "risk issue mitigation リスク 課題", lambda: rect(6, 8, 39, 47)+path('M14 18 H36 M14 27 H30 M14 36 H24')+path('M44 28 L60 56 H28 Z', "#fff")+path('M44 38 V45', stroke=ACCENT, width=3)+dot(44, 51)),
        ("meeting", "会議", "meeting workshop discussion ワークショップ 打合せ", lambda: rect(15, 24, 34, 20)+circle(32, 11, 5, "#fff")+circle(32, 56, 5, "#fff")+circle(6, 34, 4, "#fff")+circle(58, 34, 4, "#fff")+path('M32 16 V24 M32 44 V51 M10 34 H15 M49 34 H54')),
        ("dependencies", "依存関係", "dependency dependencies critical path wbs 依存", lambda: rect(5, 8, 17, 15)+rect(42, 8, 17, 15)+rect(23, 42, 18, 15)+path('M13 23 V32 H50 V23')+arrow(32, 32, 0, 10)),
        ("deliverable", "成果物", "deliverable release delivery 成果 納品", lambda: path('M7 23 L32 11 L57 23 V49 L32 61 L7 49 Z', PALE)+path('M7 23 L32 35 L57 23 M32 35 V61 M19 17 L44 29')+circle(46, 13, 10, "#fff")+check(44, 12)),
    ])
    category("09 戦略・ビジネス", "#a45e59", "#fceae5", [
        ("vision", "ビジョン", "vision future direction 将来像 方針", lambda: path('M4 32 Q32 1 60 32 Q32 63 4 32 Z', PALE)+circle(32, 32, 12, "#fff")+circle(32, 32, 5)+path('M32 4 V10 M32 54 V60', stroke=ACCENT)),
        ("kpi-target", "目標・KPI", "target goal kpi okr 目標", lambda: circle(30, 34, 24)+circle(30, 34, 16, "#fff")+circle(30, 34, 7)+arrow(56, 8, -26, 26)),
        ("swot", "SWOT分析", "swot strength weakness opportunity threat 強み 弱み 機会 脅威", lambda: rect(7, 7, 50, 50, "#fff")+path('M32 7 V57 M7 32 H57')+path('M19 13 V25 M13 19 H25 M39 19 H51', stroke=ACCENT, width=3)+path('M13 49 L19 37 L25 49 Z', PALE)+path('M40 40 L50 50 M50 40 L40 50', stroke=ACCENT, width=3)),
        ("customer-journey", "カスタマージャーニー", "customer journey experience cx touchpoint 顧客体験", lambda: person(11, 14, .55)+path('M8 36 C17 17 31 52 42 31 L55 16', stroke=ACCENT, width=3)+circle(12, 32, 4, "#fff")+circle(30, 38, 4, "#fff")+circle(46, 26, 4, "#fff")+path('M50 13 L56 14 L57 21', stroke=ACCENT)),
        ("value-chain", "バリューチェーン", "value chain supply chain 価値連鎖 サプライチェーン", lambda: path('M5 18 H20 L30 32 L20 46 H5 L15 32 Z', PALE)+path('M22 18 H37 L47 32 L37 46 H22 L32 32 Z', "#fff")+path('M39 18 H49 L59 32 L49 46 H39 L49 32 Z', PALE)),
        ("portfolio", "ポートフォリオ", "portfolio investment product business 事業 製品", lambda: rect(6, 16, 52, 41)+path('M21 16 V7 H43 V16 M6 31 H58')+rect(26, 27, 12, 9, "#fff", 1)+path('M16 43 H25 M16 49 H35', stroke=ACCENT)),
        ("transformation", "変革・DX", "transformation dx change digital デジタル変革", lambda: rect(5, 16, 18, 30)+path('M10 24 H18 M10 31 H18 M10 38 H18')+arrow(24, 31, 16, 0)+path('M43 13 L57 20 V43 L43 50 L33 44 V37', PALE)+path('M43 29 L57 20 M43 29 V50')),
        ("business-model", "ビジネスモデル", "business model canvas revenue model 収益モデル", lambda: rect(5, 10, 54, 44, "#fff")+path('M18 10 V42 M31 10 V42 M45 10 V42 M5 42 H59 M31 26 H59 M31 42 V54')+dot(11, 25, 3)+dot(24, 25, 3)+dot(38, 18, 3)+dot(51, 34, 3)),
        ("competitive-analysis", "競合比較", "competitive competitor benchmark positioning 競合 ベンチマーク", lambda: path('M11 54 V12 H53 M11 54 H57 M11 54 L49 16', stroke=ACCENT)+circle(25, 37, 6)+circle(42, 39, 4, "#fff")+circle(41, 22, 7, "#fff")),
        ("stakeholders", "ステークホルダー", "stakeholder influence interest relationship 利害関係者", lambda: circle(32, 32, 10)+path('M32 22 V13 M22 32 H13 M42 32 H51 M32 42 V51')+person(32, 7, .4)+person(7, 31, .4)+person(57, 31, .4)+person(32, 55, .4)),
    ])
    category("10 分析・報告", "#416b9c", "#e8eefb", [
        ("report", "レポート", "report reporting presentation 資料 報告書", lambda: document()+path('M24 52 V45 H29 V52 M34 52 V39 H39 V52', PALE)),
        ("dashboard", "ダッシュボード", "dashboard analytics cockpit 分析画面", lambda: rect(5, 8, 54, 48, "#fff")+rect(11, 14, 18, 15)+rect(35, 14, 18, 15)+rect(11, 35, 42, 15)+path('M16 44 L24 39 L32 45 L41 40 L48 43', stroke=ACCENT)),
        ("bar-chart", "棒グラフ", "bar chart histogram 棒グラフ 比較", lambda: chart_axes()+rect(17, 33, 8, 21, None, 1)+rect(31, 23, 8, 31, None, 1)+rect(45, 12, 8, 42, None, 1)),
        ("line-chart", "折れ線グラフ", "line chart trend time series 傾向 推移", lambda: chart_axes()+path('M16 43 L27 29 L37 36 L52 17', stroke=ACCENT, width=3)+dot(16, 43, 3)+dot(27, 29, 3)+dot(37, 36, 3)+dot(52, 17, 3)),
        ("pie-chart", "円グラフ", "pie chart share ratio 構成比", lambda: path('M29 9 A24 24 0 1 0 53 33 H29 Z', PALE)+path('M35 5 V27 H57 A22 22 0 0 0 35 5 Z', "#fff")+path('M29 33 L13 51', stroke=ACCENT)),
        ("scatter-plot", "散布図", "scatter plot correlation distribution 相関 分布", lambda: chart_axes()+''.join(dot(x, y, r) for x, y, r in [(18, 43, 3), (23, 34, 3), (31, 37, 3), (34, 26, 3), (45, 21, 4), (49, 13, 3), (51, 31, 3)])),
        ("funnel", "ファネル", "funnel conversion pipeline sales コンバージョン", lambda: path('M5 9 H59 L49 23 H15 Z', PALE)+path('M17 28 H47 L39 40 H25 Z', "#fff")+path('M27 45 H37 V57 H27 Z', PALE)),
        ("matrix", "評価マトリクス", "matrix evaluation prioritization priority 評価 優先順位", lambda: rect(7, 7, 50, 50, "#fff")+path('M7 23 H57 M7 40 H57 M23 7 V57 M40 7 V57')+circle(31, 31, 4)+check(47, 31)+dot(31, 48, 3)),
        ("insight", "インサイト・仮説", "insight hypothesis idea innovation 知見 仮説 アイデア", lambda: path('M21 43 C21 35 12 32 14 21 C17 1 47 1 50 21 C52 32 43 35 43 43 Z', PALE)+path('M23 49 H41 M26 55 H38 M29 43 V30 L23 25 M35 43 V30 L41 25', stroke=ACCENT)),
        ("survey", "アンケート・調査", "survey research questionnaire interview 調査 ヒアリング", lambda: rect(6, 8, 37, 47)+path('M13 19 H22 M13 29 H22 M13 39 H22')+rect(28, 15, 8, 8, "#fff", 1)+rect(28, 25, 8, 8, "#fff", 1)+path('M38 34 H59 V50 H50 L43 58 V50 H38 Z', PALE)+dot(44, 42)+dot(51, 42)),
    ])
    return entries, categories


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when generated files differ")
    args = parser.parse_args()
    entries, categories = make_catalog()
    assert len(entries) == 100 and len({e["file"] for e in entries}) == 100
    assert len({e["svg"] for e in entries}) == 100
    inventory = ["# yydraw 業務・ITアイコン100種", "",
                 "システム構成図、業務フロー、プロジェクト計画、戦略・分析資料向けの独自SVGアイコンです。",
                 "左のカテゴリで「業務・IT / 01 …」～「業務・IT / 10 …」を選び、名前または検索語で絞り込めます。",
                 "名前と画像を表示し、ドラッグして配置します。配置した図柄だけが文書に保存されます。",
                 "ラベルは他のyyシリーズと同じフォントを使用します。図柄内に文字や外部フォントは含みません。", "",
                 "既存クラウドアイコン1,834種と合わせて1,934種です。公式クラウド素材とは別の独自図柄で、",
                 "このリポジトリと同じGPL-3.0-or-laterで提供します。特定製品のロゴは使用していません。", "",
                 "再生成: `python tools/business-icons/generate.py`。整合性確認: `python tools/business-icons/generate.py --check`。", ""]
    for name, rows in categories:
        inventory += [f"## {name}", "", "| 名前 | 検索語 |", "| --- | --- |"]
        inventory += [f"| {label} | {keywords} |" for label, keywords, _ in rows]
        inventory.append("")
    files = [(OUT, json.dumps(entries, ensure_ascii=False, indent=2) + "\n"),
             (INVENTORY, "\n".join(inventory))]
    for filename, content in files:
        if args.check:
            if not filename.exists() or filename.read_text(encoding="utf-8") != content:
                raise SystemExit(f"Generated file is out of date: {filename}")
        else:
            filename.parent.mkdir(parents=True, exist_ok=True)
            filename.write_text(content, encoding="utf-8", newline="\n")
    print(f"Business/IT catalog: {len(entries)} unique icons in {len(categories)} categories")


if __name__ == "__main__":
    main()
