//! 3270 と Telnet の符号（コマンド・オーダー・AID・属性）とバッファのアドレス。

// ---- Telnet ----------------------------------------------------------------------

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const IP: u8 = 244;
pub const SE: u8 = 240;
pub const EOR: u8 = 239;

pub const OPT_BINARY: u8 = 0;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_EOR: u8 = 25;
pub const OPT_TN3270E: u8 = 40;

pub const TTYPE_IS: u8 = 0;
pub const TTYPE_SEND: u8 = 1;

// TN3270E のサブネゴシエーション（RFC 2355）
pub const E_ASSOCIATE: u8 = 0;
pub const E_CONNECT: u8 = 1;
pub const E_DEVICE_TYPE: u8 = 2;
pub const E_FUNCTIONS: u8 = 3;
pub const E_IS: u8 = 4;
pub const E_REASON: u8 = 5;
pub const E_REJECT: u8 = 6;
pub const E_REQUEST: u8 = 7;
pub const E_SEND: u8 = 8;

// TN3270E の関数
pub const FN_BIND_IMAGE: u8 = 0;
pub const FN_DATA_STREAM_CTL: u8 = 1;
pub const FN_RESPONSES: u8 = 2;
pub const FN_SCS_CTL_CODES: u8 = 3;
pub const FN_SYSREQ: u8 = 4;

// TN3270E のヘッダーのデータの種類
pub const DT_3270_DATA: u8 = 0;
pub const DT_SCS_DATA: u8 = 1;
pub const DT_RESPONSE: u8 = 2;
pub const DT_BIND_IMAGE: u8 = 3;
pub const DT_UNBIND: u8 = 4;
pub const DT_NVT_DATA: u8 = 5;
pub const DT_REQUEST: u8 = 6;
pub const DT_SSCP_LU_DATA: u8 = 7;
pub const DT_PRINT_EOJ: u8 = 8;

// ヘッダーの応答のフラグ（ホストからの 3270-DATA）
pub const RSP_NO_RESPONSE: u8 = 0;
pub const RSP_ERROR_RESPONSE: u8 = 1;
pub const RSP_ALWAYS_RESPONSE: u8 = 2;
// 端末からの RESPONSE
pub const RSP_POSITIVE: u8 = 0;
pub const RSP_NEGATIVE: u8 = 1;

/// TN3270E の DEVICE-TYPE REJECT の理由の説明。
pub fn reason_text(code: u8) -> &'static str {
    match code {
        0 => "CONN-PARTNER（端末の LU に対応するプリンターがありません）",
        1 => "DEVICE-IN-USE（その LU は使用中です）",
        2 => "INV-ASSOCIATE（対応づける LU が正しくありません）",
        3 => "INV-NAME（その LU 名はありません）",
        4 => "INV-DEVICE-TYPE（その端末の種類は使えません）",
        5 => "TYPE-NAME-ERROR（端末の種類と LU が合いません）",
        6 => "UNKNOWN-ERROR",
        7 => "UNSUPPORTED-REQ（その要求には対応していません）",
        _ => "（不明な理由）",
    }
}

// ---- 3270 のコマンド ---------------------------------------------------------------

/// ホストからのコマンド（SNA と非 SNA のどちらの符号も受け付ける）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Write,
    EraseWrite,
    EraseWriteAlternate,
    ReadBuffer,
    ReadModified,
    ReadModifiedAll,
    EraseAllUnprotected,
    WriteStructuredField,
}

impl Command {
    pub fn from_byte(b: u8) -> Option<Command> {
        Some(match b {
            0xF1 | 0x01 => Command::Write,
            0xF5 | 0x05 => Command::EraseWrite,
            0x7E | 0x0D => Command::EraseWriteAlternate,
            0xF2 | 0x02 => Command::ReadBuffer,
            0xF6 | 0x06 => Command::ReadModified,
            0x6E | 0x0E => Command::ReadModifiedAll,
            0x6F | 0x0F => Command::EraseAllUnprotected,
            0xF3 | 0x11 => Command::WriteStructuredField,
            _ => return None,
        })
    }
}

// WCC（Write Control Character）
pub const WCC_RESET: u8 = 0x40;
pub const WCC_START_PRINTER: u8 = 0x08;
pub const WCC_ALARM: u8 = 0x04;
pub const WCC_RESTORE: u8 = 0x02;
pub const WCC_RESET_MDT: u8 = 0x01;

// オーダー
pub const ORDER_PT: u8 = 0x05;
pub const ORDER_GE: u8 = 0x08;
pub const ORDER_SBA: u8 = 0x11;
pub const ORDER_EUA: u8 = 0x12;
pub const ORDER_IC: u8 = 0x13;
pub const ORDER_SF: u8 = 0x1D;
pub const ORDER_SA: u8 = 0x28;
pub const ORDER_SFE: u8 = 0x29;
pub const ORDER_MF: u8 = 0x2C;
pub const ORDER_RA: u8 = 0x3C;

// 書式の制御の文字（画面には空白として出る）
pub const FC_NULL: u8 = 0x00;
pub const FC_FF: u8 = 0x0C;
pub const FC_CR: u8 = 0x0D;
pub const FC_SO: u8 = 0x0E;
pub const FC_SI: u8 = 0x0F;
pub const FC_NL: u8 = 0x15;
pub const FC_EM: u8 = 0x19;
pub const FC_DUP: u8 = 0x1C;
pub const FC_FM: u8 = 0x1E;
pub const FC_SUB: u8 = 0x3F;
pub const FC_EO: u8 = 0xFF;

// フィールド属性（下位 6 ビットの意味）
pub const FA_PROTECT: u8 = 0x20;
pub const FA_NUMERIC: u8 = 0x10;
pub const FA_DISPLAY_MASK: u8 = 0x0C;
pub const FA_INTENSIFIED: u8 = 0x08;
pub const FA_NONDISPLAY: u8 = 0x0C;
pub const FA_DETECTABLE: u8 = 0x04;
pub const FA_MDT: u8 = 0x01;

// 拡張属性の種類（SFE・SA・MF）
pub const XA_ALL: u8 = 0x00;
pub const XA_3270: u8 = 0xC0;
pub const XA_VALIDATION: u8 = 0xC1;
pub const XA_OUTLINING: u8 = 0xC2;
pub const XA_HIGHLIGHTING: u8 = 0x41;
pub const XA_FOREGROUND: u8 = 0x42;
pub const XA_CHARSET: u8 = 0x43;
pub const XA_BACKGROUND: u8 = 0x45;
pub const XA_TRANSPARENCY: u8 = 0x46;

// 強調
pub const HL_DEFAULT: u8 = 0x00;
pub const HL_NORMAL: u8 = 0xF0;
pub const HL_BLINK: u8 = 0xF1;
pub const HL_REVERSE: u8 = 0xF2;
pub const HL_UNDERSCORE: u8 = 0xF4;

/// 2 バイト文字（DBCS）の文字セット
pub const CS_DBCS: u8 = 0xF8;

// ---- AID --------------------------------------------------------------------------

pub const AID_NONE: u8 = 0x60;
pub const AID_ENTER: u8 = 0x7D;
pub const AID_CLEAR: u8 = 0x6D;
pub const AID_PA1: u8 = 0x6C;
pub const AID_PA2: u8 = 0x6E;
pub const AID_PA3: u8 = 0x6B;
pub const AID_SYSREQ: u8 = 0xF0;
pub const AID_SF: u8 = 0x88;
pub const AID_SELECT: u8 = 0x7E;

/// PF1〜PF24 の AID。
pub fn pf_aid(n: u8) -> Option<u8> {
    const PF: [u8; 24] = [
        0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0xC1, 0xC2, 0xC3,
        0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C,
    ];
    PF.get(usize::from(n).checked_sub(1)?).copied()
}

/// PA・Clear（短い読み取り。フィールドを送らない）か。
pub fn is_short_read(aid: u8) -> bool {
    matches!(aid, AID_CLEAR | AID_PA1 | AID_PA2 | AID_PA3 | AID_SYSREQ)
}

// ---- Structured Field ----------------------------------------------------------------

pub const SF_READ_PARTITION: u8 = 0x01;
pub const SF_ERASE_RESET: u8 = 0x03;
pub const SF_SET_REPLY_MODE: u8 = 0x09;
pub const SF_OUTBOUND_3270DS: u8 = 0x40;
pub const SF_DATA_CHUNK: u8 = 0xD0;

pub const RP_QUERY: u8 = 0x02;
pub const RP_QUERY_LIST: u8 = 0x03;
pub const RP_RMA: u8 = 0x6E;
pub const RP_RB: u8 = 0xF2;
pub const RP_RM: u8 = 0xF6;

// Query Reply の種類
pub const QR_SUMMARY: u8 = 0x80;
pub const QR_USABLE_AREA: u8 = 0x81;
pub const QR_ALPHA_PARTITIONS: u8 = 0x84;
pub const QR_CHARSETS: u8 = 0x85;
pub const QR_COLOR: u8 = 0x86;
pub const QR_HIGHLIGHTING: u8 = 0x87;
pub const QR_REPLY_MODES: u8 = 0x88;
pub const QR_DBCS_ASIA: u8 = 0x91;
pub const QR_DDM: u8 = 0x95;
pub const QR_RPQ_NAMES: u8 = 0xA1;
pub const QR_IMPLICIT_PART: u8 = 0xA6;
pub const QR_NULL: u8 = 0xFF;

// ---- バッファのアドレス ------------------------------------------------------------

/// 12 ビットのアドレスの 6 ビットずつの符号。
const CODE_TABLE: [u8; 64] = [
    0x40, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
];

/// 2 バイトのアドレスを読む（上位 2 ビットが 00 なら 14 ビット、でなければ 12 ビット）。
pub fn decode_address(b1: u8, b2: u8) -> usize {
    if b1 & 0xC0 == 0 {
        (usize::from(b1 & 0x3F) << 8) | usize::from(b2)
    } else {
        (usize::from(b1 & 0x3F) << 6) | usize::from(b2 & 0x3F)
    }
}

/// アドレスを 2 バイトにする（4096 未満は 12 ビット、それ以上は 14 ビット）。
pub fn encode_address(addr: usize) -> [u8; 2] {
    if addr < 4096 {
        [CODE_TABLE[(addr >> 6) & 0x3F], CODE_TABLE[addr & 0x3F]]
    } else {
        [((addr >> 8) & 0x3F) as u8, (addr & 0xFF) as u8]
    }
}

/// フィールド属性の値を、送るときの符号（グラフィックの文字）にする。
pub fn encode_fa(fa: u8) -> u8 {
    CODE_TABLE[usize::from(fa & 0x3F)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        for a in [0, 1, 63, 64, 79, 80, 1919, 3563, 4095, 4096, 10000] {
            let [b1, b2] = encode_address(a);
            assert_eq!(decode_address(b1, b2), a, "{a}");
        }
        // よく見る値: 行 1 桁 1（アドレス 0）は 40 40、行 2 桁 1（80）は C1 50
        assert_eq!(encode_address(0), [0x40, 0x40]);
        assert_eq!(encode_address(80), [0xC1, 0x50]);
        assert_eq!(pf_aid(1), Some(0xF1));
        assert_eq!(pf_aid(12), Some(0x7C));
        assert_eq!(pf_aid(24), Some(0x4C));
        assert_eq!(pf_aid(25), None);
        assert_eq!(pf_aid(0), None);
    }
}
