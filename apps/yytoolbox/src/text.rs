use crate::{Output, Params, Result, checkpoint, integer, value};
use data_encoding::{BASE64, BASE64_NOPAD, BASE64URL, BASE64URL_NOPAD};
use sha2::Digest;

pub(crate) fn regex(input: &str, params: &Params, cancelled: &impl Fn() -> bool) -> Result<Output> {
    let mut builder = regex::RegexBuilder::new(value(params, "pattern", ""));
    builder
        .size_limit(2 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024);
    for flag in value(params, "flags", "")
        .chars()
        .filter(|c| !c.is_whitespace())
    {
        match flag {
            'i' => builder.case_insensitive(true),
            'm' => builder.multi_line(true),
            's' => builder.dot_matches_new_line(true),
            'x' => builder.ignore_whitespace(true),
            'U' => builder.swap_greed(true),
            _ => return Err(format!("未対応のフラグ: {flag}")),
        };
    }
    let re = builder.build().map_err(|e| e.to_string())?;
    let names: Vec<_> = re.capture_names().collect();
    let mut rows = Vec::new();
    let mut truncated = false;
    let mut budget = 0usize;
    for (index, captures) in re.captures_iter(input).enumerate() {
        checkpoint(cancelled)?;
        if index >= 10000 {
            truncated = true;
            break;
        }
        for (i, group) in captures.iter().enumerate() {
            budget = budget
                .saturating_add(group.map_or(0, |m| m.len()) + 128 + names[i].map_or(0, str::len));
            if budget > crate::MAX_OUTPUT {
                return Err("正規表現の結果が16 MiBを超えました".into());
            }
        }
        let groups: Vec<_> = captures.iter().enumerate().map(|(i, capture)| {
            match capture { Some(m) => serde_json::json!({"group":i,"name":names[i],"start_byte":m.start(),"end_byte":m.end(),"text":m.as_str()}), None => serde_json::json!({"group":i,"name":names[i],"text":null}) }
        }).collect();
        rows.push(serde_json::json!({"match":index+1,"groups":groups}));
    }
    let notice = format!(
        "一致 {} 件{}。位置はUTF-8バイトです",
        rows.len(),
        if truncated {
            "（10000件で打ち切り）"
        } else {
            ""
        }
    );
    Ok(Output::text(
        serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?,
        "matches.json",
        notice,
    ))
}

pub(crate) fn base64(input: &str, params: &Params, decode: bool) -> Result<Output> {
    let url = value(params, "alphabet", "標準") == "URL-safe";
    if !decode {
        return Ok(Output::text(
            if url {
                BASE64URL_NOPAD.encode(input.as_bytes())
            } else {
                BASE64.encode(input.as_bytes())
            },
            "encoded.txt",
            "UTF-8をエンコードしました",
        ));
    }
    let clean: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let encoding = match (url, clean.ends_with('=')) {
        (true, true) => BASE64URL,
        (true, false) => BASE64URL_NOPAD,
        (false, true) => BASE64,
        (false, false) => BASE64_NOPAD,
    };
    let bytes = encoding
        .decode(clean.as_bytes())
        .map_err(|e| format!("Base64: {e}"))?;
    let (text, filename, notice) = match String::from_utf8(bytes.clone()) {
        Ok(text) => (text, "decoded.txt", "UTF-8としてデコードしました"),
        Err(_) => (
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            "decoded.bin",
            "UTF-8ではないため16進表示。保存時は元のバイト列を保存します",
        ),
    };
    Ok(Output {
        qr: None,
        text,
        bytes,
        filename,
        notice: notice.into(),
    })
}

pub(crate) fn hash(input: &str, params: &Params) -> Result<Output> {
    let b = input.as_bytes();
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let all = [
        ("MD5", format!("{:x}", md5::compute(b))),
        ("SHA-1", hex(&sha1::Sha1::digest(b))),
        ("SHA-224", hex(&sha2::Sha224::digest(b))),
        ("SHA-256", hex(&sha2::Sha256::digest(b))),
        ("SHA-384", hex(&sha2::Sha384::digest(b))),
        ("SHA-512", hex(&sha2::Sha512::digest(b))),
        ("BLAKE3", blake3::hash(b).to_hex().to_string()),
        ("CRC32", format!("{:08x}", crc32fast::hash(b))),
    ];
    let chosen = value(params, "algorithm", "すべて");
    let text = all
        .iter()
        .filter(|(name, _)| chosen == "すべて" || *name == chosen)
        .map(|(name, hash)| format!("{name}: {hash}\n"))
        .collect::<String>();
    if text.is_empty() {
        return Err("未対応のハッシュです".into());
    }
    Ok(Output::text(
        text,
        "hashes.txt",
        format!("UTF-8 {} バイトのハッシュ", b.len()),
    ))
}

pub(crate) fn jwt(input: &str) -> Result<Output> {
    let parts: Vec<_> = input.trim().split('.').collect();
    if parts.len() != 3 {
        return Err("JWS形式の3区画JWTを指定してください。暗号化されたJWEは対象外です".into());
    }
    let decode = |part: &str| -> Result<serde_json::Value> {
        let bytes = if part.ends_with('=') {
            BASE64URL.decode(part.as_bytes())
        } else {
            BASE64URL_NOPAD.decode(part.as_bytes())
        }
        .map_err(|e| format!("Base64URL: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("JWT内のJSON: {e}"))
    };
    let header = decode(parts[0])?;
    let payload = decode(parts[1])?;
    if !header.is_object() || !payload.is_object() {
        return Err("ヘッダーとペイロードはJSONオブジェクトである必要があります".into());
    }
    if !parts[2].is_empty() {
        BASE64URL_NOPAD
            .decode(parts[2].as_bytes())
            .map_err(|e| format!("署名区画のBase64URL: {e}"))?;
    }
    let result = serde_json::json!({"signature_verified":false,"header":header,"payload":payload});
    Ok(Output::text(
        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?,
        "jwt-decoded.json",
        "デコードのみ。署名・発行者・有効期限は未検証です",
    ))
}

pub(crate) fn json(input: &str, params: &Params) -> Result<Output> {
    use serde::Deserialize;
    let mut deserializer = serde_json::Deserializer::from_str(input);
    serde::de::IgnoredAny::deserialize(&mut deserializer).map_err(|e| e.to_string())?;
    deserializer.end().map_err(|e| e.to_string())?;
    let mut tokens = Vec::new();
    let mut indices = input.char_indices().peekable();
    while let Some((start, c)) = indices.next() {
        if c.is_ascii_whitespace() {
            continue;
        }
        let mut end = start + c.len_utf8();
        if c == '"' {
            let mut escaped = false;
            for (at, next) in indices.by_ref() {
                end = at + next.len_utf8();
                if next == '"' && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
            }
        } else if !"{}[],:".contains(c) {
            while let Some(&(at, next)) = indices.peek() {
                if next.is_ascii_whitespace() || "{}[],:".contains(next) {
                    break;
                }
                indices.next();
                end = at + next.len_utf8();
            }
        }
        tokens.push(&input[start..end]);
    }
    if value(params, "layout", "整形") == "圧縮" {
        return Ok(Output::text(
            tokens.concat(),
            "formatted.json",
            "空白を圧縮しました",
        ));
    }
    let spaces = integer(params, "indent", 1, 8)? as usize;
    let mut out = String::new();
    let mut depth = 0usize;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&" ".repeat(depth * spaces));
    };
    for (i, token) in tokens.iter().enumerate() {
        match *token {
            "{" | "[" => {
                out.push_str(token);
                depth += 1;
                if !tokens.get(i + 1).is_some_and(|s| matches!(*s, "}" | "]")) {
                    newline(&mut out, depth);
                }
            }
            "}" | "]" => {
                depth -= 1;
                if !tokens
                    .get(i.wrapping_sub(1))
                    .is_some_and(|s| matches!(*s, "{" | "["))
                {
                    newline(&mut out, depth);
                }
                out.push_str(token);
            }
            "," => {
                out.push(',');
                newline(&mut out, depth);
            }
            ":" => out.push_str(": "),
            _ => out.push_str(token),
        }
        if out.len() > crate::MAX_OUTPUT {
            return Err("JSONの出力が16 MiBを超えました".into());
        }
    }
    Ok(Output::text(
        out,
        "formatted.json",
        "文字列・数値表記・キー順序を維持して整形しました",
    ))
}

pub(crate) fn sql(input: &str, params: &Params) -> Result<Output> {
    if input.len() > 128 * 1024 {
        return Err("SQL整形の入力は128 KiBまでです".into());
    }
    let mut depth = 0usize;
    for c in input.chars() {
        if c == '(' {
            depth += 1;
            if depth > 64 {
                return Err("SQLの丸括弧は64階層までです（複雑度の保守的な計数）".into());
            }
        } else if c == ')' {
            depth = depth.saturating_sub(1);
        }
    }
    let dialect = match value(params, "dialect", "Generic") {
        "Generic" => sqlformat::Dialect::Generic,
        "PostgreSQL" => sqlformat::Dialect::PostgreSql,
        "SQL Server" => sqlformat::Dialect::SQLServer,
        _ => return Err("未対応のSQL方言です".into()),
    };
    let uppercase = match value(params, "case", "変更しない") {
        "大文字" => Some(true),
        "小文字" => Some(false),
        _ => None,
    };
    let options = sqlformat::FormatOptions {
        uppercase,
        dialect,
        ..Default::default()
    };
    Ok(Output::text(
        sqlformat::format(input, &sqlformat::QueryParams::None, &options),
        "formatted.sql",
        "整形のみ。SQLの実行・完全な構文検証は行いません",
    ))
}
