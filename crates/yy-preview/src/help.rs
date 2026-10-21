//! Shared embedded-help headings and text fallback.

pub fn section_line(md: &str, section: &str) -> Option<usize> {
    let tag = format!("{{#{section}}}");
    md.lines()
        .position(|l| l.starts_with('#') && l.trim_end().ends_with(&tag))
}

pub fn plain_text(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    for line in md.lines() {
        let mut line = line.replace("**", "");
        if line.starts_with('#')
            && let Some(i) = line.rfind(" {#")
        {
            line.truncate(i);
        }
        // [文字](#先) → 文字
        let mut rest = line.as_str();
        let mut plain = String::new();
        while let Some(open) = rest.find('[') {
            let Some(mid) = rest[open..].find("](") else {
                break;
            };
            let Some(close) = rest[open + mid..].find(')') else {
                break;
            };
            plain.push_str(&rest[..open]);
            plain.push_str(&rest[open + 1..open + mid]);
            rest = &rest[open + mid + close + 1..];
        }
        plain.push_str(rest);
        out.push_str(&plain);
        out.push('\n');
    }
    out
}
