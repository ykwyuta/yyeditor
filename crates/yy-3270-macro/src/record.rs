//! 操作の記録（14 章 13.4）: 入力した文字・押したキー・カーソルの移動を Rhai のスクリプトにする。
//!
//! - 続けて入力した文字は 1 つの `type("…")` にまとめる。
//! - AID のキー（Enter・PF・PA・Clear など）のあとには `wait_unlocked()` を入れる。
//! - 非表示のフィールド（パスワード）への入力は記録せず、`password("名前")` に置き換える
//!   （名前は記録の終わりに決める）。

use yy_3270::Key;

/// 記録中の操作。
#[derive(Debug, Default)]
pub struct Recorder {
    lines: Vec<String>,
    text: String,
    /// 今のフィールドが非表示で、すでに `password` を入れた
    in_password: bool,
    passwords: usize,
}

/// `password` の名前を後で入れる印
const PASSWORD_MARK: &str = "\u{0}PASSWORD\u{0}";

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// キーの名前（`key("…")` に書くもの）。文字・カーソルの移動は `None`。
pub fn key_name(k: &Key) -> Option<String> {
    Some(match k {
        Key::Enter => "Enter".into(),
        Key::Pf(n) => format!("PF{n}"),
        Key::Pa(n) => format!("PA{n}"),
        Key::Clear => "Clear".into(),
        Key::SysReq => "SysReq".into(),
        Key::Attn => "Attn".into(),
        Key::Reset => "Reset".into(),
        Key::Tab => "Tab".into(),
        Key::BackTab => "BackTab".into(),
        Key::Home => "Home".into(),
        Key::NewLine => "NewLine".into(),
        Key::Up => "Up".into(),
        Key::Down => "Down".into(),
        Key::Left => "Left".into(),
        Key::Right => "Right".into(),
        Key::Backspace => "Backspace".into(),
        Key::Delete => "Delete".into(),
        Key::EraseEof => "EraseEOF".into(),
        Key::EraseInput => "EraseInput".into(),
        Key::Insert => "Insert".into(),
        Key::Dup => "Dup".into(),
        Key::FieldMark => "FieldMark".into(),
        Key::Char(_) | Key::MoveTo(_) => return None,
    })
}

/// ホストへ送るキー（押したあとホストの応答を待つ）。
fn is_aid(k: &Key) -> bool {
    matches!(
        k,
        Key::Enter | Key::Pf(_) | Key::Pa(_) | Key::Clear | Key::SysReq | Key::Attn
    )
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder::default()
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let t = std::mem::take(&mut self.text);
            self.lines.push(format!("type({});", quote(&t)));
        }
    }

    /// 操作を 1 つ記録する。`hidden` はカーソルのあるフィールドが非表示か、`cols` は画面の桁数。
    pub fn key(&mut self, k: &Key, hidden: bool, cols: usize) {
        match k {
            Key::Char(c) => {
                if hidden {
                    self.flush_text();
                    if !self.in_password {
                        self.in_password = true;
                        self.passwords += 1;
                        self.lines.push(format!("password({PASSWORD_MARK});"));
                    }
                } else {
                    self.in_password = false;
                    self.text.push(*c);
                }
                return;
            }
            Key::MoveTo(a) => {
                self.flush_text();
                let cols = cols.max(1);
                self.lines
                    .push(format!("move_to({}, {});", a / cols + 1, a % cols + 1));
            }
            other => {
                self.flush_text();
                if let Some(name) = key_name(other) {
                    self.lines.push(format!("key({});", quote(&name)));
                }
                if is_aid(other) {
                    self.lines.push("wait_unlocked();".into());
                }
            }
        }
        self.in_password = false;
    }

    /// 貼り付けた文字。
    pub fn paste(&mut self, text: &str) {
        self.text.push_str(text);
        self.in_password = false;
    }

    /// パスワードの入力を記録したか（記録の終わりに資格情報の名前を尋ねる）。
    pub fn has_password(&self) -> bool {
        self.passwords > 0
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.text.is_empty()
    }

    /// スクリプトにする。`password_name` は `password("…")` に入れる資格情報の名前。
    pub fn finish(mut self, title: &str, password_name: &str) -> String {
        self.flush_text();
        let mut out = format!(
            "// {title}\n// 記録したマクロ。待つ文字（wait_text）や繰り返しを書き足して仕上げてください。\n\nwait_unlocked();\n"
        );
        for l in &self.lines {
            out.push_str(&l.replace(PASSWORD_MARK, &quote(password_name)));
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_keys_text_moves_and_passwords() {
        let mut r = Recorder::new();
        assert!(r.is_empty());
        for c in "LOGON \"A\"".chars() {
            r.key(&Key::Char(c), false, 80);
        }
        r.key(&Key::Tab, false, 80);
        for c in "secret".chars() {
            r.key(&Key::Char(c), true, 80);
        }
        r.key(&Key::Enter, false, 80);
        r.key(&Key::MoveTo(161), false, 80);
        r.paste("山田");
        r.key(&Key::Pf(8), false, 80);
        assert!(r.has_password());
        let s = r.finish("ログオン", "社内ホスト");
        let body: Vec<&str> = s.lines().skip(4).collect();
        assert_eq!(
            body,
            vec![
                "type(\"LOGON \\\"A\\\"\");",
                "key(\"Tab\");",
                "password(\"社内ホスト\");",
                "key(\"Enter\");",
                "wait_unlocked();",
                "move_to(3, 2);",
                "type(\"山田\");",
                "key(\"PF8\");",
                "wait_unlocked();",
            ]
        );
        assert!(!s.contains("secret"));
        // 記録したものはそのまま文法として正しい
        crate::check(&s).unwrap();
    }
}
