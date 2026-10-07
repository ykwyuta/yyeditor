//! 証明書（X.509 の DER）から表示用の情報を読む。
//!
//! 検証は rustls（webpki）がする。ここでは利用者に見せる主体・発行者・名前・有効期間・指紋だけを、
//! DER を最小限たどって取り出す（読めない部分は空のまま）。

use sha2::{Digest, Sha256};

/// 証明書の表示用の情報。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CertInfo {
    /// 主体（`CN=…, O=…`）
    pub subject: String,
    /// 発行者
    pub issuer: String,
    /// 別名（subjectAltName の DNS 名・IP アドレス）
    pub names: Vec<String>,
    /// 有効期間の始め（`YYYY-MM-DD hh:mm:ss UTC`）
    pub not_before: String,
    /// 有効期間の終わり
    pub not_after: String,
    /// SHA-256 の指紋（`AB:CD:…`）
    pub sha256: String,
}

impl CertInfo {
    /// DER の証明書から読む。
    pub fn from_der(der: &[u8]) -> CertInfo {
        let mut info = CertInfo {
            sha256: fingerprint(der),
            ..CertInfo::default()
        };
        let _ = read_tbs(der, &mut info);
        info
    }

    /// 自己署名（主体と発行者が同じ）か。
    pub fn self_signed(&self) -> bool {
        !self.subject.is_empty() && self.subject == self.issuer
    }

    /// 複数行の説明（ダイアログ・記録用）。
    pub fn describe(&self) -> String {
        let mut s = format!("主体: {}\n発行者: {}\n", self.subject, self.issuer);
        if !self.names.is_empty() {
            s.push_str(&format!("別名: {}\n", self.names.join(", ")));
        }
        s.push_str(&format!(
            "有効期間: {} ～ {}\nSHA-256 の指紋: {}",
            self.not_before, self.not_after, self.sha256
        ));
        s
    }
}

/// SHA-256 の指紋（大文字の 16 進をコロンで区切る）。
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// DER の 1 つの要素（タグ・中身・残り）。
fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = b.split_first()?;
    let (&l, rest) = rest.split_first()?;
    let (len, rest) = if l < 0x80 {
        (l as usize, rest)
    } else {
        let n = (l & 0x7F) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |a, &x| (a << 8) | x as usize);
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    Some((tag, &rest[..len], &rest[len..]))
}

fn read_tbs(der: &[u8], info: &mut CertInfo) -> Option<()> {
    let (_, cert, _) = tlv(der)?;
    let (_, tbs, _) = tlv(cert)?;
    let mut t = tbs;
    let (tag, _, rest) = tlv(t)?;
    if tag == 0xA0 {
        t = rest; // version
    }
    let (_, _, t) = tlv(t)?; // serialNumber
    let (_, _, t) = tlv(t)?; // signature
    let (_, issuer, t) = tlv(t)?;
    info.issuer = name(issuer);
    let (_, validity, t) = tlv(t)?;
    let (tag, nb, v) = tlv(validity)?;
    info.not_before = time(tag, nb);
    let (tag, na, _) = tlv(v)?;
    info.not_after = time(tag, na);
    let (_, subject, t) = tlv(t)?;
    info.subject = name(subject);
    let (_, _, mut t) = tlv(t)?; // subjectPublicKeyInfo
    while let Some((tag, content, rest)) = tlv(t) {
        if tag == 0xA3 {
            info.names = alt_names(content).unwrap_or_default();
        }
        t = rest;
    }
    Some(())
}

/// 識別名（`CN=…, O=…`。証明書の中の順）。
fn name(b: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut sets = b;
    while let Some((_, set, rest)) = tlv(sets) {
        let mut atvs = set;
        while let Some((_, atv, r)) = tlv(atvs) {
            if let Some((0x06, oid, v)) = tlv(atv)
                && let Some((tag, value, _)) = tlv(v)
            {
                parts.push(format!("{}={}", oid_name(oid), string(tag, value)));
            }
            atvs = r;
        }
        sets = rest;
    }
    parts.join(", ")
}

fn oid_name(oid: &[u8]) -> String {
    match oid {
        [0x55, 0x04, 0x03] => "CN".into(),
        [0x55, 0x04, 0x05] => "SERIALNUMBER".into(),
        [0x55, 0x04, 0x06] => "C".into(),
        [0x55, 0x04, 0x07] => "L".into(),
        [0x55, 0x04, 0x08] => "ST".into(),
        [0x55, 0x04, 0x0A] => "O".into(),
        [0x55, 0x04, 0x0B] => "OU".into(),
        [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x01] => "E".into(),
        _ => dotted(oid),
    }
}

fn dotted(oid: &[u8]) -> String {
    let Some((&first, rest)) = oid.split_first() else {
        return String::new();
    };
    let mut out = vec![(first / 40) as u64, (first % 40) as u64];
    let mut v = 0u64;
    for &b in rest {
        v = (v << 7) | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            out.push(v);
            v = 0;
        }
    }
    out.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

fn string(tag: u8, b: &[u8]) -> String {
    match tag {
        // BMPString
        0x1E => char::decode_utf16(b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])))
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect(),
        // T61String・Latin-1 とみなす
        0x14 => b.iter().map(|&c| c as char).collect(),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

fn time(tag: u8, b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_end_matches('Z');
    let full = match tag {
        // UTCTime（YYMMDDhhmmss）
        0x17 if s.len() >= 12 => {
            let yy: u32 = s[..2].parse().unwrap_or(0);
            format!("{}{s}", if yy < 50 { "20" } else { "19" })
        }
        // GeneralizedTime（YYYYMMDDhhmmss）
        0x18 if s.len() >= 14 => s.to_string(),
        _ => return s.to_string(),
    };
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &full[..4],
        &full[4..6],
        &full[6..8],
        &full[8..10],
        &full[10..12],
        &full[12..14]
    )
}

/// 拡張（`[3]`）の subjectAltName。
fn alt_names(ext: &[u8]) -> Option<Vec<String>> {
    let (_, list, _) = tlv(ext)?;
    let mut l = list;
    while let Some((_, e, rest)) = tlv(l) {
        if let Some((0x06, [0x55, 0x1D, 0x11], v)) = tlv(e) {
            let mut v = v;
            // critical（BOOLEAN）があれば飛ばす
            if let Some((0x01, _, r)) = tlv(v) {
                v = r;
            }
            let (_, octets, _) = tlv(v)?;
            let (_, names, _) = tlv(octets)?;
            let mut out = Vec::new();
            let mut n = names;
            while let Some((tag, value, r)) = tlv(n) {
                match tag {
                    0x82 => out.push(String::from_utf8_lossy(value).into_owned()),
                    0x87 if value.len() == 4 => out.push(
                        value
                            .iter()
                            .map(u8::to_string)
                            .collect::<Vec<_>>()
                            .join("."),
                    ),
                    0x87 if value.len() == 16 => out.push(
                        std::net::Ipv6Addr::from(<[u8; 16]>::try_from(value).ok()?).to_string(),
                    ),
                    _ => {}
                }
                n = r;
            }
            return Some(out);
        }
        l = rest;
    }
    None
}
