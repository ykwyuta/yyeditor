use crate::DbKind;

/// SQL lexical splitter: quoted strings/identifiers, nested comments and dollar quotes.
pub fn statements(sql: &str, kind: DbKind) -> crate::Result<Vec<String>> {
    let bytes = sql.as_bytes();
    let mut out = vec![];
    let mut start = 0;
    let mut i = 0;
    let mut quote = 0;
    let mut comment_depth = 0;
    let mut line_comment = false;
    let mut dollar: Option<Vec<u8>> = None;
    while i < bytes.len() {
        if line_comment {
            if bytes[i] == b'\n' {
                line_comment = false;
            }
            i += 1;
            continue;
        }
        if comment_depth > 0 {
            if bytes[i..].starts_with(b"/*") {
                comment_depth += 1;
                i += 2;
            } else if bytes[i..].starts_with(b"*/") {
                comment_depth -= 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(tag) = dollar.as_ref() {
            if bytes[i..].starts_with(tag) {
                i += tag.len();
                dollar = None;
            } else {
                i += 1;
            }
            continue;
        }
        if quote != 0 {
            let end = if quote == b'[' { b']' } else { quote };
            if bytes[i] == end {
                if bytes.get(i + 1) == Some(&end) {
                    i += 2;
                    continue;
                }
                quote = 0;
            } else if bytes[i] == b'\\' && kind == DbKind::MySql {
                i += 1;
            }
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"--") {
            line_comment = true;
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            comment_depth = 1;
            i += 2;
            continue;
        }
        if matches!(bytes[i], b'\'' | b'"' | b'`')
            || (bytes[i] == b'[' && kind == DbKind::SqlServer)
        {
            quote = bytes[i];
            i += 1;
            continue;
        }
        if bytes[i] == b'$' && matches!(kind, DbKind::PostgreSql | DbKind::DuckDb) {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if bytes.get(end) == Some(&b'$') {
                dollar = Some(bytes[i..=end].to_vec());
                i = end + 1;
                continue;
            }
        }
        if bytes[i] == b';' {
            if !sql[start..i].trim().is_empty() {
                out.push(sql[start..i].trim().to_owned());
            }
            start = i + 1;
        }
        i += 1;
    }
    if quote != 0 || dollar.is_some() || comment_depth > 0 {
        return Err("SQLの引用符またはコメントが閉じていません".into());
    }
    if !sql[start..].trim().is_empty() {
        out.push(sql[start..].trim().to_owned());
    }
    Ok(out)
}

pub fn identifier(name: &str) -> crate::Result<String> {
    if name.is_empty() || name.contains('\0') {
        return Err("空または不正な識別子です".into());
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}
pub fn identifier_for(kind: DbKind, name: &str) -> crate::Result<String> {
    if kind == DbKind::MySql {
        if name.is_empty() || name.contains('\0') {
            return Err("不正な識別子です".into());
        }
        Ok(format!("`{}`", name.replace('`', "``")))
    } else {
        identifier(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_semicolons_comments_and_dollar_strings() {
        let s = "select ';'; /* a; /* b */ */ select $$x;y$$; select \"a;b\"";
        assert_eq!(statements(s, DbKind::PostgreSql).unwrap().len(), 3);
        assert!(statements("select 'x", DbKind::Sqlite).is_err());
        assert_eq!(identifier("a\"b").unwrap(), "\"a\"\"b\"");
    }
}
