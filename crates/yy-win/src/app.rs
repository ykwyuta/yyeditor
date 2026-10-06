//! アプリケーション状態とウィンドウプロシージャ。

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Controls::{
    NMHDR, SB_SETPARTS, SB_SETTEXTW, SBARS_SIZEGRIP, STATUSCLASSNAMEW, SetScrollInfo, TCIF_TEXT,
    TCITEMW, TCM_DELETEALLITEMS, TCM_GETCURSEL, TCM_INSERTITEMW, TCM_SETCURSEL, TCN_SELCHANGE,
    WC_TABCONTROLW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, FOS_ALLOWMULTISELECT, FileOpenDialog,
    FileSaveDialog, HDROP, IFileDialog, IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, Result, w};
use yy_buffer::{LineLookup, Snapshot};
use yy_config::Config;
use yy_core::edit::Change;
use yy_core::{
    Document, EditKind, Encoding, Eol, OpenOptions, Replacement, SaveError, Searcher, Selection,
    SelectionSet, motion,
};
use yy_jobs::JobHandle;
use yy_jobs::{JobPool, Notifier};
use yy_layout::rect::{self, RectEdit, RectRow};
use yy_layout::{
    ColumnConfig, RectSelection, Row, RowConfig, Viewport, next_row_start, prev_row_start, row_at,
    row_containing, rows_from,
};

mod codemode;
mod csvmode;
mod hexmode;
mod previewmode;
mod syntaxmode;
pub(crate) mod workspacemode;

pub(crate) use csvmode::colhead_proc;
use previewmode::ID_PREVIEW;
pub(crate) use previewmode::translate_preview_shortcut;

use crate::findbar::{self, FindBar};
use crate::render::{Composition, Frame, RectPaint, Renderer};
use crate::util::{Context, error_box, group_digits, human_size, info_box, wide};
use crate::{COLHEAD_CLASS, FRAME_CLASS, VIEW_CLASS, clipboard, default_proc, hiword, ime, loword};
use codemode::*;
use csvmode::*;
use hexmode::*;
use syntaxmode::*;
use workspacemode::*;

// メニュー・アクセラレータのコマンド ID
const ID_OPEN: u16 = 101;
const ID_CLOSE: u16 = 102;
const ID_EXIT: u16 = 103;
const ID_NEW: u16 = 104;
const ID_SAVE: u16 = 105;
const ID_SAVE_AS: u16 = 106;
const ID_OPEN_SHARED: u16 = 107;
const ID_TAB_NEXT: u16 = 108;
const ID_TAB_PREV: u16 = 109;
const ID_DIFF: u16 = 110;
const ID_HISTORY: u16 = 112;
const ID_BOOKMARKS: u16 = 113;
const ID_BOOKMARK_TOGGLE: u16 = 114;
const ID_OPEN_REMOTE: u16 = 115;
const ID_SAVE_AS_REMOTE: u16 = 116;
const ID_SAVE_AS_LOCAL: u16 = 117;
const ID_GOTO: u16 = 201;
const ID_ZOOM_IN: u16 = 202;
const ID_ZOOM_OUT: u16 = 203;
const ID_ZOOM_RESET: u16 = 204;
const ID_LINE_NUMBERS: u16 = 205;
const ID_CONTROL_CHARS: u16 = 206;
const ID_WHITESPACE: u16 = 208;
const ID_ABOUT: u16 = 301;
const ID_HELP: u16 = 302;
const ID_HELP_KEYS: u16 = 303;
const ID_OPEN_SETTINGS: u16 = 304;
const ID_OPEN_REMOTE_LOG: u16 = 305;
const ID_FORGET_PASSWORDS: u16 = 306;
const ID_UNDO: u16 = 401;
const ID_REDO: u16 = 402;
const ID_CUT: u16 = 403;
const ID_COPY: u16 = 404;
const ID_PASTE: u16 = 405;
const ID_SELECT_ALL: u16 = 406;
const ID_DELETE: u16 = 407;
const ID_RECT_MODE: u16 = 408;
const ID_RECT_TO_CARETS: u16 = 409;
const ID_SELECT_NEXT: u16 = 410;
const ID_SELECT_ALL_OCCURRENCES: u16 = 411;
const ID_CARETS_AT_LINE_ENDS: u16 = 412;
// 選択範囲の変換（編集メニューの「変換」）と重複行の削除
const ID_TO_UPPER: u16 = 420;
const ID_TO_LOWER: u16 = 421;
const ID_TO_FULL_KANA: u16 = 422;
const ID_TO_HALF_KANA: u16 = 423;
const ID_TO_CAMEL: u16 = 424;
const ID_TO_SNAKE: u16 = 425;
const ID_TO_KEBAB: u16 = 426;
const ID_DEDUP_LINES: u16 = 427;
const ID_FIND: u16 = 501;
const ID_REPLACE: u16 = 502;
const ID_FIND_NEXT: u16 = 503;
const ID_FIND_PREV: u16 = 504;
const ID_FIND_CLOSE: u16 = 505;
const ID_REPLACE_ONE: u16 = 506;
const ID_REPLACE_ALL: u16 = 507;
/// 検索条件（文字列・オプション）が変わった
const ID_FIND_CHANGED: u16 = 508;
/// 検索欄で Enter（Shift なら前を検索、置換欄なら置換）
const ID_FIND_OK: u16 = 509;
/// 検索欄に入力した（インクリメンタル検索）
const ID_FIND_INCREMENTAL: u16 = 510;
const ID_GREP: u16 = 511;
const ID_TAG_JUMP: u16 = 512;
const ID_SELECT_SEARCH_MATCHES: u16 = 513;
/// 検索バーの置換の行の表示を切り替える
const ID_FIND_TOGGLE_REPLACE: u16 = 515;
/// 「文字コードを指定して開き直す」の各項目（`Encoding::all()` の順）
const ID_REOPEN_BASE: u16 = 600;

const ID_STATUS: i32 = 1000;
const ID_TABS: i32 = 1001;
const TIMER_BLINK: usize = 1;
/// 保存の進捗をステータスバーに表示する
const TIMER_PROGRESS: usize = 2;
/// 編集後にプレビューを更新する
const TIMER_PREVIEW: usize = 3;

/// ワーカースレッドから行数カウントの進捗を知らせるメッセージ
const WM_APP_INDEX: u32 = WM_APP + 1;
const WM_APP_RELAYOUT: u32 = WM_APP + 2;
const WM_APP_SCROLLBARS: u32 = WM_APP + 3;
const WM_APP_REPAINT: u32 = WM_APP + 4;
const WM_APP_RESIZE: u32 = WM_APP + 5;

/// バイト位置比例スクロールバーの範囲
const SCROLL_RANGE: i32 = 1 << 16;
/// クリップボードにコピーできる最大サイズ
const MAX_CLIPBOARD_BYTES: u64 = 256 << 20;
/// この大きさ以下の文書はその場で検索する（それより大きければバックグラウンドで）
const SYNC_SEARCH_BYTES: u64 = 32 << 20;
/// インクリメンタル検索でその場で探す範囲
const INCREMENTAL_BYTES: u64 = 4 << 20;
/// ハイライトする一致箇所の上限（表示範囲内）
const HIGHLIGHT_LIMIT: usize = 5000;
/// 件数を数える上限
const COUNT_LIMIT: u64 = 100_000_000;
/// 矩形選択で一度に編集できる最大行数
const RECT_EDIT_LIMIT: usize = 1_000_000;
/// 「すべての出現箇所を選択」「各行末にカーソル」で作るカーソルの上限
const CARET_LIMIT: usize = 100_000;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// アプリ状態を借用して `f` を実行する。再入中（すでに借用中）なら `None`。
///
/// メッセージボックスやファイルダイアログはモーダルループ中にウィンドウプロシージャを
/// 再入させるため、`f` の中では呼ばないこと。
pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

pub(crate) fn shutdown() {
    // ドキュメント（mmap）とジョブプールを UI スレッド上で解放する
    let app = APP.with(|cell| cell.borrow_mut().take());
    drop(app);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollMode {
    /// 行数比例（行数が確定していて少ない場合）
    Lines,
    /// バイト位置比例（巨大ファイル・行数未確定）
    Bytes,
}

/// マウスでドラッグ選択中の状態。
struct Drag {
    anchor: u64,
    /// Ctrl+クリックで追加中なら、追加前の選択
    base: Option<SelectionSet>,
    /// 矩形選択のドラッグか
    rect: bool,
}

pub(crate) struct App {
    frame: HWND,
    view: HWND,
    status: HWND,
    tabbar: HWND,
    tabs: Vec<Option<TabState>>,
    active_tab: usize,
    menu_edit: HMENU,
    menu_view: HMENU,
    menu_csv: HMENU,
    /// 「ハイライト」の子メニュー
    menu_syntax: HMENU,
    config: Config,
    rows_cfg: RowConfig,
    pool: JobPool,
    doc: Document,
    vp: Viewport,
    scroll_x: f32,
    scroll_mode: ScrollMode,
    renderer: Renderer,
    /// ビューのクライアント領域（ピクセル）
    view_px: (u32, u32),
    show_line_numbers: bool,
    /// 空白・タブ・改行を記号で表示する
    show_whitespace: bool,
    index_posted: Arc<AtomicBool>,
    caret_visible: bool,
    focused: bool,
    overwrite: bool,
    composition: Option<Composition>,
    drag: Option<Drag>,
    /// WM_CHAR で届いたサロゲートペアの前半
    high_surrogate: Option<u16>,
    /// 矩形選択中ならその範囲（文書の選択は矩形の先端のカーソル 1 つにしておく）
    rect: Option<RectSelection>,
    /// 矩形選択モード（Shift+移動やドラッグを矩形選択として扱う）
    rect_mode: bool,
    ccfg: ColumnConfig,
    /// Alt+ドラッグの後の Alt キーの解放でメニューが開かないようにする
    suppress_alt_up: bool,
    /// 保存すると符号が変わる文字があることを警告済み
    warned_noncanonical: bool,
    findbar: FindBar,
    /// 検索バーの条件をコンパイルしたもの（誤りがあれば `None`）
    searcher: Option<Arc<Searcher>>,
    /// バックグラウンドの「次を検索」と件数カウント
    find_job: Option<FindJob>,
    select_job: Option<SelectJob>,
    count_job: Option<CountJob>,
    /// 件数と、数えたときの文書の版
    match_count: Option<(u64, u64)>,
    /// ステータスバーに出す検索・置換の結果
    status_msg: String,
    /// 区切り文字モード
    csv: Option<CsvState>,
    /// バックグラウンドで実行中のレコードの書き直し（完了時の後処理用）
    pending_record_op: Option<yy_core::csv::RecordOp>,
    /// バックグラウンドで重複行を削除中（完了時のメッセージ用）
    pending_dedup: bool,
    grep_job: Option<GrepJob>,
    /// 表示行のキャッシュ（長い行の表示テキストを作り直さないように）
    row_cache: RefCell<RowCache>,
    /// 終わった Grep の結果（フレームで文書として開く）
    grep_done: Option<String>,
    /// バックグラウンドで保存中の保存の手順（完了時の後処理用）
    save_flow: Option<SaveFlow>,
    /// 改行コードの変換（バックグラウンド）が終わったら始める保存
    save_after_convert: Option<SaveFlow>,
    /// 終わった保存（フレームで結果を処理する）
    save_done: Option<(Option<SaveFlow>, yy_core::SaveDone)>,
    /// 作業中のタブの保存を待っている・結果を処理している（その間は裏のタブを表に出さない）
    defer_inactive_saves: u32,
    /// 前回の Grep の条件
    grep_last: crate::grepdlg::GrepRequest,
    /// ハイライトの定義の一覧（組み込み＋利用者の定義）
    syntaxes: yy_core::syntax::Registry,
    /// 文書のハイライト（なければ色を付けない）
    syntax: Option<SyntaxState>,
    /// 利用者が「なし」を選んだ（ファイル種類から選び直さない）
    syntax_off: bool,
    /// 括弧の対応（（表示の版, キャレット位置）, 結果）
    bracket_cache: Option<BracketCache>,
    /// 16 進数（バイナリ）表示
    hex: Option<HexState>,
    /// コード値表示
    code: Option<CodeState>,
    /// 区切り文字モードの列見出し（A, B, …）
    colhead: HWND,
    /// 列見出しの高さ（ピクセル。表示していなければ 0）
    colhead_h: i32,
    /// タブ・ステータスバーの文字のフォント（メニューと同じ Windows のフォント）
    ui_font: windows::Win32::Graphics::Gdi::HFONT,
    /// Markdown・HTML のプレビュー（右側）
    preview: previewmode::PreviewPane,
    /// SSH 接続先のファイルの編集（11 章）
    /// ワークスペースとサイドバー（左側）
    ws: WorkspacePane,
}

/// 非表示タブの文書と表示位置。検索条件と表示設定はウィンドウ全体で共有する。
struct TabState {
    doc: Document,
    vp: Viewport,
    scroll_x: f32,
    scroll_mode: ScrollMode,
    rect: Option<RectSelection>,
    csv: Option<CsvState>,
    pending_record_op: Option<yy_core::csv::RecordOp>,
    pending_dedup: bool,
    save_flow: Option<SaveFlow>,
    save_after_convert: Option<SaveFlow>,
    warned_noncanonical: bool,
    status_msg: String,
    syntax: Option<SyntaxState>,
    syntax_off: bool,
    hex: Option<HexState>,
    code: Option<CodeState>,
}

impl TabState {
    fn new(doc: Document) -> Self {
        Self {
            doc,
            vp: Viewport::default(),
            scroll_x: 0.0,
            scroll_mode: ScrollMode::Lines,
            rect: None,
            csv: None,
            pending_record_op: None,
            pending_dedup: false,
            save_flow: None,
            save_after_convert: None,
            warned_noncanonical: false,
            status_msg: String::new(),
            syntax: None,
            syntax_off: false,
            hex: None,
            code: None,
        }
    }
}

/// 表示行のキャッシュ。文書の版・表示の設定が変わったら捨てる。
#[derive(Default)]
struct RowCache {
    /// （文書の版, 区切り文字モードの設定, 表示行の最大バイト数）
    key: (u64, usize, u64, bool),
    rows: std::collections::HashMap<u64, Row>,
}

/// バックグラウンドの Grep。
struct GrepJob {
    job: JobHandle,
    rx: crossbeam_channel::Receiver<String>,
}

// 使わなくなったバックグラウンドの処理は止める
impl Drop for FindJob {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

impl Drop for CountJob {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

impl Drop for SelectJob {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

impl Drop for GrepJob {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

/// バックグラウンドの「次を検索」。
struct FindJob {
    job: JobHandle,
    rx: crossbeam_channel::Receiver<Option<(std::ops::Range<u64>, bool)>>,
    version: u64,
}

/// バックグラウンドの件数カウント。
struct CountJob {
    job: JobHandle,
    rx: crossbeam_channel::Receiver<u64>,
    version: u64,
}

struct SelectJob {
    job: JobHandle,
    rx: crossbeam_channel::Receiver<Vec<std::ops::Range<u64>>>,
    version: u64,
}

pub(crate) fn create_accelerators() -> Result<HACCEL> {
    let ctrl = FVIRTKEY | FCONTROL;
    let ctrl_shift = FVIRTKEY | FCONTROL | FSHIFT;
    let accels = [
        (ctrl, b'N' as u16, ID_NEW),
        (ctrl, b'O' as u16, ID_OPEN),
        (ctrl_shift, b'O' as u16, ID_OPEN_REMOTE),
        (ctrl, b'E' as u16, ID_HISTORY),
        (ctrl, b'B' as u16, ID_BOOKMARKS),
        (ctrl_shift, b'B' as u16, ID_BOOKMARK_TOGGLE),
        (ctrl, b'S' as u16, ID_SAVE),
        (ctrl_shift, b'S' as u16, ID_SAVE_AS),
        (ctrl, b'W' as u16, ID_CLOSE),
        (ctrl, VK_TAB.0, ID_TAB_NEXT),
        (ctrl_shift, VK_TAB.0, ID_TAB_PREV),
        (ctrl, b'Z' as u16, ID_UNDO),
        (ctrl, b'Y' as u16, ID_REDO),
        (ctrl_shift, b'Z' as u16, ID_REDO),
        (ctrl, b'X' as u16, ID_CUT),
        (ctrl, b'C' as u16, ID_COPY),
        (ctrl, b'V' as u16, ID_PASTE),
        (ctrl_shift, b'V' as u16, ID_PREVIEW),
        (ctrl, b'A' as u16, ID_SELECT_ALL),
        (ctrl_shift, b'U' as u16, ID_TO_UPPER),
        (ctrl, b'U' as u16, ID_TO_LOWER),
        (ctrl, b'D' as u16, ID_SELECT_NEXT),
        (ctrl_shift, b'L' as u16, ID_SELECT_ALL_OCCURRENCES),
        (ctrl_shift, b'M' as u16, ID_SELECT_SEARCH_MATCHES),
        (
            FVIRTKEY | FALT | FSHIFT,
            b'I' as u16,
            ID_CARETS_AT_LINE_ENDS,
        ),
        (ctrl, b'G' as u16, ID_GOTO),
        (ctrl_shift, b'X' as u16, ID_HEX_MODE),
        (ctrl_shift, b'K' as u16, ID_CODE_MODE),
        (ctrl_shift, b'R' as u16, ID_RECORD_MODE),
        (ctrl_shift, b'E' as u16, ID_WS_SIDEBAR),
        (ctrl, VK_OEM_2.0, ID_TOGGLE_COMMENT),
        (ctrl, VK_DIVIDE.0, ID_TOGGLE_COMMENT),
        (ctrl, VK_OEM_6.0, ID_GOTO_BRACKET),
        (ctrl, b'F' as u16, ID_FIND),
        (ctrl, b'H' as u16, ID_REPLACE),
        (FVIRTKEY, VK_F3.0, ID_FIND_NEXT),
        (FVIRTKEY | FSHIFT, VK_F3.0, ID_FIND_PREV),
        (ctrl_shift, b'F' as u16, ID_GREP),
        (FVIRTKEY, VK_F12.0, ID_TAG_JUMP),
        (FVIRTKEY, VK_F1.0, ID_HELP),
        (ctrl, VK_ADD.0, ID_ZOOM_IN),
        (ctrl, VK_OEM_PLUS.0, ID_ZOOM_IN),
        (ctrl, VK_SUBTRACT.0, ID_ZOOM_OUT),
        (ctrl, VK_OEM_MINUS.0, ID_ZOOM_OUT),
        (ctrl, b'0' as u16, ID_ZOOM_RESET),
        (ctrl, VK_NUMPAD0.0, ID_ZOOM_RESET),
    ]
    .map(|(fvirt, key, cmd)| ACCEL {
        fVirt: fvirt,
        key,
        cmd,
    });
    unsafe { CreateAcceleratorTableW(&accels) }
}

fn create_menu() -> Result<(HMENU, HMENU, HMENU, HMENU)> {
    unsafe {
        let item = |menu: HMENU, id: u16, text: windows::core::PCWSTR| {
            AppendMenuW(menu, MF_STRING, id as usize, text)
        };
        let sep = |menu: HMENU| AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let bar = CreateMenu()?;

        let file = CreatePopupMenu()?;
        item(file, ID_NEW, w!("新規作成(&N)\tCtrl+N"))?;
        item(file, ID_OPEN, w!("開く(&O)...\tCtrl+O"))?;
        item(
            file,
            ID_OPEN_REMOTE,
            w!("リモートのファイルを開く(&E)...\tCtrl+Shift+O"),
        )?;
        item(file, ID_OPEN_BINARY, w!("バイナリとして開く(&B)..."))?;
        item(
            file,
            ID_OPEN_SHARED,
            w!("共有中のファイルを読み取り専用で開く..."),
        )?;
        item(file, ID_HISTORY, w!("最近開いたファイル(&H)...\tCtrl+E"))?;
        item(file, ID_BOOKMARKS, w!("ブックマーク(&K)...\tCtrl+B"))?;
        item(
            file,
            ID_BOOKMARK_TOGGLE,
            w!("このファイルをブックマーク(&M)\tCtrl+Shift+B"),
        )?;
        let reopen = CreatePopupMenu()?;
        for (i, e) in Encoding::all().iter().enumerate() {
            AppendMenuW(
                reopen,
                MF_STRING,
                (ID_REOPEN_BASE + i as u16) as usize,
                &HSTRING::from(e.label()),
            )?;
        }
        AppendMenuW(
            file,
            MF_POPUP,
            reopen.0 as usize,
            w!("文字コードを指定して開き直す(&R)"),
        )?;
        item(file, ID_SAVE, w!("上書き保存(&S)\tCtrl+S"))?;
        item(
            file,
            ID_SAVE_AS,
            w!("名前を付けて保存(&A)...\tCtrl+Shift+S"),
        )?;
        item(
            file,
            ID_SAVE_AS_REMOTE,
            w!("リモートに名前を付けて保存(&T)..."),
        )?;
        item(
            file,
            ID_SAVE_AS_LOCAL,
            w!("このパソコンに名前を付けて保存(&L)..."),
        )?;
        item(file, ID_CLOSE, w!("閉じる(&C)\tCtrl+W"))?;
        sep(file)?;
        item(file, ID_EXIT, w!("終了(&X)\tAlt+F4"))?;

        let edit = CreatePopupMenu()?;
        item(edit, ID_UNDO, w!("元に戻す(&U)\tCtrl+Z"))?;
        item(edit, ID_REDO, w!("やり直し(&R)\tCtrl+Y"))?;
        sep(edit)?;
        item(edit, ID_CUT, w!("切り取り(&T)\tCtrl+X"))?;
        item(edit, ID_COPY, w!("コピー(&C)\tCtrl+C"))?;
        item(edit, ID_PASTE, w!("貼り付け(&P)\tCtrl+V"))?;
        item(edit, ID_DELETE, w!("削除(&D)\tDel"))?;
        sep(edit)?;
        item(edit, ID_SELECT_ALL, w!("すべて選択(&A)\tCtrl+A"))?;
        item(edit, ID_SELECT_NEXT, w!("次の出現箇所を選択に追加\tCtrl+D"))?;
        item(
            edit,
            ID_SELECT_ALL_OCCURRENCES,
            w!("すべての出現箇所を選択\tCtrl+Shift+L"),
        )?;
        item(
            edit,
            ID_SELECT_SEARCH_MATCHES,
            w!("検索条件に一致する箇所をすべて選択\tCtrl+Shift+M"),
        )?;
        item(
            edit,
            ID_CARETS_AT_LINE_ENDS,
            w!("選択した各行の行末にカーソル\tAlt+Shift+I"),
        )?;
        sep(edit)?;
        item(edit, ID_RECT_MODE, w!("矩形選択モード(&B)"))?;
        item(edit, ID_RECT_TO_CARETS, w!("矩形選択をカーソルに変換"))?;
        sep(edit)?;
        item(edit, ID_TOGGLE_COMMENT, w!("コメント化 / 解除(&M)\tCtrl+/"))?;
        let convert = CreatePopupMenu()?;
        item(convert, ID_TO_UPPER, w!("大文字に(&U)\tCtrl+Shift+U"))?;
        item(convert, ID_TO_LOWER, w!("小文字に(&L)\tCtrl+U"))?;
        sep(convert)?;
        item(convert, ID_TO_FULL_KANA, w!("全角カタカナに(&Z)"))?;
        item(convert, ID_TO_HALF_KANA, w!("半角カタカナに(&H)"))?;
        sep(convert)?;
        item(
            convert,
            ID_TO_CAMEL,
            w!("キャメルケースに（camelCase）(&C)"),
        )?;
        item(
            convert,
            ID_TO_SNAKE,
            w!("スネークケースに（snake_case）(&S)"),
        )?;
        item(convert, ID_TO_KEBAB, w!("ケバブケースに（kebab-case）(&K)"))?;
        AppendMenuW(edit, MF_POPUP, convert.0 as usize, w!("変換(&V)"))?;
        item(edit, ID_DEDUP_LINES, w!("重複行を削除(&L)"))?;

        let view = CreatePopupMenu()?;
        item(view, ID_GOTO, w!("行へ移動(&G)...\tCtrl+G"))?;
        sep(view)?;
        item(view, ID_ZOOM_IN, w!("拡大(&I)\tCtrl++"))?;
        item(view, ID_ZOOM_OUT, w!("縮小(&O)\tCtrl+-"))?;
        item(view, ID_ZOOM_RESET, w!("標準のサイズ(&R)\tCtrl+0"))?;
        sep(view)?;
        AppendMenuW(
            view,
            MF_STRING | MF_CHECKED,
            ID_LINE_NUMBERS as usize,
            w!("行番号(&L)"),
        )?;
        item(view, ID_PREVIEW, w!("プレビュー(&P)\tCtrl+Shift+V"))?;
        item(view, ID_WHITESPACE, w!("空白・タブ・改行を表示(&W)"))?;
        item(view, ID_CONTROL_CHARS, w!("制御文字を表示"))?;
        item(view, ID_HEX_MODE, w!("16 進数表示(&X)\tCtrl+Shift+X"))?;
        item(view, ID_CODE_MODE, w!("コード値表示(&K)\tCtrl+Shift+K"))?;
        item(view, ID_RECORD_MODE, w!("固定長表示(&R)...\tCtrl+Shift+R"))?;
        AppendMenuW(
            view,
            MF_POPUP,
            hexmode::create_charset_menu()?.0 as usize,
            w!("16 進数表示の文字の欄の文字コード(&E)"),
        )?;
        sep(view)?;
        item(view, ID_DIFF, w!("開いているファイルを比較..."))?;

        let search = CreatePopupMenu()?;
        item(search, ID_FIND, w!("検索(&F)...\tCtrl+F"))?;
        item(search, ID_REPLACE, w!("置換(&R)...\tCtrl+H"))?;
        sep(search)?;
        item(search, ID_FIND_NEXT, w!("次を検索(&N)\tF3"))?;
        item(search, ID_FIND_PREV, w!("前を検索(&P)\tShift+F3"))?;
        sep(search)?;
        item(
            search,
            ID_GREP,
            w!("ファイルから検索 (Grep)(&G)...\tCtrl+Shift+F"),
        )?;
        item(search, ID_TAG_JUMP, w!("タグジャンプ(&J)\tF12"))?;
        item(
            search,
            ID_GOTO_BRACKET,
            w!("対応する括弧へ移動(&B)\tCtrl+]"),
        )?;

        let help = CreatePopupMenu()?;
        item(help, ID_HELP, w!("ヘルプ(&H)\tF1"))?;
        item(help, ID_HELP_KEYS, w!("キーボードショートカット(&K)"))?;
        item(help, ID_OPEN_SETTINGS, w!("設定ファイルを開く(&S)"))?;
        item(help, ID_OPEN_REMOTE_LOG, w!("リモート接続の記録を開く(&R)"))?;
        item(
            help,
            ID_FORGET_PASSWORDS,
            w!("保存したリモート接続のパスワードを削除(&P)..."),
        )?;
        sep(help)?;
        item(help, ID_ABOUT, w!("バージョン情報(&A)"))?;
        AppendMenuW(bar, MF_POPUP, file.0 as usize, w!("ファイル(&F)"))?;
        AppendMenuW(bar, MF_POPUP, edit.0 as usize, w!("編集(&E)"))?;
        AppendMenuW(bar, MF_POPUP, search.0 as usize, w!("検索(&S)"))?;
        AppendMenuW(bar, MF_POPUP, view.0 as usize, w!("表示(&V)"))?;
        let ws = create_workspace_menu()?;
        AppendMenuW(bar, MF_POPUP, ws.0 as usize, w!("ワークスペース(&W)"))?;
        let csv = create_csv_menu()?;
        AppendMenuW(bar, MF_POPUP, csv.0 as usize, w!("CSV(&C)"))?;
        AppendMenuW(bar, MF_POPUP, help.0 as usize, w!("ヘルプ(&H)"))?;
        Ok((bar, edit, view, csv))
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

impl App {
    /// ウィンドウを作成してアプリ状態を初期化する。フレームウィンドウを返す。
    pub(crate) fn create(
        hinstance: HINSTANCE,
        initial_file: Option<PathBuf>,
        initial_line: Option<u64>,
        ssh: Option<yy_remote::ConnectorFactory>,
    ) -> Result<HWND> {
        let (config, config_error) = Config::load();
        // 外部の対応表（%APPDATA%\yyeditor\mappings\*.map）
        let mut mapping_errors = yy_config::config_dir()
            .map(|d| yy_encoding::load_mappings(&d.join("mappings")))
            .unwrap_or_default();
        // ハイライトの定義（%APPDATA%\yyeditor\syntax\*.toml で追加・置き換え）
        let mut syntaxes = yy_core::syntax::Registry::builtin();
        if let Some(d) = yy_config::config_dir() {
            mapping_errors.extend(syntaxes.load_dir(&d.join("syntax")));
        }
        unsafe {
            let (menu, menu_edit, menu_view, menu_csv) = create_menu().context("create_menu")?;
            let menu_syntax =
                append_syntax_menu(menu_view, &syntaxes.list()).context("create_menu")?;
            let frame = CreateWindowExW(
                WS_EX_ACCEPTFILES,
                FRAME_CLASS,
                w!("yyeditor"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1100,
                760,
                None,
                Some(menu),
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(frame)")?;
            let view = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                VIEW_CLASS,
                None,
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_HSCROLL,
                0,
                0,
                0,
                0,
                Some(frame),
                None,
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(view)")?;
            let colhead = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                COLHEAD_CLASS,
                None,
                WS_CHILD,
                0,
                0,
                0,
                0,
                Some(frame),
                None,
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(colhead)")?;
            let tabbar = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                WC_TABCONTROLW,
                None,
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                Some(frame),
                Some(HMENU(ID_TABS as isize as *mut _)),
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(tabs)")?;
            crate::tabclose::install(tabbar, frame);
            let status = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                STATUSCLASSNAMEW,
                None,
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SBARS_SIZEGRIP),
                0,
                0,
                0,
                0,
                Some(frame),
                Some(HMENU(ID_STATUS as isize as *mut _)),
                Some(hinstance),
                None,
            )
            .context("CreateWindowExW(status)")?;
            DragAcceptFiles(frame, true);

            let dpi = GetDpiForWindow(view).max(96);
            let findbar = FindBar::create(frame, dpi)?;
            let renderer = Renderer::new(
                &config.editor.font_family,
                config.editor.font_size,
                config.editor.tab_width,
                config.colors.clone(),
                dpi,
            )?;
            let mut renderer = renderer;
            renderer.set_ambiguous_wide(config.editor.ambiguous_wide);
            renderer.set_show_whitespace(config.view.show_whitespace);
            let ccfg = ColumnConfig {
                tab_width: config.editor.tab_width,
                ambiguous_wide: config.editor.ambiguous_wide,
                wide_box_line: renderer.wide_box_line(),
            };
            crate::remote::install(
                crate::remote::RemoteState::new(ssh, config.remote.clone()),
                crate::remote::Host {
                    frame,
                    status: |t| {
                        with_app(|a| a.show_status_message(t));
                    },
                },
            );
            let ws = create_pane(frame, hinstance)?;
            let app = App {
                frame,
                view,
                status,
                tabbar,
                tabs: vec![None],
                active_tab: 0,
                menu_edit,
                menu_view,
                menu_csv,
                menu_syntax,
                rows_cfg: RowConfig::new(config.view.max_row_bytes.max(256) as u64),
                show_line_numbers: config.view.line_numbers,
                show_whitespace: config.view.show_whitespace,
                config,
                pool: JobPool::new(0),
                doc: Document::new_empty(),
                vp: Viewport::default(),
                scroll_x: 0.0,
                scroll_mode: ScrollMode::Lines,
                renderer,
                view_px: (0, 0),
                index_posted: Arc::new(AtomicBool::new(false)),
                caret_visible: true,
                focused: false,
                overwrite: false,
                composition: None,
                drag: None,
                high_surrogate: None,
                rect: None,
                rect_mode: false,
                ccfg,
                suppress_alt_up: false,
                warned_noncanonical: false,
                findbar,
                searcher: None,
                find_job: None,
                select_job: None,
                count_job: None,
                match_count: None,
                status_msg: String::new(),
                csv: None,
                pending_record_op: None,
                pending_dedup: false,
                grep_job: None,
                row_cache: RefCell::new(RowCache::default()),
                grep_done: None,
                save_flow: None,
                save_after_convert: None,
                save_done: None,
                defer_inactive_saves: 0,
                grep_last: crate::grepdlg::GrepRequest {
                    files: "*.*".into(),
                    recursive: true,
                    ..Default::default()
                },
                syntaxes,
                syntax: None,
                syntax_off: false,
                bracket_cache: None,
                hex: None,
                code: None,
                colhead,
                colhead_h: 0,
                ui_font: crate::util::ui_font(dpi),
                preview: Default::default(),
                ws,
            };
            APP.with(|cell| *cell.borrow_mut() = Some(app));
            with_app(|a| {
                a.apply_ui_font();
                a.rebuild_tree();
                a.update_workspace_menu();
                a.refresh_tabs();
                a.update_line_number_menu();
                a.update_syntax_menu();
                a.layout_children();
                a.update_title();
                a.update_status();
                a.update_scrollbars();
            });

            let _ = ShowWindow(frame, SW_SHOWDEFAULT);
            let _ = SetFocus(Some(view));

            if let Some(e) = config_error {
                error_box(frame, &e.to_string());
            }
            if !mapping_errors.is_empty() {
                error_box(
                    frame,
                    &format!(
                        "対応表・ハイライトの定義を読み込めませんでした。\n\n{}",
                        mapping_errors.join("\n")
                    ),
                );
            }
            if let Some(path) = initial_file {
                open_path(frame, path, None, false);
                if let Some(line) = initial_line.filter(|n| *n >= 1) {
                    with_app(|a| a.goto_line(line));
                }
            }
            Ok(frame)
        }
    }

    // ---- 寸法 ------------------------------------------------------------

    fn page_rows(&self) -> usize {
        let h = self.renderer.px_to_dip(self.view_px.1 as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        ((h / lh).floor() as usize).max(1)
    }

    fn text_origin_x(&self) -> f32 {
        self.renderer
            .text_origin_x(self.line_digits(), self.show_line_numbers)
    }

    fn text_area_width(&self) -> f32 {
        let w = self.renderer.px_to_dip(self.view_px.0 as f32);
        (w - self.text_origin_x()).max(0.0)
    }

    fn line_digits(&self) -> usize {
        let n = self.doc.snapshot().estimated_line_count();
        n.to_string().len()
    }

    /// `start` から最大 `count` 個の表示行（[`rows_from`] と同じ。作った行はキャッシュする）。
    fn cached_rows(&self, start: u64, count: usize) -> Vec<Row> {
        let snap = self.doc.snapshot();
        let cfg = &self.rows_cfg;
        let key = (
            self.doc.version(),
            cfg.cells.as_ref().map_or(0, |c| Arc::as_ptr(c) as usize),
            cfg.max_row_bytes,
            cfg.show_controls,
        );
        let mut cache = self.row_cache.borrow_mut();
        if cache.key != key || cache.rows.len() > 1024 {
            cache.rows.clear();
            cache.key = key;
        }
        let len = snap.len();
        let mut out = Vec::with_capacity(count.min(1024));
        let mut pos = Some(start);
        while out.len() < count {
            let Some(p) = pos else { break };
            if p > len || (p == len && !yy_layout::is_line_start(snap, p)) {
                break;
            }
            let row = match cache.rows.get(&p) {
                Some(r) => r.clone(),
                None => {
                    let r = row_at(snap, cfg, p);
                    cache.rows.insert(p, r.clone());
                    r
                }
            };
            pos = next_row_start(snap, cfg, p);
            out.push(row);
        }
        out
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.view), None, false);
            if self.colhead_h > 0 {
                let _ = InvalidateRect(Some(self.colhead), None, false);
            }
        }
    }

    fn notifier(&self) -> Notifier {
        let frame = self.frame.0 as isize;
        let posted = self.index_posted.clone();
        Arc::new(move || {
            if !posted.swap(true, Ordering::AcqRel) {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(frame as *mut _)),
                        WM_APP_INDEX,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        })
    }

    // ---- ステータスバー・タイトル ----------------------------------------

    /// タブ・ステータスバーに画面の部品のフォントを設定する（設定しないとタブは
    /// 古いシステムフォントになり、メニューなどと見た目がそろわない）。
    fn apply_ui_font(&self) {
        for w in [self.tabbar, self.status, self.ws.tree] {
            unsafe {
                SendMessageW(
                    w,
                    WM_SETFONT,
                    Some(WPARAM(self.ui_font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }

    /// DPI が変わったらフォントを作り直す。
    fn set_ui_dpi(&mut self, dpi: u32) {
        let old = std::mem::replace(&mut self.ui_font, crate::util::ui_font(dpi));
        self.apply_ui_font();
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(old.into());
        }
    }

    fn layout_children(&mut self) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.frame, &mut rc);
            SendMessageW(self.status, WM_SIZE, None, None);
            let mut src = RECT::default();
            let _ = GetWindowRect(self.status, &mut src);
            let sh = src.bottom - src.top;
            let full_w = rc.right - rc.left;
            // ワークスペースのサイドバーを左端に置き、タブ・エディタ・プレビューはその右
            let left = self.layout_sidebar(full_w, (rc.bottom - rc.top - sh).max(0));
            let w = full_w - left;
            let tab_h = (32 * GetDpiForWindow(self.frame) as i32 / 96).max(24);
            let _ = MoveWindow(self.tabbar, left, 0, w, tab_h, true);
            // プレビューを表示していれば右側に置き、エディタ（検索バーを含む）はその左
            let body_h = (rc.bottom - rc.top - sh - tab_h).max(0);
            let ew = self.layout_preview(left, w, tab_h, body_h);
            let bar_h = self.findbar.height();
            self.findbar.layout(ew);
            let _ = MoveWindow(self.findbar.hwnd, left, tab_h, ew, bar_h, true);
            // 区切り文字モードでは列見出しを本文の上に置く
            let head_h = self.column_header_height();
            self.colhead_h = head_h;
            let _ = MoveWindow(self.colhead, left, tab_h + bar_h, ew, head_h, true);
            let _ = ShowWindow(self.colhead, if head_h > 0 { SW_SHOWNA } else { SW_HIDE });
            let h = (body_h - bar_h - head_h).max(0);
            let _ = MoveWindow(self.view, left, tab_h + bar_h + head_h, ew, h, true);
            let w = full_w;
            // 位置 | サイズ | 文字コード | 改行コード | 挿入/上書き | 進捗・メッセージ・コード値
            // 最後の -1 は「右端まで」（0 にすると進捗の欄が見えなくなる）
            let dpi = GetDpiForWindow(self.frame).max(96) as i32;
            let px = |v: i32| v * dpi / 96;
            let fixed = [px(100), px(210), px(150), px(90)];
            // 最後の欄はコード値（10 文字）が入る幅を優先し、位置の欄は 170 以上
            let rest = w - fixed.iter().sum::<i32>();
            let last = px(420).min((rest - px(170)).max(px(160)));
            let pos = (rest - last).max(0);
            let mut parts = [0, 0, 0, 0, 0, -1];
            let mut x = pos;
            parts[0] = x;
            for (i, f) in fixed.iter().enumerate() {
                x += f;
                parts[i + 1] = x.max(0);
            }
            SendMessageW(
                self.status,
                SB_SETPARTS,
                Some(WPARAM(parts.len())),
                Some(LPARAM(parts.as_ptr() as isize)),
            );
        }
    }

    fn set_status(&self, part: usize, text: &str) {
        let s = wide(text);
        unsafe {
            SendMessageW(
                self.status,
                SB_SETTEXTW,
                Some(WPARAM(part)),
                Some(LPARAM(s.as_ptr() as isize)),
            );
        }
    }

    fn update_title(&self) {
        let mark = if self.doc.is_modified() { "*" } else { "" };
        let read = if self.doc.is_read_only() {
            " [読み取り専用]"
        } else {
            ""
        };
        // リモートのファイルは接続先も示す
        let host = self
            .doc
            .remote()
            .and_then(|r| yy_remote::RemoteUri::parse(&r.uri))
            .map(|u| format!(" [{}]", u.target()))
            .unwrap_or_default();
        let ws = self
            .workspace_title()
            .map(|n| format!(" - {n}"))
            .unwrap_or_default();
        let title = format!(
            "{mark}{}{host}{read}{ws} - yyeditor",
            self.doc.display_name()
        );
        unsafe {
            let _ = SetWindowTextW(self.frame, &HSTRING::from(title));
        }
        self.refresh_tabs();
    }

    fn refresh_tabs(&self) {
        unsafe {
            SendMessageW(self.tabbar, TCM_DELETEALLITEMS, None, None);
            for (i, slot) in self.tabs.iter().enumerate() {
                let doc = if i == self.active_tab {
                    &self.doc
                } else {
                    &slot.as_ref().expect("inactive tab").doc
                };
                let mut label = format!(
                    "{}{}{}",
                    if doc.is_modified() { "*" } else { "" },
                    doc.display_name(),
                    if doc.is_read_only() { " [読]" } else { "" }
                );
                if label.chars().count() > 32 {
                    label = label.chars().take(29).collect::<String>() + "...";
                }
                // 閉じるボタンの場所
                label += crate::tabclose::LABEL_PAD;
                let mut wide_label = wide(&label);
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: windows::core::PWSTR(wide_label.as_mut_ptr()),
                    ..Default::default()
                };
                SendMessageW(
                    self.tabbar,
                    TCM_INSERTITEMW,
                    Some(WPARAM(i)),
                    Some(LPARAM((&item as *const TCITEMW) as isize)),
                );
            }
            SendMessageW(
                self.tabbar,
                TCM_SETCURSEL,
                Some(WPARAM(self.active_tab)),
                None,
            );
        }
    }

    fn update_status(&self) {
        let snap = self.doc.snapshot();
        let sels = self.doc.selections();
        let head = sels.primary().head;
        let pos = snap.line_of_offset(head);
        let approx = if pos.exact { "" } else { "約 " };
        let col = motion::column_of(snap, head, 1 << 20)
            .map(|c| group_digits(c + 1))
            .unwrap_or_else(|| "-".into());
        let mut text = format!("  {approx}{} 行, {col} 列", group_digits(pos.line + 1));
        if let Some(p) = self.csv_position() {
            text += &p;
        }
        let selected: u64 = sels.iter().map(|s| s.end() - s.start()).sum();
        if let Some(r) = self.rect {
            let lines =
                snap.line_of_offset(r.bottom()).line - snap.line_of_offset(r.top()).line + 1;
            text += &format!(
                "  (矩形 {} 行 × {} 桁)",
                group_digits(lines),
                r.right() - r.left()
            );
        } else if sels.len() > 1 {
            text += &format!("  (カーソル {} 個)", sels.len());
        }
        if selected > 0 {
            text += &format!("  ({} バイト選択)", group_digits(selected));
        }
        if self.hex.is_some() {
            text = self.hex_status();
        } else if self.code.is_some() {
            text += &self.code_status();
        }
        self.set_status(0, &text);
        self.set_status(1, &format!("  {}", human_size(snap.len())));
        let mut enc = self.doc.encoding().name().to_owned();
        if self.doc.has_bom() {
            enc += " (BOM 付き)";
        }
        let stats = self.doc.decode_stats();
        if stats.invalid > 0 {
            enc += &format!("  不正バイト {}", group_digits(stats.invalid));
        }
        if let Some(records) = self.doc.encoding().records() {
            enc += &format!(" / {}", records.label());
        }
        self.set_status(2, &format!("  {enc}"));
        let syntax = self
            .syntax
            .as_ref()
            .filter(|_| self.csv.is_none())
            .map(|s| format!("  {}", s.view.syntax().name))
            .unwrap_or_default();
        if let Some(n) = self.hex.and_then(|h| h.record) {
            self.set_status(3, &format!("  固定長 {} バイト", group_digits(n)));
        } else if self.hex.is_some() {
            self.set_status(3, "  16 進数");
        } else if self.code.is_some() {
            self.set_status(3, &format!("  {}  コード値", self.doc.eol().label()));
        } else {
            self.set_status(3, &format!("  {}{syntax}", self.doc.eol().label()));
        }
        let overwrite = match (self.hex, self.code) {
            (Some(h), _) => h.overwrite,
            (_, Some(c)) => c.overwrite,
            _ => self.overwrite,
        };
        let special = self.hex.is_some() || self.code.is_some();
        let mode = match (overwrite, self.rect_mode && !special) {
            (false, false) => "  挿入",
            (true, false) => "  上書き",
            (false, true) => "  挿入 / 矩形",
            (true, true) => "  上書き / 矩形",
        };
        self.set_status(4, mode);
        let progress = match (self.doc.loading_progress(), self.doc.indexing_progress()) {
            _ if self.doc.save_progress().is_some() => format!(
                "  保存しています（Esc で中止）… {:.0}%",
                self.doc.save_progress().unwrap_or(0.0) * 100.0
            ),
            (Some(p), _) => format!("  読み込んでいます（読み取り専用）… {:.0}%", p * 100.0),
            _ if self.doc.replace_progress().is_some() => format!(
                "  書き換えています（Esc で中止）… {:.0}%",
                self.doc.replace_progress().unwrap_or(0.0) * 100.0
            ),
            _ if self.grep_job.is_some() => format!(
                "  Grep: {} ファイル目を検索しています（Esc で中止）…",
                self.grep_job
                    .as_ref()
                    .map_or(0, |j| j.job.progress().done())
            ),
            _ if self.find_job.is_some() => format!(
                "  検索しています（Esc で中止）… {:.0}%",
                self.find_job
                    .as_ref()
                    .map_or(0.0, |j| j.job.progress().fraction())
                    * 100.0
            ),
            _ if self.select_job.is_some() => format!(
                "  一致箇所を選択しています（Esc で中止）… {:.0}%",
                self.select_job
                    .as_ref()
                    .map_or(0.0, |j| j.job.progress().fraction())
                    * 100.0
            ),
            (None, Some(p)) => format!("  行数を数えています… {:.0}%", p * 100.0),
            _ if self.csv.as_ref().is_some_and(|c| !c.view.is_complete()) => format!(
                "  CSV を解析しています… {:.0}%",
                self.csv.as_ref().map_or(0.0, |c| c.view.progress()) * 100.0
            ),
            _ => {
                let mut m = format!("  {}", self.status_msg);
                if self.status_msg.is_empty() && self.hex.is_none() {
                    m += &self.code_values();
                }
                if self.findbar.visible
                    && let Some((n, v)) = self.match_count
                    && v == self.doc.version()
                {
                    m += &format!("  （{} 件）", group_digits(n));
                }
                m
            }
        };
        self.set_status(5, &progress);
    }

    /// 選択した文字列のコード値（先頭 10 文字まで）。Unicode の文字コードの文書では符号位置、
    /// それ以外ではその文字コードでの値。選択がなければ空。
    fn code_values(&self) -> String {
        const MAX_CHARS: usize = 10;
        let sel = self.doc.selections().primary();
        if sel.is_empty() || self.rect.is_some() {
            return String::new();
        }
        let r = sel.range();
        let bytes = self
            .doc
            .snapshot()
            .read(r.start..r.end.min(r.start + 4 * MAX_CHARS as u64 + 4));
        let enc = self.doc.encoding();
        let unicode = enc.is_unicode();
        let mut parts = Vec::new();
        let mut more = r.end - r.start > bytes.len() as u64;
        'outer: for chunk in bytes.utf8_chunks() {
            let items = chunk
                .valid()
                .chars()
                .map(Ok)
                .chain(chunk.invalid().iter().map(|&b| Err(b)));
            for item in items {
                if parts.len() == MAX_CHARS {
                    more = true;
                    break 'outer;
                }
                parts.push(match item {
                    // 読み込み時に不正だったバイト（エスケープ文字）・UTF-8 として不正なバイト
                    Err(b) => format!("\\x{b:02X}"),
                    Ok(c) => match yy_encoding::unescape_char(c) {
                        Some(b) if unicode => format!("\\x{b:02X}"),
                        Some(b) => format!("{b:02X}"),
                        None if unicode => format!("{:04X}", c as u32),
                        None => {
                            let mut buf = [0u8; 4];
                            match yy_encoding::encode_all(
                                enc,
                                c.encode_utf8(&mut buf).as_bytes(),
                                yy_encoding::EscapeMode::Literal,
                            ) {
                                Ok(b) if !b.is_empty() => {
                                    b.iter().map(|x| format!("{x:02X}")).collect::<String>()
                                }
                                _ => "?".to_owned(),
                            }
                        }
                    },
                });
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        // 幅を節約するため、U+ や文字コード名は先頭に 1 回だけ書く
        let prefix = if unicode {
            "U+".to_owned()
        } else {
            enc.name().to_owned()
        };
        format!(
            "コード値 ({prefix}) {}{}",
            parts.join(" "),
            if more { " …" } else { "" }
        )
    }

    /// 選択範囲（空なら単語）の文字列を変換する。
    fn transform_selection(&mut self, t: yy_core::transform::Transform) {
        if self.hex.is_some() || self.code.is_some() || self.rect.is_some() {
            self.status_msg = if self.hex.is_some() {
                "16 進数表示では使えません".into()
            } else if self.code.is_some() {
                "コード値表示では使えません".into()
            } else {
                "矩形選択では使えません（「矩形選択をカーソルに変換」してから使ってください）"
                    .into()
            };
            self.update_status();
            return;
        }
        if self.doc.transform_selections(t) {
            self.after_edit();
        }
    }

    /// 重複する行を削除する（選択範囲の行、なければ文書全体）。
    fn dedup_lines(&mut self) {
        if self.hex.is_some() || self.code.is_some() {
            return;
        }
        let n = self.notifier();
        match self.doc.dedup_lines(&self.pool, n) {
            Ok(Some(removed)) => {
                self.status_msg = if removed == 0 {
                    "重複する行はありません".into()
                } else {
                    format!("重複する {} 行を削除しました", group_digits(removed))
                };
                self.after_edit();
            }
            Ok(None) => {
                self.pending_dedup = true;
                self.update_status();
                self.invalidate();
            }
            Err(e) => {
                self.status_msg = format!("重複行を削除できませんでした: {e}");
                self.update_status();
            }
        }
    }

    fn update_scrollbars(&mut self) {
        if self.hex.is_some() {
            self.hex_update_scrollbars();
            return;
        }
        if self.code.is_some() {
            self.code_update_scrollbars();
            return;
        }
        let snap = self.doc.snapshot();
        let page = self.page_rows();
        let mut si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            ..Default::default()
        };
        match snap.line_count() {
            Some(lines) if lines <= self.config.view.line_scroll_limit => {
                self.scroll_mode = ScrollMode::Lines;
                si.nMin = 0;
                si.nMax = (lines as i32 - 1).max(0);
                si.nPage = page as u32;
                si.nPos = snap.line_of_offset(self.vp.top).line as i32;
            }
            _ => {
                self.scroll_mode = ScrollMode::Bytes;
                let len = snap.len().max(1);
                let rows = self.cached_rows(self.vp.top, page);
                let visible = rows.last().map(|r| r.next - self.vp.top).unwrap_or(0);
                si.nMin = 0;
                si.nMax = SCROLL_RANGE - 1;
                si.nPage = ((visible as f64 / len as f64) * SCROLL_RANGE as f64)
                    .clamp(1.0, SCROLL_RANGE as f64) as u32;
                si.nPos = (self.vp.fraction(snap) * SCROLL_RANGE as f64) as i32;
            }
        }
        unsafe {
            SetScrollInfo(self.view, SB_VERT, &si, true);
        }
        let area = self.text_area_width();
        let hsi = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: (self.renderer.max_text_width + self.renderer.metrics().char_width * 4.0) as i32,
            nPage: area as u32,
            nPos: self.scroll_x as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.view, SB_HORZ, &hsi, true);
        }
    }

    fn update_menu_state(&self) {
        let set = |id: u16, on: bool| unsafe {
            let flag = if on { MF_ENABLED } else { MF_GRAYED };
            let _ = EnableMenuItem(self.menu_edit, id as u32, MF_BYCOMMAND | flag);
        };
        let has_sel = !self.doc.selections().all_empty();
        set(ID_UNDO, self.doc.can_undo());
        set(ID_REDO, self.doc.can_redo());
        let has_sel = has_sel || self.rect.is_some_and(|r| !r.is_zero_width());
        set(ID_CUT, has_sel && !self.doc.is_read_only());
        set(ID_COPY, has_sel);
        set(ID_DELETE, has_sel && !self.doc.is_read_only());
        set(ID_RECT_TO_CARETS, self.rect.is_some());
        // 開いているファイルがブックマークにあればチェックを付ける
        unsafe {
            let menu = GetMenu(self.frame);
            let path = self.doc.location();
            let marked = path.as_deref().is_some_and(|p| {
                crate::recentdlg::ListKind::Bookmarks
                    .load()
                    .contains(&crate::recentdlg::normalize(p))
            });
            let flag = if marked { MF_CHECKED } else { MF_UNCHECKED };
            CheckMenuItem(menu, ID_BOOKMARK_TOGGLE as u32, (MF_BYCOMMAND | flag).0);
            let enable = if path.is_some() {
                MF_ENABLED
            } else {
                MF_GRAYED
            };
            let _ = EnableMenuItem(menu, ID_BOOKMARK_TOGGLE as u32, MF_BYCOMMAND | enable);
        }
        let flag = if self.rect_mode {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        unsafe {
            CheckMenuItem(self.menu_edit, ID_RECT_MODE as u32, (MF_BYCOMMAND | flag).0);
        }
    }

    // ---- スクロール ------------------------------------------------------

    fn after_scroll(&mut self) {
        self.update_scrollbars();
        self.update_status();
        self.invalidate();
        self.sync_preview_scroll();
    }

    fn scroll_rows(&mut self, delta: i64) {
        if self.hex.is_some() {
            self.hex_scroll_rows(delta);
            return;
        }
        if self.code.is_some() {
            self.code_scroll_rows(delta);
            return;
        }
        let page = self.page_rows();
        if self
            .vp
            .scroll_rows(self.doc.snapshot(), &self.rows_cfg, delta, page)
        {
            self.after_scroll();
        }
    }

    fn scroll_to_offset(&mut self, offset: u64) {
        if self.hex.is_some() {
            self.hex_scroll_to(offset);
            return;
        }
        if self.code.is_some() {
            self.code_scroll_to(offset);
            return;
        }
        let page = self.page_rows();
        self.vp
            .scroll_to_offset(self.doc.snapshot(), &self.rows_cfg, offset, page);
        self.after_scroll();
    }

    fn max_scroll_x(&self) -> f32 {
        (self.renderer.max_text_width + self.renderer.metrics().char_width * 4.0
            - self.text_area_width())
        .max(0.0)
    }

    fn scroll_horizontal(&mut self, x: f32) {
        let x = x.clamp(0.0, self.max_scroll_x().max(self.scroll_x));
        if x != self.scroll_x {
            self.scroll_x = x.max(0.0);
            self.update_scrollbars();
            self.invalidate();
        }
    }

    fn on_vscroll(&mut self, code: u32) {
        let page = self.page_rows() as i64;
        match SCROLLBAR_COMMAND(code as i32) {
            SB_LINEUP => self.scroll_rows(-1),
            SB_LINEDOWN => self.scroll_rows(1),
            SB_PAGEUP => self.scroll_rows(-(page - 1).max(1)),
            SB_PAGEDOWN => self.scroll_rows((page - 1).max(1)),
            SB_TOP => self.scroll_to_offset(0),
            SB_BOTTOM => self.scroll_to_offset(u64::MAX),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                unsafe {
                    let _ = GetScrollInfo(self.view, SB_VERT, &mut si);
                }
                if self.hex.is_some() {
                    self.hex_scroll_to_fraction(si.nTrackPos);
                    return;
                }
                if self.code.is_some() {
                    self.code_scroll_to_fraction(si.nTrackPos);
                    return;
                }
                let snap = self.doc.snapshot().clone();
                match self.scroll_mode {
                    ScrollMode::Lines => {
                        if let LineLookup::Found(off) = snap.line_start(si.nTrackPos as u64, false)
                        {
                            self.scroll_to_offset(off);
                        }
                    }
                    ScrollMode::Bytes => {
                        let f = si.nTrackPos as f64 / SCROLL_RANGE as f64;
                        let rows = self.page_rows();
                        self.vp.scroll_to_fraction(&snap, &self.rows_cfg, f, rows);
                        // ドラッグ中はつまみ位置を動かさない（位置の再計算で揺れるため）
                        self.update_status();
                        self.invalidate();
                    }
                }
            }
            _ => {}
        }
    }

    fn on_hscroll(&mut self, code: u32) {
        let cw = self.renderer.metrics().char_width;
        let area = self.text_area_width();
        match SCROLLBAR_COMMAND(code as i32) {
            SB_LINELEFT => self.scroll_horizontal(self.scroll_x - cw * 4.0),
            SB_LINERIGHT => self.scroll_horizontal(self.scroll_x + cw * 4.0),
            SB_PAGELEFT => self.scroll_horizontal(self.scroll_x - area * 0.8),
            SB_PAGERIGHT => self.scroll_horizontal(self.scroll_x + area * 0.8),
            SB_LEFT => self.scroll_horizontal(0.0),
            SB_RIGHT => self.scroll_horizontal(f32::MAX),
            SB_THUMBTRACK | SB_THUMBPOSITION => {
                let mut si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_TRACKPOS,
                    ..Default::default()
                };
                unsafe {
                    let _ = GetScrollInfo(self.view, SB_HORZ, &mut si);
                }
                self.scroll_horizontal(si.nTrackPos as f32);
            }
            _ => {}
        }
    }

    fn on_wheel(&mut self, delta: i16, horizontal: bool) {
        if key_down(VK_CONTROL) && !horizontal {
            let step = if delta > 0 { 1.0 } else { -1.0 };
            self.zoom(self.renderer.font_size_pt() + step);
            return;
        }
        let notches = delta as f32 / WHEEL_DELTA as f32;
        if horizontal || key_down(VK_SHIFT) {
            let sign = if horizontal { 1.0 } else { -1.0 };
            let dx = sign * notches * self.renderer.metrics().char_width * 8.0;
            self.scroll_horizontal(self.scroll_x + dx);
        } else {
            let rows = -(notches * self.config.view.wheel_lines as f32).round() as i64;
            self.scroll_rows(rows);
        }
    }

    fn zoom(&mut self, pt: f32) {
        if let Err(e) = self.renderer.set_font_size(pt) {
            error_box(self.frame, &format!("フォントを変更できません: {e}"));
            return;
        }
        let page = self.page_rows();
        self.vp.clamp(self.doc.snapshot(), &self.rows_cfg, page);
        self.after_scroll();
    }

    fn update_line_number_menu(&self) {
        let flag = if self.show_line_numbers {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        unsafe {
            CheckMenuItem(
                self.menu_view,
                ID_LINE_NUMBERS as u32,
                (MF_BYCOMMAND | flag).0,
            );
            CheckMenuItem(
                self.menu_view,
                ID_CONTROL_CHARS as u32,
                (MF_BYCOMMAND
                    | if self.rows_cfg.show_controls {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    })
                .0,
            );
            CheckMenuItem(
                self.menu_view,
                ID_WHITESPACE as u32,
                (MF_BYCOMMAND
                    | if self.show_whitespace {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    })
                .0,
            );
        }
    }

    // ---- キャレット ------------------------------------------------------

    /// 描画キャッシュを文書の版に合わせる（ヒットテストの前に呼ぶ）。
    fn sync_renderer(&mut self) {
        self.renderer.set_version(self.doc.version());
    }

    fn row_of(&self, offset: u64) -> Row {
        let snap = self.doc.snapshot();
        row_at(
            snap,
            &self.rows_cfg,
            row_containing(snap, &self.rows_cfg, offset),
        )
    }

    /// キャレットの点滅をやり直す（操作直後は表示する）。
    fn reset_blink(&mut self) {
        self.caret_visible = true;
        if self.focused {
            unsafe {
                let ms = GetCaretBlinkTime();
                if ms != u32::MAX && ms > 0 {
                    SetTimer(Some(self.view), TIMER_BLINK, ms, None);
                }
            }
        }
    }

    /// 主キャレットが画面内に入るように縦横にスクロールする。
    fn ensure_caret_visible(&mut self) {
        if self.hex.is_some() {
            self.hex_ensure_visible();
            return;
        }
        if self.code.is_some() {
            self.code_ensure_visible();
            return;
        }
        self.sync_renderer();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows();
        self.vp
            .ensure_visible(self.doc.snapshot(), &self.rows_cfg, head, page);
        let row = self.row_of(head);
        let x = self.renderer.caret_x(&row, head);
        let area = self.text_area_width();
        let margin = (self.renderer.metrics().char_width * 4.0).min(area / 3.0);
        if x < self.scroll_x + margin {
            self.scroll_x = (x - margin).max(0.0);
        } else if x > self.scroll_x + area - margin {
            self.scroll_x = x - area + margin;
        }
    }

    /// キャレットを動かした後の共通処理。
    fn after_move(&mut self) {
        self.sync_column_header();
        self.reset_blink();
        self.ensure_caret_visible();
        self.update_scrollbars();
        self.update_status();
        self.update_ime_position();
        self.invalidate();
    }

    /// 内容を変更した後の共通処理。
    fn after_edit(&mut self) {
        self.select_job = None;
        self.sync_csv();
        self.sync_syntax();
        if !self.doc.snapshot().is_fully_indexed() {
            let n = self.notifier();
            self.doc.maintain_indexing(&self.pool, n);
        }
        self.after_move();
        self.update_title();
        self.schedule_preview();
    }

    /// 行 `rows` 行分上下に移動した位置と、その水平位置。
    fn vertical_target(
        &mut self,
        snap: &Snapshot,
        offset: u64,
        goal_x: Option<f32>,
        rows: i64,
    ) -> (u64, f32) {
        let cfg = self.rows_cfg.clone();
        let start = row_containing(snap, &cfg, offset);
        let x = match goal_x {
            Some(x) => x,
            None => {
                let row = row_at(snap, &cfg, start);
                self.renderer.caret_x(&row, offset)
            }
        };
        let mut r = start;
        for _ in 0..rows.unsigned_abs() {
            let n = if rows > 0 {
                next_row_start(snap, &cfg, r)
            } else {
                prev_row_start(snap, &cfg, r)
            };
            match n {
                Some(n) => r = n,
                None => return (if rows > 0 { snap.len() } else { 0 }, x),
            }
        }
        let target = row_at(snap, &cfg, r);
        (self.renderer.hit_test(&target, x), x)
    }

    fn move_vertical(&mut self, rows: i64, extend: bool) {
        self.sync_renderer();
        let snap = self.doc.snapshot().clone();
        let sels = self.doc.selections().clone();
        let moved = sels.map(|s| {
            let (head, x) = self.vertical_target(&snap, s.head, s.goal_x, rows);
            let mut n = if extend {
                Selection::new(s.anchor, head)
            } else {
                Selection::caret(head)
            };
            n.goal_x = Some(x);
            n
        });
        self.doc.set_selections(moved);
        self.after_move();
    }

    /// 主カーソルの上下の行にカーソルを追加する（Ctrl+Alt+↑↓）。
    fn add_caret_vertical(&mut self, rows: i64) {
        self.sync_renderer();
        let snap = self.doc.snapshot().clone();
        let mut sels = self.doc.selections().clone();
        let p = *sels.primary();
        let (head, x) = self.vertical_target(&snap, p.head, p.goal_x, rows);
        let mut c = Selection::caret(head);
        c.goal_x = Some(x);
        sels.add(c);
        self.doc.set_selections(sels);
        self.after_move();
    }

    fn move_horizontal(&mut self, dir: i32, extend: bool, word: bool) {
        self.doc.move_carets(extend, |snap, s| {
            if !extend && !s.is_empty() {
                if dir < 0 { s.start() } else { s.end() }
            } else {
                match (dir < 0, word) {
                    (true, false) => motion::prev_grapheme(snap, s.head),
                    (false, false) => motion::next_grapheme(snap, s.head),
                    (true, true) => motion::prev_word(snap, s.head),
                    (false, true) => motion::next_word(snap, s.head),
                }
            }
        });
        self.after_move();
    }

    // ---- キーボード -------------------------------------------------------

    /// Alt を押しながらのキー（WM_SYSKEYDOWN）。Alt+Shift+矢印で矩形選択する。
    fn on_syskey(&mut self, vk: VIRTUAL_KEY) -> bool {
        if !key_down(VK_SHIFT) || key_down(VK_CONTROL) {
            return false;
        }
        match vk {
            VK_LEFT => self.rect_extend(0, -1),
            VK_RIGHT => self.rect_extend(0, 1),
            VK_UP => self.rect_extend(-1, 0),
            VK_DOWN => self.rect_extend(1, 0),
            _ => return false,
        }
        true
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if vk == VK_ESCAPE && self.escape_search() {
            return true;
        }
        if self.hex.is_some() {
            return self.hex_key(vk);
        }
        if self.code.is_some() {
            return self.code_key(vk);
        }
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let alt = key_down(VK_MENU);
        let page = self.page_rows() as i64;
        // 矩形選択モードでは Shift+矢印で矩形を広げる
        if self.rect_mode && shift && !ctrl {
            let d = match vk {
                VK_LEFT => Some((0, -1)),
                VK_RIGHT => Some((0, 1)),
                VK_UP => Some((-1, 0)),
                VK_DOWN => Some((1, 0)),
                _ => None,
            };
            if let Some((dr, dc)) = d {
                self.rect_extend(dr, dc);
                return true;
            }
        }
        if self.rect.is_some() {
            match vk {
                VK_BACK => {
                    self.rect_delete(true);
                    return true;
                }
                VK_DELETE if !shift => {
                    self.rect_delete(false);
                    return true;
                }
                // 移動キーでは矩形選択をやめて、先端のカーソルから通常の移動をする
                // （文字キーは WM_CHAR で矩形に入力するので、ここでは矩形を残す）
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT
                | VK_ESCAPE => self.clear_rect(),
                _ => {}
            }
        }
        match vk {
            VK_LEFT => self.move_horizontal(-1, shift, ctrl),
            VK_RIGHT => self.move_horizontal(1, shift, ctrl),
            VK_UP if ctrl && alt => self.add_caret_vertical(-1),
            VK_DOWN if ctrl && alt => self.add_caret_vertical(1),
            VK_UP if ctrl => self.scroll_rows(-1),
            VK_DOWN if ctrl => self.scroll_rows(1),
            VK_UP => self.move_vertical(-1, shift),
            VK_DOWN => self.move_vertical(1, shift),
            VK_PRIOR | VK_NEXT => {
                let d = if vk == VK_PRIOR {
                    -(page - 1).max(1)
                } else {
                    (page - 1).max(1)
                };
                let size = self.page_rows();
                self.vp
                    .scroll_rows(self.doc.snapshot(), &self.rows_cfg, d, size);
                self.move_vertical(d, shift);
            }
            VK_HOME => {
                self.doc.move_carets(shift, |snap, s| {
                    if ctrl {
                        0
                    } else {
                        motion::smart_home(snap, s.head)
                    }
                });
                self.after_move();
            }
            VK_END => {
                self.doc.move_carets(shift, |snap, s| {
                    if ctrl {
                        snap.len()
                    } else {
                        motion::line_end(snap, s.head)
                    }
                });
                self.after_move();
            }
            VK_BACK => {
                let changed = if ctrl {
                    self.doc.delete_word_backward()
                } else {
                    self.doc.delete_backward()
                };
                if changed {
                    self.after_edit();
                }
            }
            VK_DELETE if shift => return false,
            VK_DELETE => {
                let changed = if ctrl {
                    self.doc.delete_word_forward()
                } else {
                    self.doc.delete_forward()
                };
                if changed {
                    self.after_edit();
                }
            }
            VK_INSERT if !ctrl && !shift => {
                self.overwrite = !self.overwrite;
                self.update_status();
                self.invalidate();
            }
            VK_ESCAPE => {
                let mut sels = self.doc.selections().clone();
                if sels.len() > 1 {
                    sels.collapse_to_primary();
                } else {
                    let h = sels.primary().head;
                    sels = SelectionSet::single(Selection::caret(h));
                }
                self.doc.set_selections(sels);
                self.after_move();
            }
            _ => return false,
        }
        true
    }

    fn on_char(&mut self, code: u16) {
        if self.hex.is_some() {
            self.hex_char(code);
            return;
        }
        if self.code.is_some() {
            self.code_char(code);
            return;
        }
        // Ctrl+英字などは制御文字として届くので無視する（AltGr = Ctrl+Alt は通す）
        if key_down(VK_CONTROL) && !key_down(VK_MENU) {
            return;
        }
        let text = match code {
            0x0D => {
                if self.rect.is_some() {
                    self.rect_to_carets();
                }
                if self.doc.insert_newline(true) {
                    self.after_edit();
                }
                return;
            }
            0x09 if self.csv.is_some() && self.rect.is_none() => {
                // 区切り文字モードでは Tab / Shift+Tab でセルを移動する
                self.move_cell(!key_down(VK_SHIFT));
                return;
            }
            0x09 => "\t".to_owned(),
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(code);
                return;
            }
            0xDC00..=0xDFFF => match self.high_surrogate.take() {
                Some(hi) => String::from_utf16_lossy(&[hi, code]),
                None => return,
            },
            c if c < 0x20 || c == 0x7F => return,
            c => String::from_utf16_lossy(&[c]),
        };
        if self.rect.is_some() {
            self.rect_type(&[&text], EditKind::Typing);
            return;
        }
        if self.insert_typed(&text) {
            self.after_edit();
        }
    }

    // ---- 矩形選択（09 章 3） ------------------------------------------------

    /// 矩形の先端の位置（仮想空白は行末に丸める）。
    fn rect_head_offset(&self, r: &RectSelection) -> u64 {
        rect::offset_at(
            self.doc.snapshot(),
            &self.rows_cfg,
            &self.ccfg,
            r.head_row,
            r.head_col,
        )
    }

    /// 文書の選択を矩形の先端のカーソルに合わせる（スクロール追従・IME・ステータス表示用）。
    fn sync_rect_caret(&mut self) {
        if let Some(r) = self.rect {
            let off = self.rect_head_offset(&r);
            self.doc
                .set_selections(SelectionSet::single(Selection::caret(off)));
        }
    }

    /// 矩形選択をやめて、先端の位置のカーソルにする。
    fn clear_rect(&mut self) {
        if self.rect.take().is_some() {
            self.invalidate();
        }
    }

    /// 矩形の先端を動かす（Alt+Shift+矢印）。矩形がなければ主カーソルの位置から始める。
    fn rect_extend(&mut self, drow: i64, dcol: i64) {
        let snap = self.doc.snapshot().clone();
        let mut r = self.rect.unwrap_or_else(|| {
            let (row, col) = rect::row_and_col(
                &snap,
                &self.rows_cfg,
                &self.ccfg,
                self.doc.selections().primary().head,
            );
            RectSelection {
                anchor_row: row,
                anchor_col: col,
                head_row: row,
                head_col: col,
            }
        });
        for _ in 0..drow.unsigned_abs() {
            let n = if drow > 0 {
                next_row_start(&snap, &self.rows_cfg, r.head_row)
            } else {
                prev_row_start(&snap, &self.rows_cfg, r.head_row)
            };
            match n {
                // 文書末の「改行で終わらない最終行の次」は存在しないので止まる
                Some(n) if n <= snap.len() => r.head_row = n,
                _ => break,
            }
        }
        r.head_col = (r.head_col as i64 + dcol).max(0) as u32;
        self.rect = Some(r);
        self.sync_rect_caret();
        self.after_move();
    }

    /// 矩形の全行を展開する。行数が多すぎる場合は `None`。
    fn rect_rows_all(&self) -> Option<Vec<RectRow>> {
        let r = self.rect?;
        match rect::rect_rows(
            self.doc.snapshot(),
            &self.rows_cfg,
            &self.ccfg,
            &r,
            RECT_EDIT_LIMIT,
        ) {
            Ok(rows) => Some(rows),
            Err(_) => {
                info_box(
                    self.frame,
                    &format!(
                        "矩形選択の行数が多すぎるため編集できません（上限 {} 行）。",
                        group_digits(RECT_EDIT_LIMIT as u64)
                    ),
                );
                None
            }
        }
    }

    /// 矩形に対する編集を適用し、矩形を新しい桁の縦一列カーソルにする。
    fn rect_apply(&mut self, edit: RectEdit, kind: EditKind) -> bool {
        let Some(r) = self.rect else {
            return false;
        };
        let changes: Vec<Change> = edit
            .changes
            .into_iter()
            .map(|(range, bytes)| {
                if bytes.is_empty() {
                    Change::delete(range)
                } else {
                    Change::replace_bytes(range, bytes)
                }
            })
            .collect();
        let map_row = |row: u64| {
            // 行頭はその行の変更より前にあるので、前の行の変更による移動だけを反映する
            let delta: i128 = changes
                .iter()
                .filter(|c| c.range.end <= row)
                .map(|c| c.insert_len as i128 - (c.range.end - c.range.start) as i128)
                .sum();
            (row as i128 + delta) as u64
        };
        let new_rect = RectSelection {
            anchor_row: map_row(r.anchor_row),
            head_row: map_row(r.head_row),
            anchor_col: edit.new_col,
            head_col: edit.new_col,
        };
        let edited = self.doc.apply_changes(changes, kind, |a| {
            SelectionSet::single(Selection::caret(*a.new_ends.first().unwrap_or(&0)))
        });
        self.rect = Some(new_rect);
        self.sync_rect_caret();
        if edited {
            self.after_edit();
        } else {
            self.after_move();
        }
        edited
    }

    /// 矩形の各行に文字列を入力する（行ごとに異なる文字列も可）。
    fn rect_type(&mut self, texts: &[&str], kind: EditKind) {
        if let Some(rows) = self.rect_rows_all() {
            let edit = rect::replace_rows(&rows, texts, &self.ccfg);
            self.rect_apply(edit, kind);
        }
    }

    fn rect_delete(&mut self, backward: bool) {
        let (Some(r), Some(rows)) = (self.rect, self.rect_rows_all()) else {
            return;
        };
        let edit = if backward {
            rect::delete_backward(&rows, &r)
        } else {
            rect::delete_forward(&rows, &r)
        };
        self.rect_apply(edit, EditKind::Other);
    }

    /// 矩形部分のテキスト（行ごとに改行で連結）。
    fn rect_text(&self) -> Option<String> {
        let rows = self.rect_rows_all()?;
        let eol = self.doc.eol().as_bytes();
        let mut out = Vec::new();
        for (i, t) in rect::row_texts(self.doc.snapshot(), &rows)
            .into_iter()
            .enumerate()
        {
            if i > 0 {
                out.extend_from_slice(eol);
            }
            out.extend(t);
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    /// 矩形選択を各行の選択範囲（マルチカーソル）に変換する。
    fn rect_to_carets(&mut self) {
        let Some(rows) = self.rect_rows_all() else {
            return;
        };
        let sels: Vec<Selection> = rows
            .iter()
            .map(|r| Selection::new(r.range.start, r.range.end))
            .collect();
        self.rect = None;
        if !sels.is_empty() {
            let primary = sels.len() - 1;
            self.doc
                .set_selections(SelectionSet::from_vec(sels, primary));
        }
        self.after_move();
    }

    /// 矩形データの貼り付け: 主カーソルの桁から下の行へ 1 行ずつ入れる（09 章 3.2）。
    /// 行が足りなければ文書末に行を追加する。
    fn column_paste(&mut self, lines: &[&str]) {
        let snap = self.doc.snapshot().clone();
        let head = self.doc.selections().primary().head;
        let (row, col) = rect::row_and_col(&snap, &self.rows_cfg, &self.ccfg, head);
        // 貼り付ける行数分の矩形（足りない分は後で追加する）
        let mut bottom = row;
        let mut n = 1;
        while n < lines.len() {
            match next_row_start(&snap, &self.rows_cfg, bottom) {
                Some(b) if b < snap.len() || yy_layout::is_line_start(&snap, b) => {
                    bottom = b;
                    n += 1;
                }
                _ => break,
            }
        }
        let r = RectSelection {
            anchor_row: row,
            anchor_col: col,
            head_row: bottom,
            head_col: col,
        };
        let Ok(rows) = rect::rect_rows(&snap, &self.rows_cfg, &self.ccfg, &r, RECT_EDIT_LIMIT)
        else {
            return;
        };
        let mut edit = rect::replace_rows(&rows, &lines[..rows.len()], &self.ccfg);
        // 文書末に足りない行を追加する
        if rows.len() < lines.len() {
            let eol = self.doc.eol().as_bytes();
            let mut tail = Vec::new();
            for line in &lines[rows.len()..] {
                tail.extend_from_slice(eol);
                tail.extend(std::iter::repeat_n(b' ', col as usize));
                tail.extend_from_slice(line.as_bytes());
            }
            let len = snap.len();
            match edit.changes.last_mut() {
                Some((range, bytes)) if range.end == len => bytes.extend(tail),
                _ => edit.changes.push((len..len, tail)),
            }
        }
        self.rect = Some(r);
        self.rect_apply(edit, EditKind::Paste);
        self.rect = None;
        self.after_move();
    }

    /// 矩形選択の表示情報（表示中の行だけ計算する）。
    fn rect_paints(&self, rows: &[Row]) -> Vec<RectPaint> {
        let Some(r) = self.rect else {
            return Vec::new();
        };
        let (top, bottom) = (r.top(), r.bottom());
        rows.iter()
            .filter(|row| row.start >= top && row.start <= bottom)
            .map(|row| {
                let rr = rect::rect_row(
                    self.doc.snapshot(),
                    &self.rows_cfg,
                    &self.ccfg,
                    &r,
                    row.start,
                );
                let content = yy_layout::columns::content_cols(&rr.units);
                // 行末より右は仮想空白として桁数だけ伸ばして描く
                let left = (rr.range.start, r.left().saturating_sub(content));
                let right = (rr.range.end, r.right().saturating_sub(content));
                let caret = if r.is_zero_width() {
                    Some(left)
                } else if row.start == r.head_row {
                    Some(if r.head_col == r.left() { left } else { right })
                } else {
                    None
                };
                RectPaint {
                    row_start: row.start,
                    left,
                    right,
                    caret,
                }
            })
            .collect()
    }

    // ---- マウス ----------------------------------------------------------

    /// ビューのクライアント座標（ピクセル）の表示行と、本文の左端からの x 座標（DIP）。
    fn row_at_point(&mut self, x_px: i32, y_px: i32) -> Option<(Row, f32)> {
        self.sync_renderer();
        let x = self.renderer.px_to_dip(x_px as f32);
        let y = self.renderer.px_to_dip(y_px as f32);
        let lh = self.renderer.metrics().line_height.max(1.0);
        let snap = self.doc.snapshot().clone();
        let cfg = self.rows_cfg.clone();
        let row_start = if y < 0.0 {
            prev_row_start(&snap, &cfg, self.vp.top).unwrap_or(0)
        } else {
            let idx = (y / lh).floor() as usize;
            let rows = rows_from(&snap, &cfg, self.vp.top, idx + 1);
            rows.last()?.start
        };
        let row = row_at(&snap, &cfg, row_start);
        Some((row, x - self.text_origin_x() + self.scroll_x))
    }

    /// ビューのクライアント座標（ピクセル）に最も近い文書の位置。
    fn offset_at_point(&mut self, x_px: i32, y_px: i32) -> u64 {
        match self.row_at_point(x_px, y_px) {
            Some((row, tx)) => self.renderer.hit_test(&row, tx),
            None => self.doc.snapshot().len(),
        }
    }

    /// ビューのクライアント座標（ピクセル）の表示行と表示桁（行末より右は仮想空白の桁）。
    fn row_col_at_point(&mut self, x_px: i32, y_px: i32) -> (u64, u32) {
        let Some((row, tx)) = self.row_at_point(x_px, y_px) else {
            let len = self.doc.snapshot().len();
            return rect::row_and_col(self.doc.snapshot(), &self.rows_cfg, &self.ccfg, len);
        };
        let us = yy_layout::columns::units(&row, &self.ccfg);
        let end_x = self.renderer.caret_x(&row, row.end);
        let cw = self.renderer.metrics().char_width.max(1.0);
        if tx > end_x {
            let extra = ((tx - end_x) / cw).round() as u32;
            return (row.start, yy_layout::columns::content_cols(&us) + extra);
        }
        let off = self.renderer.hit_test(&row, tx);
        (row.start, yy_layout::columns::col_of(&us, &row, off))
    }

    fn on_lbutton_down(&mut self, x: i32, y: i32, double: bool) {
        if self.hex.is_some() {
            self.hex_mouse_down(x, y);
            return;
        }
        if self.code.is_some() {
            self.code_mouse_down(x, y);
            return;
        }
        let shift = key_down(VK_SHIFT);
        let ctrl = key_down(VK_CONTROL);
        if !double && !ctrl && (key_down(VK_MENU) || self.rect_mode) {
            // Alt+ドラッグ（または矩形選択モード）で矩形選択
            let (row, col) = self.row_col_at_point(x, y);
            let r = match (shift, self.rect) {
                (true, Some(mut r)) => {
                    r.head_row = row;
                    r.head_col = col;
                    r
                }
                _ => RectSelection {
                    anchor_row: row,
                    anchor_col: col,
                    head_row: row,
                    head_col: col,
                },
            };
            self.rect = Some(r);
            self.suppress_alt_up = key_down(VK_MENU);
            self.drag = Some(Drag {
                anchor: 0,
                base: None,
                rect: true,
            });
            self.sync_rect_caret();
            self.after_move();
            return;
        }
        self.rect = None;
        let pos = self.offset_at_point(x, y);
        let mut sels = self.doc.selections().clone();
        if double {
            let r = motion::word_range(self.doc.snapshot(), pos);
            self.doc
                .set_selections(SelectionSet::single(Selection::new(r.start, r.end)));
            self.drag = None;
        } else if shift {
            let anchor = sels.primary().anchor;
            self.doc
                .set_selections(SelectionSet::single(Selection::new(anchor, pos)));
            self.drag = Some(Drag {
                anchor,
                base: None,
                rect: false,
            });
        } else if ctrl {
            let base = sels.clone();
            sels.add(Selection::caret(pos));
            self.doc.set_selections(sels);
            self.drag = Some(Drag {
                anchor: pos,
                base: Some(base),
                rect: false,
            });
        } else {
            self.doc
                .set_selections(SelectionSet::single(Selection::caret(pos)));
            self.drag = Some(Drag {
                anchor: pos,
                base: None,
                rect: false,
            });
        }
        self.after_move();
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        if self.hex.is_some() {
            self.hex_mouse_move(x, y);
            return;
        }
        if self.code.is_some() {
            self.code_mouse_move(x, y);
            return;
        }
        let Some(drag) = &self.drag else {
            return;
        };
        let (anchor, base, is_rect) = (drag.anchor, drag.base.clone(), drag.rect);
        // ビューの外に出たら 1 行ずつスクロールする
        let h = self.view_px.1 as i32;
        if y < 0 {
            self.scroll_rows(-1);
        } else if y > h {
            self.scroll_rows(1);
        }
        if is_rect {
            let (row, col) = self.row_col_at_point(x, y);
            if let Some(mut r) = self.rect
                && (r.head_row != row || r.head_col != col)
            {
                r.head_row = row;
                r.head_col = col;
                self.rect = Some(r);
                self.sync_rect_caret();
                self.after_move();
            }
            return;
        }
        let pos = self.offset_at_point(x, y);
        let sel = Selection::new(anchor, pos);
        let sels = match base {
            Some(mut b) => {
                b.add(sel);
                b
            }
            None => SelectionSet::single(sel),
        };
        if &sels != self.doc.selections() {
            self.doc.set_selections(sels);
            self.after_move();
        }
    }

    // ---- IME -------------------------------------------------------------

    /// 変換ウィンドウ・候補ウィンドウを主キャレットの位置に合わせる。
    fn update_ime_position(&mut self) {
        if self.hex.is_some() {
            return;
        }
        if self.code.is_some() {
            self.code_ime_position();
            return;
        }
        self.sync_renderer();
        let head = self.doc.selections().primary().head;
        let page = self.page_rows();
        let rows = self.cached_rows(self.vp.top, page + 1);
        let Some(i) = rows.iter().position(|r| r.shows_caret(head)) else {
            return;
        };
        let lh = self.renderer.metrics().line_height;
        let x = self.text_origin_x() + self.renderer.caret_x(&rows[i], head) - self.scroll_x;
        let y = i as f32 * lh;
        let scale = |v: f32| (v * self.renderer.px_to_dip(1.0).recip()).round() as i32;
        ime::set_position(self.view, scale(x), scale(y), scale(lh));
    }

    fn on_composition(&mut self, update: ime::CompositionUpdate) {
        let mut edited = false;
        if self.code.is_some() {
            // コード値表示では変換中の文字列は表示せず、確定した文字列を入力する
            self.composition = None;
            if let Some(result) = update.result {
                self.code_ime_result(&result);
            }
            return;
        }
        if let Some(result) = update.result {
            self.composition = None;
            if self.rect.is_some() {
                self.rect_type(&[&result], EditKind::Typing);
            } else {
                edited |= self.insert_typed(&result);
            }
        }
        if let Some((text, cursor)) = update.composing {
            if text.is_empty() {
                self.composition = None;
            } else {
                // 変換を始めたときに選択範囲があれば削除する
                if self.composition.is_none() && !self.doc.selections().all_empty() {
                    edited |= self.doc.delete_selection(EditKind::Other);
                }
                // 変換中の文字列は主カーソルを先頭に、すべてのカーソル位置にプレビューする
                let sels = self.doc.selections();
                let mut offsets = vec![sels.primary().head];
                offsets.extend(
                    sels.iter()
                        .map(|s| s.head)
                        .filter(|&h| h != sels.primary().head),
                );
                self.composition = Some(Composition {
                    offsets,
                    text,
                    cursor,
                });
            }
        }
        if edited {
            self.after_edit();
        } else {
            self.after_move();
        }
    }

    // ---- 描画 ------------------------------------------------------------

    fn paint(&mut self) {
        if self.hex.is_some() {
            self.hex_paint();
            return;
        }
        if self.code.is_some() {
            self.code_paint();
            return;
        }
        // 区切り文字モード: 表示する行の列幅を先に測る（広がったら表示を作り直す）
        self.measure_visible();
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(self.view, &mut ps);
            let page = self.page_rows();
            let rows = self.cached_rows(self.vp.top, page + 1);
            let tokens = self.visible_tokens(&rows);
            let brackets = self.caret_brackets();
            let snap = self.doc.snapshot();
            let first = snap.line_of_offset(self.vp.top);
            // 表示範囲に関係する選択・キャレットだけを渡す
            let (lo, hi) = (
                self.vp.top,
                rows.last().map(|r| r.next).unwrap_or(self.vp.top),
            );
            let sels = self.doc.selections();
            let selections: Vec<_> = sels
                .iter()
                .filter(|s| !s.is_empty() && s.end() >= lo && s.start() <= hi)
                .map(|s| s.range())
                .collect();
            // 矩形選択中は矩形側でキャレットを描く
            let carets: Vec<u64> = if self.rect.is_some() {
                Vec::new()
            } else {
                sels.iter()
                    .map(|s| s.head)
                    .filter(|h| (lo..=hi).contains(h))
                    .collect()
            };
            let rect_paints = self.rect_paints(&rows);
            let matches = match (&self.searcher, self.findbar.visible) {
                (Some(s), true) => s.matches_in(snap, lo.saturating_sub(4096)..hi, HIGHLIGHT_LIMIT),
                _ => Vec::new(),
            };
            let frame = Frame {
                version: self.paint_version(),
                rows: &rows,
                first_line: first.line,
                line_exact: first.exact,
                line_digits: self.line_digits(),
                show_line_numbers: self.show_line_numbers,
                scroll_x: self.scroll_x,
                selections: &selections,
                matches: &matches,
                carets: &carets,
                caret_visible: self.focused && self.caret_visible,
                overwrite: self.overwrite,
                composition: self.composition.as_ref(),
                rect: &rect_paints,
                tokens: &tokens,
                brackets: &brackets,
            };
            let before = self.renderer.max_text_width;
            let result = self
                .renderer
                .draw(self.view, self.view_px.0, self.view_px.1, &frame);
            let _ = EndPaint(self.view, &ps);
            match result {
                Ok(true) => {}
                // 描画ターゲットを作り直したので描き直す
                Ok(false) => self.invalidate(),
                Err(e) => eprintln!("draw failed: {}", crate::util::describe_error(&e)),
            }
            if self.renderer.max_text_width != before {
                // 描画後にスクロールバーを更新すると WM_SIZE が再入し得るため後回しにする
                let _ = PostMessageW(Some(self.view), WM_APP_SCROLLBARS, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn resize_view(&mut self, w: u32, h: u32) {
        self.view_px = (w, h);
        self.renderer.resize(w, h);
        let page = self.page_rows();
        self.vp.clamp(self.doc.snapshot(), &self.rows_cfg, page);
        self.update_scrollbars();
        self.invalidate();
    }

    // ---- 文書の切り替え・保存 -----------------------------------------------

    pub(crate) fn set_document(&mut self, doc: Document) {
        ime::cancel(self.view);
        self.composition = None;
        self.drag = None;
        self.rect = None;
        self.doc = doc;
        // 新しい文書も版は 0 から始まるので、前の文書の表示行を捨てる
        self.row_cache.borrow_mut().rows.clear();
        self.warned_noncanonical = false;
        self.find_job = None;
        self.select_job = None;
        self.count_job = None;
        self.match_count = None;
        self.status_msg.clear();
        self.csv = None;
        self.pending_record_op = None;
        self.pending_dedup = false;
        self.vp = Viewport::default();
        self.rebuild_cells();
        self.update_csv_menu();
        self.csv_mode_for_path();
        self.syntax = None;
        self.syntax_off = false;
        self.bracket_cache = None;
        self.hex = None;
        self.code = None;
        self.update_hex_menu();
        self.syntax_for_path();
        let n = self.notifier();
        self.doc.start_indexing(&self.pool, n);
        self.vp = Viewport::default();
        self.scroll_x = 0.0;
        self.renderer.clear_cache();
        self.update_title();
        self.after_move();
        self.refresh_preview();
    }

    fn take_active_tab(&mut self) -> TabState {
        TabState {
            doc: std::mem::replace(&mut self.doc, Document::new_empty()),
            vp: self.vp,
            scroll_x: self.scroll_x,
            scroll_mode: self.scroll_mode,
            rect: self.rect.take(),
            csv: self.csv.take(),
            pending_record_op: self.pending_record_op.take(),
            pending_dedup: std::mem::take(&mut self.pending_dedup),
            save_flow: self.save_flow.take(),
            save_after_convert: self.save_after_convert.take(),
            warned_noncanonical: self.warned_noncanonical,
            status_msg: std::mem::take(&mut self.status_msg),
            syntax: self.syntax.take(),
            syntax_off: self.syntax_off,
            hex: self.hex.take(),
            code: self.code.take(),
        }
    }

    fn restore_tab(&mut self, state: TabState) {
        ime::cancel(self.view);
        self.composition = None;
        self.drag = None;
        self.doc = state.doc;
        self.vp = state.vp;
        self.scroll_x = state.scroll_x;
        self.scroll_mode = state.scroll_mode;
        self.rect = state.rect;
        self.csv = state.csv;
        self.pending_record_op = state.pending_record_op;
        self.pending_dedup = state.pending_dedup;
        self.save_flow = state.save_flow;
        self.save_after_convert = state.save_after_convert;
        self.warned_noncanonical = state.warned_noncanonical;
        self.status_msg = state.status_msg;
        self.syntax = state.syntax;
        self.syntax_off = state.syntax_off;
        self.hex = state.hex;
        self.code = state.code;
        self.update_hex_menu();
        self.bracket_cache = None;
        self.find_job = None;
        self.select_job = None;
        self.count_job = None;
        self.match_count = None;
        self.row_cache.borrow_mut().rows.clear();
        self.rebuild_cells();
        self.update_csv_menu();
        self.update_syntax_menu();
        self.renderer.clear_cache();
        self.update_title();
        self.after_move();
        // 非表示中に完了した行数カウント・変換を反映する。
        let _ = self.on_index_progress();
        if self.doc.is_saving() {
            // 非表示中に終わった保存はフレームで処理する
            unsafe {
                let _ = PostMessageW(Some(self.frame), WM_APP_INDEX, WPARAM(0), LPARAM(0));
            }
        }
        self.refresh_preview();
    }

    fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        let next = self.tabs[index].take().expect("inactive tab");
        let old = self.take_active_tab();
        self.tabs[self.active_tab] = Some(old);
        self.active_tab = index;
        self.restore_tab(next);
    }

    pub(crate) fn add_document(&mut self, doc: Document) {
        if self.tabs.len() == 1
            && self.doc.path().is_none()
            && !self.doc.is_modified()
            && !self.doc.is_saving()
            && self.doc.snapshot().is_empty()
        {
            self.set_document(doc);
            return;
        }
        let index = self.tabs.len();
        self.tabs.push(Some(TabState::new(doc)));
        self.switch_tab(index);
        let n = self.notifier();
        self.doc.start_indexing(&self.pool, n);
        self.csv_mode_for_path();
        self.syntax_for_path();
        self.update_title();
    }

    fn new_tab(&mut self) {
        let index = self.tabs.len();
        self.tabs.push(Some(TabState::new(Document::new_empty())));
        self.switch_tab(index);
    }

    fn close_tab(&mut self) {
        self.tabs.remove(self.active_tab);
        if self.tabs.is_empty() {
            self.tabs.push(None);
            self.active_tab = 0;
            self.set_document(Document::new_empty());
        } else {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
            let state = self.tabs[self.active_tab].take().expect("inactive tab");
            self.restore_tab(state);
        }
    }

    /// ステータスバーに案内を出す（空なら消す）。
    pub(crate) fn show_status_message(&mut self, text: &str) {
        self.status_msg = text.to_owned();
        self.update_status();
    }

    /// `location`（手元のパスまたは `ssh://…`）を編集用に開いているタブがあれば表に出す。
    pub(crate) fn focus_location(&mut self, location: &std::path::Path) -> bool {
        self.focus_open(location, false)
    }

    fn focus_open(&mut self, location: &std::path::Path, read_only: bool) -> bool {
        let same = |d: &Document| {
            d.is_read_only() == read_only
                && d.location()
                    .is_some_and(|l| yy_config::recent::same_path(&l, location))
        };
        if same(&self.doc) {
            return true;
        }
        if let Some(index) = self
            .tabs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|t| same(&t.doc)))
        {
            self.switch_tab(index);
            return true;
        }
        false
    }

    /// ファイルを開くときの指定。文字コードの指定がなければファイル種類の設定（拡張子）の文字コード。
    pub(crate) fn open_options(
        &self,
        location: &std::path::Path,
        encoding: Option<Encoding>,
        raw: bool,
    ) -> OpenOptions {
        let configured = encoding.or_else(|| {
            let ext = location.extension()?.to_string_lossy().into_owned();
            let (_, ft) = self.config.filetype_for_extension(&ext)?;
            Encoding::from_name(ft.encoding.as_deref()?)
        });
        OpenOptions {
            encoding: configured.filter(|_| !raw),
            detect_ebcdic: self.config.editor.detect_ebcdic,
            raw,
            ..OpenOptions::default()
        }
    }

    fn open(
        &mut self,
        path: PathBuf,
        encoding: Option<Encoding>,
        shared_read_only: bool,
    ) -> std::result::Result<(), String> {
        let opts = self.open_options(&path, encoding, false);
        // 既に開いているパスなら、そのタブへ移動する（明示的な開き直しは別処理）。
        if encoding.is_none() && self.focus_open(&path, shared_read_only) {
            return Ok(());
        }
        let doc = if shared_read_only {
            Document::open_shared_read_only(&path, &opts)
        } else {
            Document::open_with(&path, &opts)
        }
        .map_err(|e| format!("{}\n\n{e}", path.display()))?;
        crate::recentdlg::remember(&path);
        self.add_document(doc);
        Ok(())
    }

    /// ファイルをバイト列のまま開き直す（16 進数表示用）。
    fn reopen_raw(&mut self, path: PathBuf) -> std::result::Result<(), String> {
        let opts = OpenOptions {
            raw: true,
            ..OpenOptions::default()
        };
        let doc = if self.doc.is_read_only() {
            Document::open_shared_read_only(&path, &opts)
        } else {
            Document::open_with(&path, &opts)
        }
        .map_err(|e| format!("{}\n\n{e}", path.display()))?;
        self.set_document(doc);
        Ok(())
    }

    /// ファイルをバイト列のまま新しいタブで開き、16 進数表示にする。
    fn open_raw(&mut self, path: PathBuf) -> std::result::Result<(), String> {
        let opts = OpenOptions {
            raw: true,
            ..OpenOptions::default()
        };
        let doc =
            Document::open_with(&path, &opts).map_err(|e| format!("{}\n\n{e}", path.display()))?;
        crate::recentdlg::remember(&path);
        self.add_document(doc);
        self.set_hex(true, None);
        Ok(())
    }

    /// 「16 進数表示」のチェックを今の表示に合わせる。
    fn update_hex_menu(&self) {
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let fixed = self.hex.is_some_and(|h| h.record.is_some());
        let flag = check(self.hex.is_some() && !fixed);
        let record = check(fixed);
        let code = if self.code.is_some() {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        unsafe {
            CheckMenuItem(self.menu_view, ID_HEX_MODE as u32, (MF_BYCOMMAND | flag).0);
            CheckMenuItem(self.menu_view, ID_CODE_MODE as u32, (MF_BYCOMMAND | code).0);
            CheckMenuItem(
                self.menu_view,
                ID_RECORD_MODE as u32,
                (MF_BYCOMMAND | record).0,
            );
            CheckMenuItem(
                self.menu_view,
                ID_RECORD_MODE as u32,
                (MF_BYCOMMAND | record).0,
            );
        }
        self.update_charset_menu();
    }

    fn reopen(&mut self, path: PathBuf, encoding: Encoding) -> std::result::Result<(), String> {
        let opts = OpenOptions {
            encoding: Some(encoding),
            ..OpenOptions::default()
        };
        let doc = if self.doc.is_read_only() {
            Document::open_shared_read_only(&path, &opts)
        } else {
            Document::open_with(&path, &opts)
        }
        .map_err(|e| format!("{}\n\n{e}", path.display()))?;
        self.set_document(doc);
        Ok(())
    }

    /// バックグラウンドで保存を始める（保存中も編集できる）。終わるとフレームが
    /// [`App::poll_save`] で結果を受け取り、[`on_save_done`] で処理する。
    fn start_save(&mut self, flow: SaveFlow) -> std::result::Result<(), String> {
        let n = self.notifier();
        let t = &flow.target;
        match &t.remote {
            Some(r) => {
                self.doc
                    .start_save_remote(t.encoding, t.bom, &self.pool, n, r.file(), r.upload())
            }
            None => self
                .doc
                .start_save(&t.path, t.encoding, t.bom, &self.pool, n),
        }
        .map_err(|e| format!("{}\n\n{e}", t.path.display()))?;
        self.save_flow = Some(flow);
        self.status_msg.clear();
        self.update_status();
        unsafe {
            SetTimer(Some(self.view), TIMER_PROGRESS, 200, None);
        }
        Ok(())
    }

    /// 終わった保存を受け取って [`App::save_done`] に置く（作業中のタブ）。結果の処理（エラーの
    /// 表示など）はフレームで行う。
    fn poll_save(&mut self) {
        let Some(done) = self.doc.poll_save() else {
            return;
        };
        if done.result.is_ok() {
            let n = self.notifier();
            self.doc.maintain_indexing(&self.pool, n);
            self.renderer.clear_cache();
            // 拡張子が変わっていればハイライトの定義も選び直す
            self.syntax_for_path();
            self.update_title();
            self.after_move();
            self.refresh_preview();
        }
        self.save_done = Some((self.save_flow.take(), done));
    }

    /// 裏のタブの終わった保存を受け取る。保存できなかったタブは表に出して結果を処理する。
    fn poll_inactive_saves(&mut self) {
        if self.defer_inactive_saves > 0 {
            return;
        }
        let mut changed = false;
        for i in 0..self.tabs.len() {
            let Some(tab) = self.tabs[i].as_mut() else {
                continue;
            };
            let Some(done) = tab.doc.poll_save() else {
                continue;
            };
            changed = true;
            let flow = tab.save_flow.take();
            if done.result.is_err() {
                self.switch_tab(i);
                self.save_done = Some((flow, done));
                return;
            }
        }
        if changed {
            self.refresh_tabs();
        }
    }

    /// 行数カウント・文字コード変換の進捗を反映する。変換に失敗したらそのエラーを返す。
    fn on_index_progress(&mut self) -> Option<String> {
        self.index_posted.store(false, Ordering::Release);
        self.poll_search_jobs();
        if self.csv.is_some() {
            self.poll_csv();
        }
        self.poll_syntax();
        let was_replacing = self.doc.replace_progress().is_some();
        let was_loading = self.doc.is_loading();
        let mut error = None;
        if self.doc.poll_indexing() {
            if was_replacing
                && self.doc.replace_progress().is_none()
                && let Some(flow) = self.save_after_convert.take()
            {
                // 保存の前の改行コードの変換が終わった
                self.after_edit();
                match self.doc.take_replace_result() {
                    Some(Ok(_)) => {
                        if let Err(msg) = self.start_save(flow) {
                            error = Some(format!("保存できませんでした。\n{msg}"));
                        }
                    }
                    Some(Err(e)) => {
                        self.status_msg = format!("改行コードを変換できませんでした: {e}");
                        self.update_status();
                    }
                    None => {}
                }
            } else if was_replacing && self.doc.replace_progress().is_none() {
                let op = self.pending_record_op.take();
                match (self.doc.take_replace_result(), op) {
                    (Some(Ok(n)), Some(op)) => {
                        self.status_msg = format!("{} レコードを書き換えました", group_digits(n));
                        self.after_record_op(op);
                    }
                    (Some(Ok(n)), None) if std::mem::take(&mut self.pending_dedup) => {
                        self.status_msg = format!("重複する {} 行を削除しました", group_digits(n));
                    }
                    (Some(Ok(n)), None) => {
                        self.status_msg = format!("{} 個置換しました", group_digits(n));
                    }
                    (Some(Err(e)), _) => self.status_msg = format!("処理できませんでした: {e}"),
                    (None, _) => {}
                }
                self.after_edit();
            }
            if was_loading && !self.doc.is_loading() {
                // 変換が終わって内容を差し替えた
                self.sync_csv();
                self.syntax_for_path();
                self.sync_syntax();
                self.renderer.clear_cache();
                self.update_title();
                if let Some(e) = self.doc.load_error() {
                    let msg = format!("ファイルを読み込めませんでした。\n{e}");
                    self.update_status();
                    self.invalidate();
                    return Some(msg);
                }
            }
            if self.doc.indexing_progress().is_none() && !self.doc.snapshot().is_fully_indexed() {
                // 編集で分割されたピースなど、数え残しがあれば続けて数える
                let n = self.notifier();
                self.doc.maintain_indexing(&self.pool, n);
            }
            self.update_scrollbars();
            self.update_status();
            self.invalidate();
        }
        error
    }

    // ---- 検索・置換 --------------------------------------------------------

    /// 検索バーを開く。1 行の短い選択があれば検索文字列にする。
    fn open_findbar(&mut self, replace_mode: bool) {
        let sel = *self.doc.selections().primary();
        let initial = (!sel.is_empty() && sel.end() - sel.start() <= 256)
            .then(|| self.doc.snapshot().read(sel.range()))
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|t| !t.contains('\n'));
        self.findbar.show(replace_mode, initial.as_deref());
        self.layout_children();
        self.compile_search();
        self.invalidate();
    }

    fn close_findbar(&mut self) {
        self.find_job = None;
        self.count_job = None;
        self.findbar.hide();
        self.status_msg.clear();
        self.layout_children();
        self.update_status();
        self.invalidate();
        unsafe {
            let _ = SetFocus(Some(self.view));
        }
    }

    /// Esc: 実行中の検索・置換を中止するか、検索バーを閉じる。何かしたら `true`。
    fn escape_search(&mut self) -> bool {
        if self.find_job.take().is_some()
            | self.select_job.take().is_some()
            | self.grep_job.take().is_some()
        {
            self.status_msg = "検索を中止しました".into();
            self.update_status();
            return true;
        }
        if self.doc.replace_progress().is_some() {
            self.doc.cancel_replace();
            return true;
        }
        if self.doc.is_saving() {
            self.doc.cancel_save();
            return true;
        }
        if self.findbar.visible {
            self.close_findbar();
            return true;
        }
        false
    }

    /// 検索バーの条件をコンパイルする。誤りがあればステータスバーに表示して `false`。
    fn compile_search(&mut self) -> bool {
        let q = self.findbar.query();
        self.select_job = None;
        self.match_count = None;
        self.count_job = None;
        if q.pattern.is_empty() {
            self.searcher = None;
            self.status_msg.clear();
            self.update_status();
            return false;
        }
        match Searcher::new(&q) {
            Ok(s) => {
                self.searcher = Some(Arc::new(s));
                self.status_msg.clear();
                self.update_status();
                true
            }
            Err(e) => {
                self.searcher = None;
                self.status_msg = e.to_string().replace('\n', " ");
                self.update_status();
                false
            }
        }
    }

    /// 見つけた範囲を選択して表示する。
    fn select_match(&mut self, m: std::ops::Range<u64>, wrapped: bool) {
        self.rect = None;
        self.doc
            .set_selections(SelectionSet::single(Selection::new(m.start, m.end)));
        self.scroll_to_offset(m.start);
        self.status_msg = if wrapped {
            "文書の端を越えて検索しました".into()
        } else {
            String::new()
        };
        self.after_move();
    }

    fn not_found(&mut self) {
        self.status_msg = "見つかりません".into();
        self.update_status();
    }

    /// 次（`forward`）・前を検索する。大きな文書はバックグラウンドで探す。
    fn find(&mut self, forward: bool) {
        if self.searcher.is_none() && !self.compile_search() {
            if !self.findbar.visible {
                self.open_findbar(false);
            }
            return;
        }
        let Some(s) = self.searcher.clone() else {
            return;
        };
        self.find_job = None;
        let snap = self.doc.snapshot().clone();
        let sel = *self.doc.selections().primary();
        let from = if forward {
            // 空の一致（^ など）が同じ位置で見つかり続けないように 1 文字進める
            if sel.is_empty() && self.last_found_empty(&s, sel.head) {
                motion::next_grapheme(&snap, sel.head)
            } else {
                sel.end()
            }
        } else {
            sel.start()
        };
        self.start_count(&s);
        if snap.len() <= SYNC_SEARCH_BYTES {
            match s.find_wrapping(&snap, from, forward, &mut |_| true) {
                Ok(Some((m, wrapped))) => self.select_match(m, wrapped),
                _ => self.not_found(),
            }
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let notify = self.notifier();
        let job = self.pool.spawn(move |ctx| {
            ctx.progress.set_total(snap.len());
            let mut done = 0u64;
            let r = s.find_wrapping(&snap, from, forward, &mut |pos| {
                done = done.max(pos.abs_diff(from));
                ctx.progress.set_done(done);
                !ctx.cancel.is_cancelled()
            });
            if let Ok(r) = r {
                let _ = tx.send(r);
                notify();
            }
        });
        self.find_job = Some(FindJob {
            job,
            rx,
            version: self.doc.version(),
        });
        self.update_status();
    }

    /// 検索バーの条件に一致する文字列を複数選択にする。通常のコピーで改行区切りにできる。
    fn select_search_matches(&mut self) -> (usize, bool) {
        if !self.compile_search() {
            return (0, false);
        }
        let Some(searcher) = self.searcher.clone() else {
            return (0, false);
        };
        let snap = self.doc.snapshot().clone();
        if snap.len() > SYNC_SEARCH_BYTES {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let notify = self.notifier();
            let version = self.doc.version();
            let job = self.pool.spawn(move |ctx| {
                let len = snap.len();
                ctx.progress.set_total(len);
                let result =
                    searcher.matches_cancellable(&snap, 0..len, CARET_LIMIT + 1, &mut |pos| {
                        ctx.progress.set_done(pos);
                        !ctx.cancel.is_cancelled()
                    });
                if let Ok(matches) = result {
                    let _ = tx.send(matches);
                    notify();
                }
            });
            self.select_job = Some(SelectJob { job, rx, version });
            self.update_status();
            return (0, false);
        }
        let matches = searcher.matches_in(&snap, 0..snap.len(), CARET_LIMIT + 1);
        self.apply_search_matches(matches)
    }

    fn apply_search_matches(&mut self, matches: Vec<std::ops::Range<u64>>) -> (usize, bool) {
        let truncated = matches.len() > CARET_LIMIT;
        let sels: Vec<_> = matches
            .into_iter()
            .filter(|m| !m.is_empty())
            .take(CARET_LIMIT)
            .map(|m| Selection::new(m.start, m.end))
            .collect();
        let count = sels.len();
        if count == 0 {
            self.not_found();
            return (0, false);
        }
        let start = sels[0].start();
        self.rect = None;
        self.doc.set_selections(SelectionSet::from_vec(sels, 0));
        self.scroll_to_offset(start);
        self.status_msg = if truncated {
            format!(
                "先頭から {} 箇所を選択しました（上限）",
                group_digits(count as u64)
            )
        } else {
            format!("{} 箇所を選択しました", group_digits(count as u64))
        };
        self.after_move();
        (count, truncated)
    }

    /// カーソル位置に空の一致があるか（そこで止まり続けないようにするため）。
    fn last_found_empty(&self, s: &Searcher, at: u64) -> bool {
        let snap = self.doc.snapshot();
        let end = (at + s.max_match_len()).min(snap.len());
        matches!(
            s.find_next(snap, at..end, at, &mut |_| true),
            Ok(Some(m)) if m.start == at && m.is_empty()
        )
    }

    /// 入力中の検索文字列で、選択の先頭から近くを探す（インクリメンタル検索）。
    fn find_incremental(&mut self) {
        if !self.compile_search() {
            self.invalidate();
            return;
        }
        let Some(s) = self.searcher.clone() else {
            return;
        };
        let snap = self.doc.snapshot().clone();
        let from = self.doc.selections().primary().start();
        let end = (from + INCREMENTAL_BYTES).min(snap.len());
        match s.find_next(&snap, 0..end, from, &mut |_| true) {
            Ok(Some(m)) => {
                self.rect = None;
                self.doc
                    .set_selections(SelectionSet::single(Selection::new(m.start, m.end)));
                self.scroll_to_offset(m.start);
                self.after_move();
            }
            _ => self.invalidate(),
        }
    }

    /// 件数をバックグラウンドで数える（同じ条件・同じ内容なら数え直さない）。
    fn start_count(&mut self, s: &Arc<Searcher>) {
        let version = self.doc.version();
        if self.match_count.is_some_and(|(_, v)| v == version)
            || self
                .count_job
                .as_ref()
                .is_some_and(|j| j.version == version)
        {
            return;
        }
        let snap = self.doc.snapshot().clone();
        let s = s.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let notify = self.notifier();
        let job = self.pool.spawn(move |ctx| {
            let len = snap.len();
            if let Ok(n) = s.count(&snap, 0..len, COUNT_LIMIT, &mut |_| {
                !ctx.cancel.is_cancelled()
            }) {
                let _ = tx.send(n);
                notify();
            }
        });
        self.count_job = Some(CountJob { job, rx, version });
    }

    /// バックグラウンドの検索・件数の結果を反映する。
    fn poll_search_jobs(&mut self) {
        if let Some(j) = &self.find_job
            && let Ok(r) = j.rx.try_recv()
        {
            let current = j.version == self.doc.version();
            self.find_job = None;
            match r {
                Some((m, wrapped)) if current => self.select_match(m, wrapped),
                Some(_) => {}
                None => self.not_found(),
            }
        }
        if let Some(j) = &self.select_job
            && let Ok(matches) = j.rx.try_recv()
        {
            let current = j.version == self.doc.version();
            self.select_job = None;
            if current {
                self.apply_search_matches(matches);
            }
        }
        if let Some(j) = &self.count_job
            && let Ok(n) = j.rx.try_recv()
        {
            self.match_count = Some((n, j.version));
            self.count_job = None;
            self.update_status();
        }
        if let Some(j) = &self.grep_job
            && let Ok(text) = j.rx.try_recv()
        {
            self.grep_job = None;
            self.grep_done = Some(text);
        }
        if self.find_job.is_some() || self.select_job.is_some() || self.grep_job.is_some() {
            self.update_status();
        }
    }

    /// 置換文字列（正規表現なら `$1` などを解釈する）。
    fn replacement(&mut self, s: &Searcher) -> Option<Replacement> {
        let text = self.findbar.replacement_text();
        if !self.findbar.query().regex {
            return Some(Replacement::literal(&text));
        }
        match Replacement::parse(&text, s) {
            Ok(r) => Some(r),
            Err(e) => {
                self.status_msg = e.to_string();
                self.update_status();
                None
            }
        }
    }

    /// 選択が一致していれば置き換えて、次を検索する。
    fn replace_one(&mut self) {
        if self.searcher.is_none() && !self.compile_search() {
            return;
        }
        let Some(s) = self.searcher.clone() else {
            return;
        };
        let Some(r) = self.replacement(&s) else {
            return;
        };
        if self.doc.replace_selection(&s, &r) {
            self.after_edit();
        }
        self.find(true);
    }

    /// すべて置換する（「選択範囲のみ置換」なら主選択の範囲内）。
    fn replace_all(&mut self) {
        if self.searcher.is_none() && !self.compile_search() {
            return;
        }
        let Some(s) = self.searcher.clone() else {
            return;
        };
        let Some(r) = self.replacement(&s) else {
            return;
        };
        let sel = *self.doc.selections().primary();
        let range = if self.findbar.selection_only() && !sel.is_empty() {
            sel.range()
        } else {
            0..self.doc.snapshot().len()
        };
        self.rect = None;
        let notify = self.notifier();
        let result = unsafe {
            let old = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
            let r = self.doc.replace_all(s, r, range, &self.pool, notify);
            SetCursor(Some(old));
            r
        };
        match result {
            Ok(Some(n)) => {
                self.status_msg = if n == 0 {
                    "見つかりません".into()
                } else {
                    format!("{} 個置換しました", group_digits(n))
                };
                self.after_edit();
            }
            Ok(None) => self.update_status(),
            Err(e) => {
                self.status_msg = format!("置換できませんでした: {e}");
                self.update_status();
            }
        }
    }

    /// Grep ダイアログの初期値（検索バーの文字列・現在のファイルのフォルダ）。
    fn grep_defaults(&self) -> crate::grepdlg::GrepRequest {
        let mut r = self.grep_last.clone();
        let q = self.findbar.query();
        if !q.pattern.is_empty() {
            r.pattern = q.pattern;
            r.regex = q.regex;
            r.case_sensitive = q.case_sensitive;
            r.whole_word = q.whole_word;
        }
        if r.dir.is_empty()
            && self.doc.remote().is_none()
            && let Some(dir) = self.doc.path().and_then(|p| p.parent())
        {
            r.dir = dir.display().to_string();
        }
        r
    }

    /// Grep をバックグラウンドで始める。
    fn start_grep(&mut self, req: crate::grepdlg::GrepRequest) {
        let q = yy_core::Query {
            pattern: req.pattern.clone(),
            regex: req.regex,
            case_sensitive: req.case_sensitive,
            whole_word: req.whole_word,
        };
        let searcher = match Searcher::new(&q) {
            Ok(s) => s,
            Err(e) => {
                self.status_msg = e.to_string().replace('\n', " ");
                self.update_status();
                return;
            }
        };
        let opts = yy_core::grep::GrepOptions {
            dir: PathBuf::from(req.dir.trim()),
            files: req.files.clone(),
            recursive: req.recursive,
        };
        self.grep_last = req.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let notify = self.notifier();
        let job = self.pool.spawn(move |ctx| {
            const MAX_HITS: u64 = 100_000;
            let mut out = String::new();
            let mut n = 0u64;
            let r = yy_core::grep::grep(
                &searcher,
                &opts,
                &mut |_| {
                    ctx.progress.add_done(1);
                    !ctx.cancel.is_cancelled()
                },
                &mut |h| {
                    out.push_str(&yy_core::grep::format_hit(&h));
                    out.push_str("\r\n");
                    n += 1;
                    n < MAX_HITS
                },
            );
            if ctx.cancel.is_cancelled() {
                return;
            }
            let summary = match r {
                Ok(st) => {
                    let mut s = format!(
                        "検索: {}  フォルダ: {}  ファイル: {}\r\n{} 件（{} ファイル中 {} ファイル）",
                        req.pattern,
                        opts.dir.display(),
                        req.files,
                        n,
                        st.files,
                        st.matched_files
                    );
                    if n >= MAX_HITS {
                        s += "  ※ 件数が多いため途中までです";
                    }
                    if st.skipped > 0 {
                        s += &format!("  バイナリなどで飛ばしたファイル: {}", st.skipped);
                    }
                    s
                }
                Err(e) => format!("Grep できませんでした: {}: {e}", opts.dir.display()),
            };
            let _ = tx.send(format!(
                "{summary}\r\n（行を選んで F12 でファイルを開きます）\r\n\r\n{out}"
            ));
            notify();
        });
        self.grep_job = Some(GrepJob { job, rx });
        self.update_status();
    }

    /// カーソル行の「パス(行番号)」のファイルを開く。別のファイルは新しいウィンドウで開く。
    fn tag_jump(&mut self) -> Option<String> {
        let snap = self.doc.snapshot();
        let head = self.doc.selections().primary().head;
        let start = motion::line_start(snap, head);
        let end = motion::line_end(snap, start).min(start + 8192);
        let line = String::from_utf8_lossy(&snap.read(start..end)).into_owned();
        let Some((path, n)) = yy_core::grep::parse_tag_line(&line) else {
            return Some("この行にはファイル名と行番号がありません。".into());
        };
        if self.doc.remote().is_none() && self.doc.path().is_some_and(|p| p == path) {
            self.goto_line(n);
            return None;
        }
        if !path.is_file() {
            return Some(format!("ファイルが見つかりません。\n{}", path.display()));
        }
        match self.open(path, None, false) {
            Ok(()) => {
                self.goto_line(n);
                None
            }
            Err(e) => Some(format!("開けませんでした。\n{e}")),
        }
    }

    /// 保存できない文字の範囲を置き換える（1 回の Undo で戻せる）。
    fn replace_unmappable(&mut self, ranges: &[std::ops::Range<u64>], ncr: bool) {
        self.rect = None;
        let ok = self.doc.replace_ranges(ranges, |b| {
            let c = std::str::from_utf8(b).ok().and_then(|s| s.chars().next());
            match c {
                Some(c) if ncr && yy_encoding::unescape_char(c).is_none() => {
                    format!("&#x{:X};", c as u32).into_bytes()
                }
                _ => b"?".to_vec(),
            }
        });
        if ok {
            self.after_edit();
        }
    }

    /// 保存できない文字のうち、似た文字に置き換えられる範囲。隣り合う範囲はまとめて
    /// 置き換えられればまとめる（半角カナと濁点など）。まとめて置き換えられなければ 1 文字ずつ。
    fn foldable(
        &self,
        ranges: &[std::ops::Range<u64>],
        enc: Encoding,
    ) -> Vec<std::ops::Range<u64>> {
        let snap = self.doc.snapshot();
        let can_fold = |r: &std::ops::Range<u64>| {
            let bytes = snap.read(r.clone());
            std::str::from_utf8(&bytes).is_ok_and(|s| {
                !s.chars().any(|c| yy_encoding::unescape_char(c).is_some())
                    && yy_encoding::fold_compat(enc, s).is_some()
            })
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < ranges.len() {
            let mut j = i + 1;
            while j < ranges.len() && ranges[j].start == ranges[j - 1].end {
                j += 1;
            }
            let group = ranges[i].start..ranges[j - 1].end;
            if j - i > 1 && can_fold(&group) {
                out.push(group);
            } else {
                out.extend(ranges[i..j].iter().filter(|r| can_fold(r)).cloned());
            }
            i = j;
        }
        out
    }

    /// [`App::foldable`] の範囲を似た文字に置き換える（1 回の Undo で戻せる）。
    fn replace_folded(&mut self, ranges: &[std::ops::Range<u64>], enc: Encoding) {
        self.rect = None;
        let ok = self.doc.replace_ranges(ranges, |b| {
            let s = String::from_utf8_lossy(b);
            yy_encoding::fold_compat(enc, &s)
                .map(String::into_bytes)
                .unwrap_or_else(|| b.to_vec())
        });
        if ok {
            self.after_edit();
        }
    }

    /// 保存できない最初の文字へ移動して選択する。
    fn select_range(&mut self, r: std::ops::Range<u64>) {
        self.rect = None;
        self.doc
            .set_selections(SelectionSet::single(Selection::new(r.start, r.end)));
        self.scroll_to_offset(r.start);
        self.after_move();
    }

    /// 保存できない文字の説明（行と文字）。
    fn describe_offset(&self, r: &std::ops::Range<u64>) -> String {
        let snap = self.doc.snapshot();
        let line = snap.line_of_offset(r.start);
        let bytes = snap.read(r.clone());
        let what = match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| s.chars().next())
        {
            Some(c) => match yy_encoding::unescape_char(c) {
                Some(b) => format!("読み込み時に不正だったバイト 0x{b:02X}"),
                None => format!("「{c}」(U+{:04X})", c as u32),
            },
            None => format!("不正なバイト 0x{:02X}", bytes.first().copied().unwrap_or(0)),
        };
        let approx = if line.exact { "" } else { "約 " };
        format!("{approx}{} 行目の {what}", group_digits(line.line + 1))
    }

    /// 行へ移動。行数が未確定の範囲はその場で数える。
    fn goto_line(&mut self, line: u64) {
        let snap = self.doc.snapshot().clone();
        let lookup = match snap.line_start(line - 1, false) {
            LineLookup::NotIndexed => unsafe {
                let old = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
                let r = snap.line_start(line - 1, true);
                SetCursor(Some(old));
                r
            },
            other => other,
        };
        match lookup {
            LineLookup::Found(off) => {
                self.doc
                    .set_selections(SelectionSet::single(Selection::caret(off)));
                self.scroll_to_offset(off);
                self.after_move();
            }
            _ => info_box(
                self.frame,
                &format!("{} 行目はありません。", group_digits(line)),
            ),
        }
    }

    /// コピーする文字列と、それが矩形選択のデータか。
    fn copy_selection(&self) -> std::result::Result<Option<(String, bool)>, u64> {
        if self.rect.is_some() {
            return Ok(self.rect_text().map(|t| (t, true)));
        }
        Ok(self
            .doc
            .selected_text(MAX_CLIPBOARD_BYTES)?
            .map(|t| (t, false)))
    }

    /// 選択範囲（矩形を含む）を削除する。
    fn delete_selection(&mut self, kind: EditKind) -> bool {
        if let Some(r) = self.rect {
            if r.is_zero_width() {
                return false;
            }
            self.rect_delete(true);
            return true;
        }
        if self.doc.delete_selection(kind) {
            self.after_edit();
            true
        } else {
            false
        }
    }

    fn paste(&mut self, text: &str, column: bool) {
        let lines: Vec<&str> = text
            .strip_suffix("\r\n")
            .or(text.strip_suffix('\n'))
            .unwrap_or(text)
            .split('\n')
            .map(|l| l.trim_end_matches('\r'))
            .collect();
        if let Some(rows) = self.rect_rows_all() {
            // 矩形への貼り付け: 行数が一致すれば 1 行ずつ、1 行なら全行に同じ文字列
            if lines.len() == rows.len() || lines.len() == 1 {
                let edit = rect::replace_rows(&rows, &lines, &self.ccfg);
                self.rect_apply(edit, EditKind::Paste);
                return;
            }
            // 行数が合わなければ矩形の左上から矩形データとして貼り付ける
            let top_left = rows[0].range.start;
            self.rect = None;
            self.doc
                .set_selections(SelectionSet::single(Selection::caret(top_left)));
            self.column_paste(&lines);
            return;
        }
        if column && lines.len() > 1 && self.doc.selections().len() == 1 {
            if self.doc.delete_selection(EditKind::Paste) {
                self.after_edit();
            }
            self.column_paste(&lines);
            return;
        }
        if self.doc.paste(text) {
            self.after_edit();
        }
    }
}

// ---- コマンド（モーダル UI を伴うため状態の借用の外で実行する） ---------------

/// 変更を保存するか確認する。続行してよければ `true`。
/// タブ `index` を閉じる（閉じるボタン・中ボタンのクリック）。変更があれば確認する。
/// 別のタブを閉じた場合は、元のタブに戻る。閉じたら `true`（取り消したら `false`）。
fn close_tab_at(hwnd: HWND, index: usize) -> bool {
    let Some((active, count)) = with_app(|a| (a.active_tab, a.tabs.len())) else {
        return false;
    };
    if index >= count {
        return false;
    }
    with_app(|a| a.switch_tab(index));
    if !confirm_discard(hwnd) {
        with_app(|a| a.switch_tab(active));
        return false;
    }
    with_app(|a| {
        a.close_tab();
        if active != index {
            let back = if active > index { active - 1 } else { active };
            a.switch_tab(back);
        }
    });
    true
}

/// タブの右クリックのメニュー（閉じる・ほかのタブ・右側・左側を閉じる）。変更のある文書は
/// 1 つずつ確認し、取り消したらそこでやめる。最後に右クリックしたタブを表に出す。
fn tab_menu(hwnd: HWND, index: usize) {
    let Some(count) = with_app(|a| a.tabs.len()) else {
        return;
    };
    let Some(which) = crate::tabclose::menu(hwnd, index, count) else {
        return;
    };
    if which == crate::tabclose::TabMenu::Close {
        close_tab_at(hwnd, index);
        return;
    }
    // 右から閉じる（閉じていないタブの番号が変わらないように）
    let mut keep = index;
    for i in which.targets(index, count).into_iter().rev() {
        if !close_tab_at(hwnd, i) {
            break;
        }
        if i < keep {
            keep -= 1;
        }
    }
    with_app(|a| a.switch_tab(keep));
}

/// 作業中のタブの文書を閉じてよいか確かめる。保存中なら終わるまで待ち、変更があれば
/// 保存するか尋ねる（保存する場合は終わるまで待つ）。
fn confirm_discard(hwnd: HWND) -> bool {
    wait_save();
    let Some((modified, name)) = with_app(|a| (a.doc.is_modified(), a.doc.display_name())) else {
        return false;
    };
    if !modified {
        return true;
    }
    let r = unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(format!("「{name}」への変更を保存しますか？")),
            &HSTRING::from("yyeditor"),
            MB_YESNOCANCEL | MB_ICONWARNING,
        )
    };
    match r {
        IDYES => {
            if !cmd_save(hwnd, false) {
                return false;
            }
            wait_save();
            with_app(|a| !a.doc.is_modified()).unwrap_or(false)
        }
        IDNO => true,
        _ => false,
    }
}

/// 設定ファイル（%APPDATA%\yyeditor\config.toml）を開く。なければ既定値を書き出して作る。
fn open_settings(hwnd: HWND) {
    let Some(path) = Config::default_path() else {
        error_box(hwnd, "設定ファイルの場所（%APPDATA%）が分かりません。");
        return;
    };
    if !path.exists() {
        let created = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, Config::default_file_contents()));
        if let Err(e) = created {
            error_box(
                hwnd,
                &format!(
                    "設定ファイルを作れませんでした。\n{}\n\n{e}",
                    path.display()
                ),
            );
            return;
        }
    }
    open_path(hwnd, path, None, false);
    with_app(|a| {
        a.status_msg = "設定の変更は次に起動したときから反映されます".into();
        a.update_status();
    });
}

/// リモート接続の記録（接続のたびに追記する）を開く。
fn open_remote_log(hwnd: HWND) {
    match crate::remote::log_path() {
        Some(path) if path.exists() => open_path(hwnd, path, None, false),
        _ => info_box(
            hwnd,
            "リモート接続の記録はまだありません。SSH の接続先に接続すると記録します。",
        ),
    }
}

/// `wparam`（WM_COMMAND）が「閉じる」か（ヘルプのウィンドウで Ctrl+W を受けるため）。
pub(crate) fn is_close_command(wparam: WPARAM) -> bool {
    loword(wparam.0) as u16 == ID_CLOSE
}

/// 開いているファイルをブックマークに加える（既にあれば外す）。
fn cmd_toggle_bookmark(hwnd: HWND) {
    use crate::recentdlg::ListKind;
    let Some(Some(path)) = with_app(|a| a.doc.location().map(|p| crate::recentdlg::normalize(&p)))
    else {
        info_box(hwnd, "保存してからブックマークしてください。");
        return;
    };
    let msg = match ListKind::Bookmarks.update(|l| {
        if l.remove(&path) {
            Ok(false)
        } else {
            l.push(&path).map(|_| true)
        }
    }) {
        Some(Ok(true)) => "ブックマークに追加しました".to_owned(),
        Some(Ok(false)) => "ブックマークから外しました".to_owned(),
        Some(Err(())) => {
            info_box(
                hwnd,
                &format!(
                    "ブックマークは {} 件までです。不要なものを外してください（ファイル メニューの「ブックマーク」）。",
                    ListKind::Bookmarks.limit()
                ),
            );
            return;
        }
        None => "ブックマークを保存できませんでした".to_owned(),
    };
    with_app(|a| {
        a.status_msg = msg;
        a.update_status();
    });
}

/// リモートのファイルを選んで開く（11 章 7.2）。
fn cmd_open_remote(hwnd: HWND) {
    let Some((available, initial)) = with_app(|a| {
        let current = a
            .doc
            .remote()
            .and_then(|r| yy_remote::RemoteUri::parse(&r.uri));
        (
            crate::remote::available(),
            current.or_else(crate::remote::last),
        )
    }) else {
        return;
    };
    if !available {
        info_box(
            hwnd,
            "この yyeditor には SSH の機能が組み込まれていません。",
        );
        return;
    }
    let Some(p) = crate::remotedlg::show(hwnd, crate::remotedlg::Mode::Open, initial, None, false)
    else {
        return;
    };
    let enc = match p
        .encoding
        .map(|e| crate::recorddlg::confirm(hwnd, e, "開く"))
    {
        Some(None) => return,
        Some(e) => e,
        None => None,
    };
    if let Err(msg) = crate::remote::open(hwnd, &p.uri, enc, false, crate::remote::OpenAs::NewTab) {
        error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
    }
}

fn open_path(hwnd: HWND, path: PathBuf, encoding: Option<Encoding>, shared_read_only: bool) {
    // SSH 接続先のファイル（履歴・ブックマーク・コマンドラインの `ssh://…`）
    if yy_config::recent::is_remote(&path) {
        if let Err(msg) = crate::remote::open_location(hwnd, &path, encoding) {
            error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
        }
        return;
    }
    if let Some(Err(msg)) = with_app(|a| a.open(path, encoding, shared_read_only)) {
        error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
    }
}

/// 保存先と形式。
pub(crate) struct SaveTarget {
    /// 保存先（リモートなら `ssh://…`。メッセージと拡張子の判定に使う）
    path: PathBuf,
    encoding: Encoding,
    bom: bool,
    /// 保存前に揃える改行コード（`None` なら変更しない）
    eol: Option<Eol>,
    /// SSH 接続先のファイルへの保存（11 章 7.1）
    remote: Option<crate::remote::RemoteDest>,
}

/// バックグラウンドの保存の手順（変換できない文字の扱いを尋ねて保存し直すための状態）。
pub(crate) struct SaveFlow {
    target: SaveTarget,
    /// 似た文字への置き換えを尋ねた
    fold_offered: bool,
    /// 「?」などに置き換えた
    replaced: bool,
}

fn message_box(hwnd: HWND, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from("yyeditor"),
            style,
        )
    }
}

/// 保存する。`as_new` または名前がなければ保存先と形式を尋ねる。保存はバックグラウンドで行い
/// （結果は [`on_save_done`] で処理する）、始められたら `true`。
fn cmd_save(hwnd: HWND, as_new: bool) -> bool {
    cmd_save_to(hwnd, as_new, SaveWhere::Same)
}

/// 「名前を付けて保存」の保存先の種類。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveWhere {
    /// 今のファイルと同じ側（リモートのファイルならリモート）
    Same,
    Remote,
    Local,
}

fn cmd_save_to(hwnd: HWND, as_new: bool, place: SaveWhere) -> bool {
    if with_app(|a| a.doc.is_read_only()) == Some(true) {
        info_box(hwnd, "読み取り専用で開いたファイルは保存できません。");
        return false;
    }
    if with_app(|a| a.doc.is_saving() || a.save_after_convert.is_some()) == Some(true) {
        info_box(hwnd, "保存中です。終わってから、もう一度保存してください。");
        return false;
    }
    let Some((current, remote, encoding, bom, eol, loading, noncanonical)) = with_app(|a| {
        let nc = if a.warned_noncanonical {
            0
        } else {
            a.doc.decode_stats().noncanonical
        };
        (
            a.doc.path().map(|p| p.to_owned()),
            a.doc.remote().cloned(),
            a.doc.encoding(),
            a.doc.has_bom(),
            a.doc.eol(),
            a.doc.is_loading(),
            nc,
        )
    }) else {
        return false;
    };
    // リモートのファイルの保存は、手元の写しのパスを使わない
    let current = if remote.is_some() { None } else { current };
    if loading {
        info_box(hwnd, "読み込みが終わるまで保存できません。");
        return false;
    }
    let to_remote = match place {
        SaveWhere::Same => remote.is_some(),
        SaveWhere::Remote => true,
        SaveWhere::Local => false,
    };
    let target = match (&remote, current) {
        // リモートのファイルの上書き保存
        (Some(file), _) if !as_new && to_remote => match crate::remote::current_dest(file) {
            Ok(dest) => SaveTarget {
                path: PathBuf::from(&file.uri),
                encoding,
                bom,
                eol: None,
                remote: Some(dest),
            },
            Err(msg) => {
                error_box(hwnd, &format!("保存できませんでした。\n{msg}"));
                return false;
            }
        },
        (_, Some(path)) if !as_new => SaveTarget {
            path,
            encoding,
            bom,
            eol: None,
            remote: None,
        },
        _ if to_remote => {
            let initial = remote
                .as_ref()
                .and_then(|f| yy_remote::RemoteUri::parse(&f.uri))
                .or_else(crate::remote::last);
            let Some(p) = crate::remotedlg::show(
                hwnd,
                crate::remotedlg::Mode::Save,
                initial,
                Some(encoding),
                bom,
            ) else {
                return false;
            };
            let mut enc = p.encoding.unwrap_or(encoding);
            if enc != encoding {
                // EBCDIC はレコードの区切り方も尋ねる
                match crate::recorddlg::confirm(hwnd, enc, "保存する") {
                    Some(e) => enc = e,
                    None => return false,
                }
            }
            SaveTarget {
                path: PathBuf::from(p.uri.to_string()),
                encoding: enc,
                bom: p.bom && enc.supports_bom(),
                eol: None,
                remote: p.dest(),
            }
        }
        (_, current) => match show_save_dialog(hwnd, current.as_deref(), encoding, bom, eol) {
            Some(t) => t,
            None => return false,
        },
    };
    // 区切り文字モードで拡張子を .csv ⇔ .tsv に変える場合は区切り文字の変換を勧める（04 章 5）
    let current_dialect = with_app(|a| a.csv.as_ref().map(|c| c.view.dialect)).flatten();
    let new_dialect = target
        .path
        .extension()
        .and_then(|e| yy_delimited::Dialect::for_extension(&e.to_string_lossy()));
    if let (Some(cur), Some(new)) = (current_dialect, new_dialect)
        && cur.delimiter() != new.delimiter()
    {
        let msg = format!(
            "拡張子に合わせて、区切り文字を「{}」から「{}」に変換しますか？\n\n\
             はい: 変換して保存（「元に戻す」で取り消せます）\nいいえ: そのまま保存",
            cur.name(),
            new.name()
        );
        match message_box(hwnd, &msg, MB_YESNOCANCEL | MB_ICONQUESTION) {
            IDYES => {
                let busy = with_app(|a| {
                    a.csv_record_op(yy_core::csv::RecordOp::Convert(new));
                    a.doc.is_busy()
                });
                if busy == Some(true) {
                    info_box(
                        hwnd,
                        "区切り文字を変換しています。終わってから、もう一度保存してください。",
                    );
                    return false;
                }
            }
            IDNO => {}
            _ => return false,
        }
    }
    if target.encoding == encoding && noncanonical > 0 {
        let msg = format!(
            "このファイルには、保存すると符号が変わる文字が {} 個あります\n\
             （CP932 の NEC 選定 IBM 拡張文字など、同じ文字に複数の符号があるもの）。\n\
             保存すると Windows 標準の符号になります。\n\n保存しますか？",
            group_digits(noncanonical)
        );
        if message_box(hwnd, &msg, MB_OKCANCEL | MB_ICONWARNING) != IDOK {
            return false;
        }
        with_app(|a| a.warned_noncanonical = true);
    }
    let flow = SaveFlow {
        target,
        fold_offered: false,
        replaced: false,
    };
    if let Some(eol) = flow.target.eol {
        let r = with_app(|a| {
            let n = a.notifier();
            let r = a.doc.convert_eol_with(eol, &a.pool, n);
            match r {
                Ok(Some(true)) => a.after_edit(),
                Ok(None) => {
                    a.update_status();
                    a.invalidate();
                }
                _ => {}
            }
            r.map_err(|e| e.to_string())
        });
        match r {
            // 大きな文書はバックグラウンドで変換し、終わってから保存する（on_index_progress）
            Some(Ok(None)) => {
                with_app(|a| a.save_after_convert = Some(flow));
                return true;
            }
            Some(Ok(Some(_))) => {}
            Some(Err(msg)) => {
                error_box(hwnd, &format!("改行コードを変換できませんでした。\n{msg}"));
                return false;
            }
            None => return false,
        }
    }
    begin_save(hwnd, flow)
}

/// バックグラウンドで保存を始める。始められなければエラーを表示して `false`。
fn begin_save(hwnd: HWND, flow: SaveFlow) -> bool {
    match with_app(|a| a.start_save(flow)) {
        Some(Ok(())) => true,
        Some(Err(msg)) => {
            error_box(hwnd, &format!("保存できませんでした。\n{msg}"));
            false
        }
        None => false,
    }
}

/// 保存が終わっていれば結果を処理する（[`App::save_done`]）。
fn on_save_done(hwnd: HWND) {
    let Some(Some((flow, done))) = with_app(|a| a.save_done.take()) else {
        return;
    };
    // 尋ねている間に裏のタブが表に出ないようにする
    with_app(|a| a.defer_inactive_saves += 1);
    handle_save_result(hwnd, flow, done);
    with_app(|a| {
        a.defer_inactive_saves -= 1;
        unsafe {
            let _ = PostMessageW(Some(a.frame), WM_APP_INDEX, WPARAM(0), LPARAM(0));
        }
    });
}

/// 保存の結果を処理する: エラーを表示し、変換できない文字があれば扱いを尋ねて保存し直す。
fn handle_save_result(hwnd: HWND, flow: Option<SaveFlow>, done: yy_core::SaveDone) {
    let err = match done.result {
        Ok(()) => {
            let path = with_app(|a| {
                a.status_msg = "保存しました".into();
                a.update_status();
                if let Some(u) = a
                    .doc
                    .remote()
                    .and_then(|r| yy_remote::RemoteUri::parse(&r.uri))
                {
                    crate::remote::set_last(u);
                }
                a.doc.location()
            });
            // 名前を付けて保存したファイルも履歴に残す
            if let Some(Some(p)) = path {
                crate::recentdlg::remember(&p);
            }
            return;
        }
        Err(e) => e,
    };
    let Some(mut flow) = flow else {
        error_box(hwnd, &format!("保存できませんでした。\n{err}"));
        return;
    };
    let (ranges, total) = match err {
        SaveError::Io(e) if yy_io::is_cancelled(&e) => {
            with_app(|a| {
                a.status_msg = "保存を中止しました".into();
                a.update_status();
            });
            return;
        }
        SaveError::Unmappable { ranges, total, .. } => (ranges, total),
        // リモートのファイルが外部で変更されていた（11 章 7.1）
        SaveError::Conflict(msg) => {
            let text = format!(
                "{}\n\n{msg}\n\n上書きしますか？（いいえ: 保存しない）",
                flow.target.path.display()
            );
            if message_box(hwnd, &text, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES {
                if let Some(r) = flow.target.remote.as_mut() {
                    r.force = true;
                }
                begin_save(hwnd, flow);
            }
            return;
        }
        e => {
            error_box(
                hwnd,
                &format!(
                    "保存できませんでした。\n{}\n\n{e}",
                    flow.target.path.display()
                ),
            );
            return;
        }
    };
    // 保存中に編集していれば、見つかった範囲は今の内容と合わないので保存し直す
    if done.edited {
        begin_save(hwnd, flow);
        return;
    }
    let target = &flow.target;
    let first = ranges[0].clone();
    let desc = with_app(|a| a.describe_offset(&first)).unwrap_or_default();
    let mut msg = format!(
        "{} 個の文字は {} で保存できません。\n最初の文字: {desc}\n\n",
        group_digits(total),
        target.encoding.name()
    );
    // 互換文字・半角カナなどを似た文字に置き換えられるか（03 章 4.1）
    if !flow.fold_offered {
        flow.fold_offered = true;
        let foldable = with_app(|a| a.foldable(&ranges, target.encoding)).unwrap_or_default();
        if !foldable.is_empty() {
            let text = format!(
                "{msg}このうち {} か所は似た文字に置き換えられます\n\
                 （① → (1)、Ⅱ → II、ｶﾞ → ガ、全角英数 → 半角 など）。\n\n\
                 はい: 似た文字に置き換えて保存（置き換えられない文字は続けて尋ねます）\n\
                 いいえ: 置き換えない\n\
                 キャンセル: 保存せずに最初の文字へ移動\n\n\
                 置き換えは「元に戻す」で取り消せます。",
                group_digits(foldable.len() as u64)
            );
            match message_box(hwnd, &text, MB_YESNOCANCEL | MB_ICONWARNING) {
                IDYES => {
                    let enc = target.encoding;
                    with_app(|a| a.replace_folded(&foldable, enc));
                    begin_save(hwnd, flow);
                    return;
                }
                IDNO => {}
                _ => {
                    with_app(|a| a.select_range(first));
                    return;
                }
            }
        }
    }
    if flow.replaced || ranges.len() as u64 != total {
        msg += "保存せずに最初の文字へ移動します。";
        message_box(hwnd, &msg, MB_OK | MB_ICONWARNING);
        with_app(|a| a.select_range(first));
        return;
    }
    msg += "はい: 「?」に置き換えて保存\n\
            いいえ: 数値文字参照（&#x….;）に置き換えて保存\n\
            キャンセル: 保存せずに最初の文字へ移動\n\n\
            置き換えは「元に戻す」で取り消せます。";
    match message_box(hwnd, &msg, MB_YESNOCANCEL | MB_ICONWARNING) {
        IDYES => with_app(|a| a.replace_unmappable(&ranges, false)),
        IDNO => with_app(|a| a.replace_unmappable(&ranges, true)),
        _ => {
            with_app(|a| a.select_range(first));
            return;
        }
    };
    flow.replaced = true;
    begin_save(hwnd, flow);
}

/// 作業中のタブの保存（変換できない文字の扱いを尋ねて保存し直す場合も含む）が終わるまで待つ。
///
/// その間もウィンドウの描画や進捗の表示は続けるが、キーボードとマウスの操作は受け付けない
/// （Esc で保存を中止できる）。
fn wait_save() {
    with_app(|a| a.defer_inactive_saves += 1);
    let busy = || {
        with_app(|a| a.doc.is_saving() || a.save_after_convert.is_some() || a.save_done.is_some())
            .unwrap_or(false)
    };
    let mut msg = MSG::default();
    while busy() {
        unsafe {
            match GetMessageW(&mut msg, None, 0, 0).0 {
                -1 => return,
                0 => {
                    // 終了の要求はメインのメッセージループに任せる
                    PostQuitMessage(msg.wParam.0 as i32);
                    return;
                }
                _ => {}
            }
            let m = msg.message;
            let input = (WM_KEYFIRST..=WM_KEYLAST).contains(&m)
                || (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m)
                || (WM_NCMOUSEMOVE..=WM_NCXBUTTONDBLCLK).contains(&m);
            if input {
                if m == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize {
                    with_app(|a| a.doc.cancel_save());
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // 待っている間に終わった裏のタブの保存は、今の操作が終わってから処理する
    with_app(|a| {
        a.defer_inactive_saves -= 1;
        unsafe {
            let _ = PostMessageW(Some(a.frame), WM_APP_INDEX, WPARAM(0), LPARAM(0));
        }
    });
}

// 保存・開くダイアログに追加するコントロールの ID
const CTL_ENCODING: u32 = 1;
const CTL_BOM: u32 = 2;
const CTL_EOL: u32 = 3;
const CTL_GROUP_ENCODING: u32 = 4;
const CTL_GROUP_EOL: u32 = 5;

/// ダイアログに文字コードの選択欄を追加する。`auto` なら先頭に「自動判別」を置く。
fn add_encoding_combo(c: &IFileDialogCustomize, selected: Option<Encoding>, auto: bool) {
    unsafe {
        let _ = c.StartVisualGroup(CTL_GROUP_ENCODING, w!("文字コード:"));
        let _ = c.AddComboBox(CTL_ENCODING);
        let offset = u32::from(auto);
        if auto {
            let _ = c.AddControlItem(CTL_ENCODING, 0, w!("自動判別"));
        }
        let all = Encoding::all();
        for (i, e) in all.iter().enumerate() {
            let _ = c.AddControlItem(CTL_ENCODING, i as u32 + offset, &HSTRING::from(e.label()));
        }
        let sel = selected
            .and_then(|s| all.iter().position(|e| e.same_charset(&s)))
            .map_or(0, |i| i as u32 + offset);
        let _ = c.SetSelectedControlItem(CTL_ENCODING, sel);
        let _ = c.EndVisualGroup();
    }
}

/// 選択された文字コード（「自動判別」なら `None`）。
fn selected_encoding(c: &IFileDialogCustomize, auto: bool) -> Option<Encoding> {
    let i = unsafe { c.GetSelectedControlItem(CTL_ENCODING) }.ok()?;
    let i = i.checked_sub(u32::from(auto))?;
    Encoding::all().get(i as usize).copied()
}

/// ダイアログで選ばれたファイルのパス。
fn dialog_result(dialog: &IFileDialog) -> Option<PathBuf> {
    unsafe {
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// 開くファイルと文字コード（`None` なら自動判別）を尋ねる。
fn show_open_dialog(owner: HWND) -> Option<(Vec<PathBuf>, Option<Encoding>)> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog
            .SetOptions(dialog.GetOptions().ok()? | FOS_ALLOWMULTISELECT)
            .ok()?;
        let custom = dialog.cast::<IFileDialogCustomize>().ok();
        if let Some(c) = &custom {
            add_encoding_combo(c, None, true);
        }
        dialog.Show(Some(owner)).ok()?;
        let results = dialog.GetResults().ok()?;
        let count = results.GetCount().ok()?;
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let item = results.GetItemAt(i).ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            paths.push(PathBuf::from(path?));
        }
        let enc = custom.as_ref().and_then(|c| selected_encoding(c, true));
        Some((paths, enc))
    }
}

/// 保存先と形式（文字コード・BOM・改行コード）を尋ねる。
fn show_save_dialog(
    owner: HWND,
    current: Option<&std::path::Path>,
    encoding: Encoding,
    bom: bool,
    eol: Eol,
) -> Option<SaveTarget> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("すべてのファイル (*.*)"),
                pszSpec: w!("*.*"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("テキスト ファイル (*.txt)"),
                pszSpec: w!("*.txt"),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        let name = current
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "無題.txt".to_owned());
        let _ = dialog.SetFileName(&HSTRING::from(name));
        let custom = dialog.cast::<IFileDialogCustomize>().ok();
        if let Some(c) = &custom {
            add_encoding_combo(c, Some(encoding), false);
            let _ = c.AddCheckButton(CTL_BOM, w!("BOM を付ける（Unicode のみ）"), bom);
            let _ = c.StartVisualGroup(CTL_GROUP_EOL, w!("改行コード:"));
            let _ = c.AddComboBox(CTL_EOL);
            let _ = c.AddControlItem(
                CTL_EOL,
                0,
                &HSTRING::from(format!("変更しない（{}）", eol.label())),
            );
            let _ = c.AddControlItem(CTL_EOL, 1, w!("CRLF (Windows)"));
            let _ = c.AddControlItem(CTL_EOL, 2, w!("LF (Unix)"));
            let _ = c.SetSelectedControlItem(CTL_EOL, 0);
            let _ = c.EndVisualGroup();
        }
        dialog.Show(Some(owner)).ok()?;
        let path = dialog_result(&dialog.cast().ok()?)?;
        let mut target = SaveTarget {
            path,
            encoding,
            bom,
            eol: None,
            remote: None,
        };
        if let Some(c) = &custom {
            target.encoding = match selected_encoding(c, false) {
                // EBCDIC のレコードの区切り方は今のものを初期値にして尋ねる
                Some(e) if e.same_charset(&encoding) && encoding.records().is_some() => {
                    crate::recorddlg::ask_records(owner, encoding, "保存する")?
                }
                Some(e) => crate::recorddlg::confirm(owner, e, "保存する")?,
                None => encoding,
            };
            target.bom = c.GetCheckButtonState(CTL_BOM).map_or(bom, |b| b.as_bool());
            target.eol = match c.GetSelectedControlItem(CTL_EOL) {
                Ok(1) => Some(Eol::CrLf),
                Ok(2) => Some(Eol::Lf),
                _ => None,
            }
            .filter(|e| *e != eol);
        }
        Some(target)
    }
}

fn cmd_copy(hwnd: HWND, cut: bool) {
    let Some(sel) = with_app(|a| a.copy_selection()) else {
        return;
    };
    match sel {
        Ok(Some((text, column))) => {
            if !clipboard::set_text(hwnd, &text, column) {
                error_box(hwnd, "クリップボードにコピーできませんでした。");
                return;
            }
            if cut {
                with_app(|a| a.delete_selection(EditKind::Cut));
            }
        }
        Ok(None) => {}
        Err(n) => info_box(
            hwnd,
            &format!(
                "選択範囲が大きすぎるためコピーできません（{}）。",
                human_size(n)
            ),
        ),
    }
}

fn on_command(hwnd: HWND, id: u16) {
    if on_workspace_command(hwnd, id) {
        return;
    }
    match id {
        ID_NEW => {
            with_app(|a| a.new_tab());
        }
        ID_OPEN => {
            if let Some((paths, enc)) = show_open_dialog(hwnd) {
                let enc = match enc.map(|e| crate::recorddlg::confirm(hwnd, e, "開く")) {
                    Some(None) => return,
                    Some(e) => e,
                    None => None,
                };
                for p in paths {
                    open_path(hwnd, p, enc, false);
                }
            }
        }
        ID_OPEN_SHARED => {
            if let Some((paths, enc)) = show_open_dialog(hwnd) {
                let enc = match enc.map(|e| crate::recorddlg::confirm(hwnd, e, "開く")) {
                    Some(None) => return,
                    Some(e) => e,
                    None => None,
                };
                for p in paths {
                    open_path(hwnd, p, enc, true);
                }
            }
        }
        id if id >= ID_REOPEN_BASE && ((id - ID_REOPEN_BASE) as usize) < Encoding::all().len() => {
            let enc = Encoding::all()[(id - ID_REOPEN_BASE) as usize];
            let Some((path, current)) =
                with_app(|a| (a.doc.path().map(|p| p.to_owned()), a.doc.encoding()))
            else {
                return;
            };
            let Some(path) = path else {
                info_box(hwnd, "ファイルを開いていません。");
                return;
            };
            // EBCDIC はレコードの区切り方も尋ねる（同じ文字コードなら今の区切り方を初期値にする）
            let enc = if enc.same_charset(&current) {
                current
            } else {
                enc
            };
            let Some(enc) = crate::recorddlg::confirm(hwnd, enc, "開き直す") else {
                return;
            };
            if confirm_discard(hwnd) {
                // リモートのファイルは取り寄せ直す
                let remote = with_app(|a| a.doc.remote().map(|r| r.uri.clone())).flatten();
                let r = match remote.as_deref().and_then(yy_remote::RemoteUri::parse) {
                    Some(uri) => crate::remote::open(
                        hwnd,
                        &uri,
                        Some(enc),
                        false,
                        crate::remote::OpenAs::Replace,
                    ),
                    None => with_app(|a| a.reopen(path, enc)).unwrap_or(Ok(())),
                };
                if let Err(msg) = r {
                    error_box(hwnd, &format!("ファイルを開けません。\n{msg}"));
                }
            }
        }
        ID_SAVE => {
            cmd_save(hwnd, false);
        }
        ID_SAVE_AS => {
            cmd_save(hwnd, true);
        }
        ID_SAVE_AS_REMOTE => {
            cmd_save_to(hwnd, true, SaveWhere::Remote);
        }
        ID_SAVE_AS_LOCAL => {
            cmd_save_to(hwnd, true, SaveWhere::Local);
        }
        ID_OPEN_REMOTE => cmd_open_remote(hwnd),
        ID_CLOSE => {
            if confirm_discard(hwnd) {
                with_app(|a| a.close_tab());
            }
        }
        ID_TAB_NEXT | ID_TAB_PREV => {
            with_app(|a| {
                let n = a.tabs.len();
                let next = if id == ID_TAB_NEXT {
                    (a.active_tab + 1) % n
                } else {
                    (a.active_tab + n - 1) % n
                };
                a.switch_tab(next);
            });
        }
        ID_TO_UPPER | ID_TO_LOWER | ID_TO_FULL_KANA | ID_TO_HALF_KANA | ID_TO_CAMEL
        | ID_TO_SNAKE | ID_TO_KEBAB => {
            use yy_core::transform::Transform;
            let t = match id {
                ID_TO_UPPER => Transform::Upper,
                ID_TO_LOWER => Transform::Lower,
                ID_TO_FULL_KANA => Transform::FullKatakana,
                ID_TO_HALF_KANA => Transform::HalfKatakana,
                ID_TO_CAMEL => Transform::Camel,
                ID_TO_SNAKE => Transform::Snake,
                _ => Transform::Kebab,
            };
            with_app(|a| a.transform_selection(t));
        }
        ID_DEDUP_LINES => {
            with_app(|a| a.dedup_lines());
        }
        ID_TOGGLE_COMMENT => {
            with_app(|a| a.toggle_comment());
        }
        ID_GOTO_BRACKET => {
            with_app(|a| a.goto_bracket());
        }
        id if hexmode::is_charset_command(id) => {
            with_app(|a| a.set_hex_charset(id));
        }
        id if (ID_SYNTAX_NONE..ID_SYNTAX_BASE + 150).contains(&id) => {
            with_app(|a| {
                a.choose_syntax(id);
                a.refresh_preview();
            });
        }
        ID_DIFF => {
            let Some((count, active)) = with_app(|a| (a.tabs.len(), a.active_tab)) else {
                return;
            };
            if count < 2 {
                info_box(hwnd, "比較するファイルをもう 1 つ開いてください。");
                return;
            }
            let other = if count == 2 {
                1 - active
            } else {
                let prompt = format!("比較先タブの番号（1 〜 {count}、現在は {}）:", active + 1);
                let Some(n) =
                    crate::goto::prompt_line(hwnd, &prompt, ((active + 1) % count + 1) as u64)
                else {
                    return;
                };
                let index = n as usize - 1;
                if index >= count || index == active {
                    info_box(hwnd, "現在のタブ以外の番号を指定してください。");
                    return;
                }
                index
            };
            let docs = with_app(|a| {
                let target = &a.tabs[other].as_ref().expect("inactive tab").doc;
                (
                    a.doc.display_name(),
                    a.doc.snapshot().clone(),
                    a.doc.is_loading(),
                    target.display_name(),
                    target.snapshot().clone(),
                    target.is_loading(),
                )
            });
            if let Some((ln, ls, ll, rn, rs, rl)) = docs {
                if ll || rl {
                    info_box(hwnd, "文字コードの変換が終わってから比較してください。");
                } else if let Err(e) = crate::diffview::show(hwnd, &ln, &ls, &rn, &rs) {
                    error_box(hwnd, &e);
                }
            }
        }
        ID_EXIT => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        ID_UNDO => {
            with_app(|a| {
                a.rect = None;
                if a.doc.undo() {
                    a.composition = None;
                    a.after_edit();
                }
            });
        }
        ID_REDO => {
            with_app(|a| {
                a.rect = None;
                if a.doc.redo() {
                    a.composition = None;
                    a.after_edit();
                }
            });
        }
        ID_CUT | ID_COPY if with_app(|a| a.hex.is_some()) == Some(true) => {
            if let Some(Some(text)) = with_app(|a| a.hex_copy_text()) {
                if !clipboard::set_text(hwnd, &text, false) {
                    error_box(hwnd, "クリップボードにコピーできませんでした。");
                } else if id == ID_CUT {
                    with_app(|a| a.hex_cut());
                }
            }
        }
        ID_PASTE if with_app(|a| a.hex.is_some()) == Some(true) => {
            if let Some((text, _)) = clipboard::get_text(hwnd) {
                with_app(|a| a.hex_paste(&text));
            }
        }
        ID_DELETE if with_app(|a| a.hex.is_some()) == Some(true) => {
            with_app(|a| a.hex_delete(false));
        }
        ID_CUT | ID_COPY if with_app(|a| a.code.is_some()) == Some(true) => {
            if let Some(Some(text)) = with_app(|a| a.code_copy_text()) {
                if !clipboard::set_text(hwnd, &text, false) {
                    error_box(hwnd, "クリップボードにコピーできませんでした。");
                } else if id == ID_CUT {
                    with_app(|a| a.code_cut());
                }
            }
        }
        ID_PASTE if with_app(|a| a.code.is_some()) == Some(true) => {
            if let Some((text, _)) = clipboard::get_text(hwnd) {
                with_app(|a| a.code_paste(&text));
            }
        }
        ID_DELETE if with_app(|a| a.code.is_some()) == Some(true) => {
            with_app(|a| a.code_delete(false));
        }
        ID_GOTO if with_app(|a| a.hex.is_some()) == Some(true) => cmd_hex_goto(hwnd),
        ID_HEX_MODE => cmd_toggle_hex(hwnd),
        ID_HISTORY | ID_BOOKMARKS => {
            let kind = if id == ID_HISTORY {
                crate::recentdlg::ListKind::History
            } else {
                crate::recentdlg::ListKind::Bookmarks
            };
            for p in crate::recentdlg::show(hwnd, kind) {
                open_path(hwnd, p, None, false);
            }
        }
        ID_BOOKMARK_TOGGLE => cmd_toggle_bookmark(hwnd),
        ID_CODE_MODE => cmd_toggle_code(hwnd),
        ID_RECORD_MODE => cmd_toggle_record(hwnd),
        ID_OPEN_BINARY => {
            if let Some((paths, _)) = show_open_dialog(hwnd) {
                cmd_open_binary(hwnd, paths);
            }
        }
        ID_CUT => cmd_copy(hwnd, true),
        ID_COPY => cmd_copy(hwnd, false),
        ID_PASTE => {
            if let Some((text, column)) = clipboard::get_text(hwnd) {
                with_app(|a| a.paste(&text, column));
            }
        }
        ID_DELETE => {
            with_app(|a| {
                if !a.delete_selection(EditKind::Other)
                    && a.rect.is_none()
                    && a.doc.delete_forward()
                {
                    a.after_edit();
                }
            });
        }
        ID_SELECT_ALL => {
            with_app(|a| {
                a.rect = None;
                a.doc.select_all();
                a.after_move();
            });
        }
        ID_SELECT_NEXT => {
            with_app(|a| {
                a.rect = None;
                a.doc.select_next_occurrence();
                a.after_move();
            });
        }
        ID_SELECT_ALL_OCCURRENCES | ID_CARETS_AT_LINE_ENDS => {
            let result = with_app(|a| {
                a.rect = None;
                let r = if id == ID_SELECT_ALL_OCCURRENCES {
                    a.doc.select_all_occurrences(CARET_LIMIT)
                } else {
                    a.doc.carets_at_line_ends(CARET_LIMIT)
                };
                a.after_move();
                r
            });
            if let Some((n, true)) = result {
                info_box(
                    hwnd,
                    &format!(
                        "カーソルが多すぎるため、先頭から {} 個までにしました。",
                        group_digits(n as u64)
                    ),
                );
            }
        }
        ID_SELECT_SEARCH_MATCHES => {
            let result = with_app(|a| {
                let r = a.select_search_matches();
                // 検索バーから選んだら、続けて入力できるように本文へフォーカスを移す
                if a.findbar.has_focus() {
                    unsafe {
                        let _ = SetFocus(Some(a.view));
                    }
                }
                r
            });
            if let Some((n, true)) = result {
                info_box(
                    hwnd,
                    &format!(
                        "一致が多いため、先頭から {} 箇所まで選択しました。",
                        group_digits(n as u64)
                    ),
                );
            }
        }
        ID_FIND | ID_REPLACE => {
            with_app(|a| a.open_findbar(id == ID_REPLACE));
        }
        ID_FIND_NEXT | ID_FIND_PREV => {
            with_app(|a| a.find(id == ID_FIND_NEXT));
        }
        ID_FIND_OK => {
            with_app(|a| {
                if a.findbar.replacement_focused() {
                    a.replace_one();
                } else {
                    a.find(!key_down(VK_SHIFT));
                }
            });
        }
        ID_FIND_CLOSE => {
            with_app(|a| a.close_findbar());
        }
        ID_FIND_TOGGLE_REPLACE => {
            with_app(|a| {
                a.findbar.toggle_replace();
                a.layout_children();
                a.invalidate();
            });
        }
        ID_FIND_CHANGED => {
            with_app(|a| {
                a.compile_search();
                a.invalidate();
            });
        }
        ID_FIND_INCREMENTAL => {
            with_app(|a| a.find_incremental());
        }
        ID_REPLACE_ONE => {
            with_app(|a| a.replace_one());
        }
        ID_GREP => {
            let Some(initial) = with_app(|a| a.grep_defaults()) else {
                return;
            };
            if let Some(req) = crate::grepdlg::prompt(hwnd, &initial) {
                with_app(|a| a.start_grep(req));
            }
        }
        ID_TAG_JUMP => {
            if let Some(Some(msg)) = with_app(|a| a.tag_jump()) {
                info_box(hwnd, &msg);
            }
        }
        ID_REPLACE_ALL => {
            with_app(|a| a.replace_all());
        }
        ID_CSV_OFF => {
            with_app(|a| a.set_csv_mode(None));
        }
        ID_CSV_AUTO => {
            if let Some(Some(msg)) = with_app(|a| a.csv_auto()) {
                info_box(hwnd, &msg);
            }
        }
        ID_CSV_COMMA | ID_CSV_TAB | ID_CSV_SEMICOLON | ID_CSV_PIPE => {
            let delim: &[u8] = match id {
                ID_CSV_COMMA => b",",
                ID_CSV_TAB => b"\t",
                ID_CSV_SEMICOLON => b";",
                _ => b"|",
            };
            with_app(|a| a.set_csv_mode(yy_delimited::Dialect::new(delim, Some(b'"'))));
        }
        ID_CSV_NEXT_CELL | ID_CSV_PREV_CELL => {
            with_app(|a| a.move_cell(id == ID_CSV_NEXT_CELL));
        }
        ID_CSV_INSERT_COL | ID_CSV_DELETE_COL => {
            with_app(|a| match a.caret_field() {
                Some(f) => a.csv_record_op(if id == ID_CSV_INSERT_COL {
                    yy_core::csv::RecordOp::InsertField(f)
                } else {
                    yy_core::csv::RecordOp::DeleteField(f)
                }),
                None => {
                    a.status_msg = "区切り文字モードではありません".into();
                    a.update_status();
                }
            });
        }
        ID_CSV_TO_COMMA | ID_CSV_TO_TAB => {
            let to = if id == ID_CSV_TO_COMMA {
                yy_delimited::Dialect::csv()
            } else {
                yy_delimited::Dialect::tsv()
            };
            with_app(|a| a.csv_record_op(yy_core::csv::RecordOp::Convert(to)));
        }
        ID_RECT_MODE => {
            with_app(|a| {
                a.rect_mode = !a.rect_mode;
                a.update_status();
            });
        }
        ID_RECT_TO_CARETS => {
            with_app(|a| a.rect_to_carets());
        }
        ID_GOTO => {
            let Some((current, total)) = with_app(|a| {
                let snap = a.doc.snapshot();
                let cur = snap.line_of_offset(a.doc.selections().primary().head).line + 1;
                let total = match snap.line_count() {
                    Some(n) => format!("1 〜 {}", group_digits(n)),
                    None => format!("1 〜 約 {}", group_digits(snap.estimated_line_count())),
                };
                (cur, total)
            }) else {
                return;
            };
            let prompt = format!("行番号 ({total}):");
            if let Some(line) = crate::goto::prompt_line(hwnd, &prompt, current) {
                with_app(|a| a.goto_line(line));
            }
        }
        ID_ZOOM_IN => {
            with_app(|a| a.zoom(a.renderer.font_size_pt() + 1.0));
        }
        ID_ZOOM_OUT => {
            with_app(|a| a.zoom(a.renderer.font_size_pt() - 1.0));
        }
        ID_ZOOM_RESET => {
            with_app(|a| a.zoom(a.config.editor.font_size));
        }
        ID_LINE_NUMBERS => {
            with_app(|a| {
                a.show_line_numbers = !a.show_line_numbers;
                a.update_line_number_menu();
                a.after_scroll();
            });
        }
        ID_PREVIEW => {
            if let Some(Err(msg)) = with_app(|a| a.toggle_preview()) {
                error_box(hwnd, &format!("プレビューを表示できません。\n{msg}"));
            }
        }
        ID_WHITESPACE => {
            with_app(|a| {
                a.show_whitespace = !a.show_whitespace;
                a.renderer.set_show_whitespace(a.show_whitespace);
                a.update_line_number_menu();
                a.invalidate();
            });
        }
        ID_CONTROL_CHARS => {
            with_app(|a| {
                a.rows_cfg.show_controls = !a.rows_cfg.show_controls;
                a.row_cache.borrow_mut().rows.clear();
                a.renderer.clear_cache();
                a.update_line_number_menu();
                a.invalidate();
            });
        }
        ID_HELP | ID_HELP_KEYS => {
            let section = (id == ID_HELP_KEYS).then_some("shortcuts");
            if let Err(e) = crate::help::show(section) {
                error_box(hwnd, &format!("ヘルプを表示できません。\n{}", e.message()));
            }
        }
        ID_OPEN_SETTINGS => open_settings(hwnd),
        ID_OPEN_REMOTE_LOG => open_remote_log(hwnd),
        ID_FORGET_PASSWORDS => crate::remote::forget_passwords(hwnd),
        ID_ABOUT => info_box(
            hwnd,
            &format!(
                "yyeditor {}\n\n巨大ファイル対応の軽量テキストエディタ\n\n\
                 このプログラムはフリーソフトウェアです。GNU 一般公衆利用許諾書（GPL）\
                 バージョン 3、またはそれ以降のバージョンの条件で再頒布・改変できます\
                 （Microsoft Edge WebView2 Loader とのリンクを認める追加の許可付き）。\
                 このプログラムは無保証です。",
                env!("CARGO_PKG_VERSION")
            ),
        ),
        _ => {}
    }
}

/// フォーカスが検索バーにあればバーのウィンドウ。
pub(crate) fn findbar_with_focus() -> Option<HWND> {
    with_app(|a| a.findbar.has_focus().then_some(a.findbar.hwnd)).flatten()
}

/// 検索バーにフォーカスがあってもアクセラレータとして扱うキー（それ以外は入力欄に渡す）。
pub(crate) fn is_global_shortcut(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    let vk = VIRTUAL_KEY(msg.wParam.0 as u16);
    if vk == VK_F3 {
        return true;
    }
    key_down(VK_CONTROL)
        && (vk == VK_TAB
            || matches!(
                vk.0 as u8,
                b'F' | b'H' | b'S' | b'O' | b'N' | b'G' | b'W' | b'M'
            ))
}

/// 検索バーのウィンドウプロシージャ。コントロールの通知をフレームへのコマンドにする。
pub(crate) extern "system" fn findbar_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            let code = hiword(wparam.0);
            let cmd = match id {
                // IsDialogMessage が送る Enter / Esc
                1 => Some(ID_FIND_OK),
                2 => Some(ID_FIND_CLOSE),
                findbar::ID_FIND_NEXT_BTN => Some(ID_FIND_NEXT),
                findbar::ID_FIND_PREV_BTN => Some(ID_FIND_PREV),
                findbar::ID_FIND_CLOSE_BTN => Some(ID_FIND_CLOSE),
                findbar::ID_REPLACE_BTN => Some(ID_REPLACE_ONE),
                findbar::ID_REPLACE_ALL_BTN => Some(ID_REPLACE_ALL),
                findbar::ID_TOGGLE_REPLACE => Some(ID_FIND_TOGGLE_REPLACE),
                findbar::ID_SELECT_MATCHES => Some(ID_SELECT_SEARCH_MATCHES),
                findbar::ID_GREP_BTN => Some(ID_GREP),
                findbar::ID_CASE | findbar::ID_WORD | findbar::ID_REGEX => Some(ID_FIND_CHANGED),
                findbar::ID_PATTERN if code == EN_CHANGE => Some(ID_FIND_INCREMENTAL),
                _ => None,
            };
            if let Some(cmd) = cmd {
                unsafe {
                    if let Ok(parent) = GetParent(hwnd) {
                        let _ =
                            PostMessageW(Some(parent), WM_COMMAND, WPARAM(cmd as usize), LPARAM(0));
                    }
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => unsafe {
            use windows::Win32::Graphics::Gdi::{
                COLOR_BTNFACE, GetSysColorBrush, HDC, SetBkMode, TRANSPARENT,
            };
            // ラベル・チェックボックスの背景をバーと同じ色にする
            SetBkMode(HDC(wparam.0 as *mut _), TRANSPARENT);
            LRESULT(GetSysColorBrush(COLOR_BTNFACE).0 as isize)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn dropped_files(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut result = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            result.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
        }
        DragFinish(hdrop);
        result
    }
}

pub(crate) extern "system" fn frame_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            if with_app(|a| a.layout_children()).is_none() {
                // 再入中なら後でやり直す
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_RELAYOUT, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_APP_RELAYOUT => {
            with_app(|a| a.layout_children());
            LRESULT(0)
        }
        WM_SETFOCUS => {
            if let Some(view) = with_app(|a| a.view) {
                unsafe {
                    let _ = SetFocus(Some(view));
                }
            }
            LRESULT(0)
        }
        WM_INITMENUPOPUP => {
            with_app(|a| a.update_menu_state());
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, loword(wparam.0) as u16);
            LRESULT(0)
        }
        WM_NOTIFY => {
            if lparam.0 != 0 {
                let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
                if let Some(r) = on_tree_notify(hwnd, hdr, lparam) {
                    return r;
                }
                if hdr.code == TCN_SELCHANGE {
                    let view = with_app(|a| {
                        if hdr.hwndFrom == a.tabbar {
                            let index =
                                unsafe { SendMessageW(a.tabbar, TCM_GETCURSEL, None, None).0 };
                            if index >= 0 {
                                a.switch_tab(index as usize);
                            }
                        }
                        a.view
                    });
                    if let Some(view) = view {
                        unsafe {
                            let _ = SetFocus(Some(view));
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            for p in dropped_files(HDROP(wparam.0 as *mut _)) {
                // フォルダはワークスペースに加える
                if p.is_dir() {
                    with_app(|a| a.add_workspace_folder(&p));
                } else {
                    open_path(hwnd, p, None, false);
                }
            }
            LRESULT(0)
        }
        crate::preview::WM_APP_PREVIEW_OPEN => {
            // プレビューのリンクから文書を開く
            let path = unsafe { Box::from_raw(lparam.0 as *mut PathBuf) };
            open_path(hwnd, *path, None, false);
            LRESULT(0)
        }
        WM_SETCURSOR if loword(lparam.0 as usize) == HTCLIENT => {
            let on = unsafe {
                let mut pt = windows::Win32::Foundation::POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                with_app(|a| a.on_preview_splitter(pt.x, pt.y) || a.on_sidebar_splitter(pt.x, pt.y))
                    .unwrap_or(false)
            };
            if on {
                unsafe {
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                }
                LRESULT(1)
            } else {
                default_proc(hwnd, msg, wparam, lparam)
            }
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_of(lparam);
            if with_app(|a| a.on_preview_splitter(x, y)) == Some(true) {
                with_app(|a| a.preview.dragging = true);
                unsafe {
                    SetCapture(hwnd);
                }
            } else if with_app(|a| a.on_sidebar_splitter(x, y)) == Some(true) {
                with_app(|a| a.ws.dragging = true);
                unsafe {
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_of(lparam);
            workspacemode::drag_move(x, y);
            with_app(|a| {
                if a.preview.dragging {
                    a.drag_preview_splitter(x);
                }
                if a.ws.dragging {
                    a.drag_sidebar_splitter(x);
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            // サイドバーの項目のドラッグ（ボタンを離したら移す。キャプチャを失ったら取りやめる）
            let (x, y) = point_of(lparam);
            if workspacemode::drag_end(hwnd, x, y, msg == WM_LBUTTONUP) {
                return LRESULT(0);
            }
            let dragged = with_app(|a| {
                std::mem::take(&mut a.preview.dragging) | std::mem::take(&mut a.ws.dragging)
            });
            if dragged == Some(true) && msg == WM_LBUTTONUP {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            LRESULT(0)
        }
        WM_APP_INDEX => {
            if let Some(Some(msg)) = with_app(|a| a.on_index_progress()) {
                error_box(hwnd, &msg);
            }
            // 終わった保存（作業中のタブ、裏のタブの順）
            with_app(|a| a.poll_save());
            on_save_done(hwnd);
            with_app(|a| a.poll_inactive_saves());
            on_save_done(hwnd);
            // Grep の結果を新しい文書として開く
            if let Some(Some(text)) = with_app(|a| a.grep_done.take()) {
                with_app(|a| {
                    a.add_document(Document::from_text(&text));
                    a.status_msg = "Grep の結果（F12 でファイルを開く）".into();
                    a.update_status();
                });
            }
            LRESULT(0)
        }
        workspacemode::WM_APP_WS_OP => {
            workspacemode::on_op(hwnd, lparam);
            LRESULT(0)
        }
        workspacemode::WM_APP_WS_REMOTE => {
            workspacemode::load_remote(hwnd, wparam.0, lparam.0 as usize);
            LRESULT(0)
        }
        crate::remote::WM_APP_REMOTE_PROMPT => {
            crate::remote::on_prompt(hwnd, lparam);
            LRESULT(0)
        }
        // バックグラウンドの処理を待つループ（remote::wait）の外に届いた通知
        crate::remote::WM_APP_REMOTE_WAKE => LRESULT(0),
        crate::tabclose::WM_APP_CLOSE_TAB => {
            close_tab_at(hwnd, wparam.0);
            LRESULT(0)
        }
        crate::tabclose::WM_APP_TAB_MENU => {
            tab_menu(hwnd, wparam.0);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = hiword(wparam.0);
            unsafe {
                let rc = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            with_app(|a| {
                a.renderer.set_dpi(dpi);
                a.renderer.clear_cache();
                a.findbar.set_dpi(dpi);
                a.set_ui_dpi(dpi);
                a.layout_children();
                a.after_scroll();
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            let count = with_app(|a| a.tabs.len()).unwrap_or(0);
            let mut ok = true;
            for index in 0..count {
                with_app(|a| a.switch_tab(index));
                if !confirm_discard(hwnd) {
                    ok = false;
                    break;
                }
            }
            if ok {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}

fn point_of(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

pub(crate) extern "system" fn view_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with_app(|a| a.paint()).is_none() {
                // 状態を借用中（モーダルループ中など）は描画せず、後で再描画する
                unsafe {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                    let _ = PostMessageW(Some(hwnd), WM_APP_REPAINT, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_APP_REPAINT => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_APP_SCROLLBARS => {
            with_app(|a| a.update_scrollbars());
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
            if with_app(|a| a.resize_view(w, h)).is_none() {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_RESIZE, wparam, lparam);
                }
            }
            LRESULT(0)
        }
        WM_APP_RESIZE => {
            let (w, h) = (loword(lparam.0 as usize), hiword(lparam.0 as usize));
            with_app(|a| a.resize_view(w, h));
            LRESULT(0)
        }
        WM_SETFOCUS => {
            with_app(|a| {
                a.focused = true;
                a.reset_blink();
                a.update_ime_position();
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_BLINK);
            }
            with_app(|a| {
                a.focused = false;
                a.drag = None;
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_PREVIEW => {
            with_app(|a| a.refresh_preview());
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_PROGRESS => {
            with_app(|a| {
                if !a.doc.is_saving() {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_PROGRESS);
                    }
                }
                a.update_status();
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_BLINK => {
            with_app(|a| {
                a.caret_visible = !a.caret_visible;
                a.invalidate();
            });
            LRESULT(0)
        }
        WM_VSCROLL => {
            with_app(|a| a.on_vscroll(loword(wparam.0)));
            LRESULT(0)
        }
        WM_HSCROLL => {
            with_app(|a| a.on_hscroll(loword(wparam.0)));
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = hiword(wparam.0) as u16 as i16;
            with_app(|a| a.on_wheel(delta, msg == WM_MOUSEHWHEEL));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let handled = with_app(|a| a.on_key(VIRTUAL_KEY(wparam.0 as u16))).unwrap_or(false);
            if handled {
                LRESULT(0)
            } else {
                // Shift+Delete / Ctrl+Insert / Shift+Insert（切り取り・コピー・貼り付け）
                let vk = VIRTUAL_KEY(wparam.0 as u16);
                let parent = unsafe { GetParent(hwnd).unwrap_or_default() };
                let cmd = match vk {
                    VK_DELETE if key_down(VK_SHIFT) => Some(ID_CUT),
                    VK_INSERT if key_down(VK_CONTROL) => Some(ID_COPY),
                    VK_INSERT if key_down(VK_SHIFT) => Some(ID_PASTE),
                    _ => None,
                };
                match cmd {
                    Some(c) => {
                        on_command(parent, c);
                        LRESULT(0)
                    }
                    None => default_proc(hwnd, msg, wparam, lparam),
                }
            }
        }
        WM_SYSKEYUP if VIRTUAL_KEY(wparam.0 as u16) == VK_MENU => {
            let suppress = with_app(|a| std::mem::take(&mut a.suppress_alt_up)).unwrap_or(false);
            if suppress {
                LRESULT(0)
            } else {
                default_proc(hwnd, msg, wparam, lparam)
            }
        }
        WM_SYSKEYDOWN => {
            let handled = with_app(|a| a.on_syskey(VIRTUAL_KEY(wparam.0 as u16))).unwrap_or(false);
            if handled {
                LRESULT(0)
            } else {
                default_proc(hwnd, msg, wparam, lparam)
            }
        }
        WM_CHAR => {
            with_app(|a| a.on_char(wparam.0 as u16));
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            unsafe {
                let _ = SetFocus(Some(hwnd));
                SetCapture(hwnd);
            }
            let (x, y) = point_of(lparam);
            with_app(|a| a.on_lbutton_down(x, y, msg == WM_LBUTTONDBLCLK));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point_of(lparam);
            with_app(|a| a.on_mouse_move(x, y));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            with_app(|a| {
                a.drag = None;
                if let Some(h) = &mut a.hex {
                    h.drag_anchor = None;
                }
                if let Some(c) = &mut a.code {
                    c.drag_anchor = None;
                }
            });
            LRESULT(0)
        }
        WM_IME_SETCONTEXT => {
            // システムの変換ウィンドウを出さず、変換中の文字列は自分で描く
            let lp = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            default_proc(hwnd, msg, wparam, lp)
        }
        WM_IME_STARTCOMPOSITION => {
            with_app(|a| a.update_ime_position());
            LRESULT(0)
        }
        WM_IME_COMPOSITION => {
            let update = ime::read_composition(hwnd, lparam.0 as u32);
            with_app(|a| a.on_composition(update));
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            with_app(|a| {
                a.composition = None;
                a.invalidate();
            });
            LRESULT(0)
        }
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}
