//! キー・貼り付け・マウスの操作を、端末に送るバイト列にする（xterm と同じ形）。

use crate::term::{Modes, MouseMode};

/// 文字以外のキー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1〜F12
    F(u8),
}

/// 修飾キー。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    /// xterm の修飾の番号（修飾がなければ `None`）。
    fn code(self) -> Option<u8> {
        let m = u8::from(self.shift) | (u8::from(self.alt) << 1) | (u8::from(self.ctrl) << 2);
        (m != 0).then_some(m + 1)
    }
}

/// キーを送るバイト列。
pub fn encode(key: Key, mods: Mods, modes: &Modes) -> Vec<u8> {
    let alt_prefix = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    // `ESC [ 1 ; m X` 形式（修飾なしは `ESC [ X`、アプリケーション モードは `ESC O X`）
    let cursor = |c: u8| match mods.code() {
        Some(m) => format!("\x1b[1;{m}{}", c as char).into_bytes(),
        None if modes.app_cursor => vec![0x1b, b'O', c],
        None => vec![0x1b, b'[', c],
    };
    // `ESC [ n ; m ~` 形式
    let tilde = |n: u8| match mods.code() {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    };
    match key {
        Key::Char(c) => {
            let mut v = Vec::new();
            if mods.ctrl {
                if let Some(b) = ctrl_byte(c) {
                    v.push(b);
                    return alt_prefix(v);
                }
            }
            let mut buf = [0u8; 4];
            v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            alt_prefix(v)
        }
        Key::Enter => alt_prefix(if modes.newline {
            b"\r\n".to_vec()
        } else {
            b"\r".to_vec()
        }),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(b"\t".to_vec()),
        Key::Backspace if mods.ctrl => alt_prefix(vec![0x08]),
        Key::Backspace => alt_prefix(vec![0x7f]),
        Key::Escape => alt_prefix(vec![0x1b]),
        Key::Up => cursor(b'A'),
        Key::Down => cursor(b'B'),
        Key::Right => cursor(b'C'),
        Key::Left => cursor(b'D'),
        Key::Home => cursor(b'H'),
        Key::End => cursor(b'F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let c = b'P' + (n - 1);
            match mods.code() {
                Some(m) => format!("\x1b[1;{m}{}", c as char).into_bytes(),
                None => vec![0x1b, b'O', c],
            }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        Key::F(_) => Vec::new(),
    }
}

/// Ctrl と組み合わせた文字の制御文字（`Ctrl+A` → 0x01 など）。
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// 貼り付ける文字列を送るバイト列（改行は CR にする。ブラケット ペーストなら囲む）。
pub fn paste(text: &str, modes: &Modes) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        // 貼り付けの終わりの印を含めて、貼り付けの外に出られないようにする
        let body: String = normalized
            .replace("\x1b[201~", "")
            .chars()
            .filter(|&c| c != '\x1b')
            .collect();
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(body.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        normalized.into_bytes()
    }
}

/// マウスのボタン。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// マウスの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    Press(Button),
    Release(Button),
    /// 移動（押しているボタンがあれば `Some`）
    Move(Option<Button>),
}

/// マウスの操作を報告するバイト列（プログラムが要求していなければ `None`）。`col`・`row` は 0 始まり。
pub fn mouse(ev: MouseEvent, col: usize, row: usize, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    let mode = modes.mouse;
    let wanted = match ev {
        MouseEvent::Press(_) => mode != MouseMode::Off,
        MouseEvent::Release(_) => !matches!(mode, MouseMode::Off | MouseMode::Press),
        MouseEvent::Move(Some(_)) => {
            matches!(mode, MouseMode::ButtonMotion | MouseMode::AnyMotion)
        }
        MouseEvent::Move(None) => mode == MouseMode::AnyMotion,
    };
    if !wanted {
        return None;
    }
    let button_code = |b: Button| match b {
        Button::Left => 0u32,
        Button::Middle => 1,
        Button::Right => 2,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    let mut code = match ev {
        MouseEvent::Press(b) => button_code(b),
        MouseEvent::Release(b) => {
            if modes.mouse_sgr {
                button_code(b)
            } else {
                3
            }
        }
        MouseEvent::Move(b) => 32 + b.map_or(3, button_code),
    };
    if mode != MouseMode::Press {
        code += u32::from(mods.shift) * 4 + u32::from(mods.alt) * 8 + u32::from(mods.ctrl) * 16;
    }
    if modes.mouse_sgr {
        let end = if matches!(ev, MouseEvent::Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    // 旧来の形式は座標が 223 までしか送れない
    let enc = |v: usize| (32 + 1 + v).min(255) as u8;
    Some(vec![
        0x1b,
        b'[',
        b'M',
        (32 + code).min(255) as u8,
        enc(col),
        enc(row),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Modes {
        Modes::default()
    }

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };
    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
    };

    #[test]
    fn cursor_keys() {
        assert_eq!(encode(Key::Up, NONE, &m()), b"\x1b[A");
        let app = Modes {
            app_cursor: true,
            ..m()
        };
        assert_eq!(encode(Key::Up, NONE, &app), b"\x1bOA");
        assert_eq!(encode(Key::Left, CTRL, &app), b"\x1b[1;5D");
        let shift = Mods {
            shift: true,
            ..NONE
        };
        assert_eq!(encode(Key::End, shift, &m()), b"\x1b[1;2F");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(encode(Key::Delete, NONE, &m()), b"\x1b[3~");
        assert_eq!(encode(Key::PageDown, CTRL, &m()), b"\x1b[6;5~");
        assert_eq!(encode(Key::F(1), NONE, &m()), b"\x1bOP");
        assert_eq!(encode(Key::F(5), NONE, &m()), b"\x1b[15~");
        assert_eq!(encode(Key::F(12), NONE, &m()), b"\x1b[24~");
        assert_eq!(encode(Key::Backspace, NONE, &m()), [0x7f]);
        let shift = Mods {
            shift: true,
            ..NONE
        };
        assert_eq!(encode(Key::Tab, shift, &m()), b"\x1b[Z");
    }

    #[test]
    fn characters() {
        assert_eq!(encode(Key::Char('c'), CTRL, &m()), [3]);
        assert_eq!(encode(Key::Char(' '), CTRL, &m()), [0]);
        let alt = Mods { alt: true, ..NONE };
        assert_eq!(encode(Key::Char('x'), alt, &m()), b"\x1bx");
        assert_eq!(encode(Key::Char('あ'), NONE, &m()), "あ".as_bytes());
    }

    #[test]
    fn pasting() {
        assert_eq!(paste("a\r\nb\nc", &m()), b"a\rb\rc");
        let br = Modes {
            bracketed_paste: true,
            ..m()
        };
        assert_eq!(paste("x\x1b[201~y", &br), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn mouse_reports() {
        assert_eq!(
            mouse(MouseEvent::Press(Button::Left), 0, 0, NONE, &m()),
            None
        );
        let normal = Modes {
            mouse: MouseMode::Normal,
            ..m()
        };
        assert_eq!(
            mouse(MouseEvent::Press(Button::Left), 2, 3, NONE, &normal).unwrap(),
            [0x1b, b'[', b'M', 32, 35, 36]
        );
        assert_eq!(
            mouse(MouseEvent::Move(Some(Button::Left)), 0, 0, NONE, &normal),
            None
        );
        let sgr = Modes {
            mouse: MouseMode::ButtonMotion,
            mouse_sgr: true,
            ..m()
        };
        assert_eq!(
            mouse(MouseEvent::Release(Button::Right), 9, 4, CTRL, &sgr).unwrap(),
            b"\x1b[<18;10;5m"
        );
        assert_eq!(
            mouse(MouseEvent::Move(Some(Button::Left)), 0, 0, NONE, &sgr).unwrap(),
            b"\x1b[<32;1;1M"
        );
        assert_eq!(
            mouse(MouseEvent::Press(Button::WheelUp), 0, 0, NONE, &sgr).unwrap(),
            b"\x1b[<64;1;1M"
        );
    }
}
