//! Telnet・TN3270E・3270 の符号（模擬ホストの側。試験する側の実装は使わない）。

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const AYT: u8 = 246;
pub const AO: u8 = 245;
pub const IP: u8 = 244;
pub const BRK: u8 = 243;
pub const NOP: u8 = 241;
pub const SE: u8 = 240;
pub const EOR: u8 = 239;

pub const OPT_BINARY: u8 = 0;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_EOR: u8 = 25;
pub const OPT_TN3270E: u8 = 40;
pub const OPT_TM: u8 = 6;
pub const TTYPE_IS: u8 = 0;
pub const TTYPE_SEND: u8 = 1;

pub const E_ASSOCIATE: u8 = 0;
pub const E_CONNECT: u8 = 1;
pub const E_DEVICE_TYPE: u8 = 2;
pub const E_FUNCTIONS: u8 = 3;
pub const E_IS: u8 = 4;
pub const E_REASON: u8 = 5;
pub const E_REJECT: u8 = 6;
pub const E_REQUEST: u8 = 7;
pub const E_SEND: u8 = 8;

pub const REASON_DEVICE_IN_USE: u8 = 1;
pub const REASON_INV_ASSOCIATE: u8 = 2;
pub const REASON_INV_NAME: u8 = 3;
pub const REASON_INV_DEVICE_TYPE: u8 = 4;
pub const REASON_TYPE_NAME_ERROR: u8 = 5;

pub const FN_BIND_IMAGE: u8 = 0;
pub const FN_DATA_STREAM_CTL: u8 = 1;
pub const FN_RESPONSES: u8 = 2;
pub const FN_SCS_CTL_CODES: u8 = 3;
pub const FN_SYSREQ: u8 = 4;

pub const DT_3270_DATA: u8 = 0;
pub const DT_SCS_DATA: u8 = 1;
pub const DT_RESPONSE: u8 = 2;
pub const DT_BIND_IMAGE: u8 = 3;
pub const DT_UNBIND: u8 = 4;
pub const DT_NVT_DATA: u8 = 5;
pub const DT_REQUEST: u8 = 6;
pub const DT_SSCP_LU_DATA: u8 = 7;
pub const DT_PRINT_EOJ: u8 = 8;
pub const RSP_NO_RESPONSE: u8 = 0;
pub const RSP_ERROR_RESPONSE: u8 = 1;
pub const RSP_ALWAYS_RESPONSE: u8 = 2;
pub const RSP_POSITIVE: u8 = 0;
pub const RSP_NEGATIVE: u8 = 1;

// コマンド（SNA の符号）
pub const CMD_W: u8 = 0xF1;
pub const CMD_EW: u8 = 0xF5;
pub const CMD_WSF: u8 = 0xF3;
pub const CMD_EWA: u8 = 0x7E;
pub const CMD_RB: u8 = 0xF2;
pub const CMD_RM: u8 = 0xF6;
pub const CMD_RMA: u8 = 0x6E;
pub const CMD_EAU: u8 = 0x6F;

pub const WCC_RESET: u8 = 0x40;
pub const WCC_START_PRINTER: u8 = 0x08;
pub const WCC_ALARM: u8 = 0x04;
pub const WCC_RESTORE: u8 = 0x02;
pub const WCC_RESET_MDT: u8 = 0x01;

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

// 書式の制御の文字
pub const FC_NL: u8 = 0x15;
pub const FC_DUP: u8 = 0x1C;
pub const FC_FM: u8 = 0x1E;

pub const FA_PROTECT: u8 = 0x20;
pub const FA_NUMERIC: u8 = 0x10;
pub const FA_INTENSIFIED: u8 = 0x08;
pub const FA_NONDISPLAY: u8 = 0x0C;
pub const FA_DETECTABLE: u8 = 0x04;
pub const FA_MDT: u8 = 0x01;

pub const XA_ALL: u8 = 0x00;
pub const XA_3270: u8 = 0xC0;
pub const XA_VALIDATION: u8 = 0xC1;
pub const XA_OUTLINING: u8 = 0xC2;
pub const XA_HIGHLIGHTING: u8 = 0x41;
pub const XA_FOREGROUND: u8 = 0x42;
pub const XA_CHARSET: u8 = 0x43;
pub const XA_BACKGROUND: u8 = 0x45;
pub const XA_TRANSPARENCY: u8 = 0x46;
pub const XA_INPUT_CONTROL: u8 = 0xFE;
pub const HL_DEFAULT: u8 = 0x00;
pub const HL_NORMAL: u8 = 0xF0;
pub const HL_BLINK: u8 = 0xF1;
pub const HL_REVERSE: u8 = 0xF2;
pub const HL_UNDERSCORE: u8 = 0xF4;
pub const HL_INTENSIFY: u8 = 0xF8;
pub const CS_APL: u8 = 0xF1;
pub const CS_DBCS: u8 = 0xF8;
pub const COLOR_DEFAULT: u8 = 0x00;
pub const COLOR_NEUTRAL_BLACK: u8 = 0xF0;
pub const COLOR_BLUE: u8 = 0xF1;
pub const COLOR_RED: u8 = 0xF2;
pub const COLOR_PINK: u8 = 0xF3;
pub const COLOR_GREEN: u8 = 0xF4;
pub const COLOR_TURQUOISE: u8 = 0xF5;
pub const COLOR_YELLOW: u8 = 0xF6;
pub const COLOR_WHITE: u8 = 0xF7;
pub const VAL_MUSTFILL: u8 = 0x04;
pub const VAL_MUSTENTER: u8 = 0x02;
pub const VAL_TRIGGER: u8 = 0x01;

pub const AID_NONE: u8 = 0x60;
pub const AID_ENTER: u8 = 0x7D;
pub const AID_CLEAR: u8 = 0x6D;
pub const AID_SF: u8 = 0x88;
pub const AID_SELECT: u8 = 0x7E;
pub const AID_PA1: u8 = 0x6C;
pub const AID_PA2: u8 = 0x6E;
pub const AID_PA3: u8 = 0x6B;
pub const AID_SYSREQ: u8 = 0xF0;
pub const AID_PF3: u8 = 0xF3;
pub const AID_PF7: u8 = 0xF7;
pub const AID_PF8: u8 = 0xF8;
/// PF1〜PF24 の AID（添字 0 が PF1）。
pub const AID_PF: [u8; 24] = [
    0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0xC1, 0xC2, 0xC3, 0xC4,
    0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C,
];

/// AID の名前（`PF13`・`PA1`・`ENTER` など）。
pub fn aid_name(aid: u8) -> String {
    if let Some(i) = AID_PF.iter().position(|&a| a == aid) {
        return format!("PF{}", i + 1);
    }
    match aid {
        AID_ENTER => "ENTER".into(),
        AID_CLEAR => "CLEAR".into(),
        AID_PA1 => "PA1".into(),
        AID_PA2 => "PA2".into(),
        AID_PA3 => "PA3".into(),
        AID_SELECT => "SELECT".into(),
        AID_SYSREQ => "SYSREQ".into(),
        AID_SF => "SF".into(),
        AID_NONE => "NONE".into(),
        0x61 => "QREPLY".into(),
        a => format!("{a:02X}"),
    }
}

// 構造化フィールド（WSF）
pub const SF_READ_PARTITION: u8 = 0x01;
pub const SF_ERASE_RESET: u8 = 0x03;
pub const SF_SET_REPLY_MODE: u8 = 0x09;
pub const SF_CREATE_PARTITION: u8 = 0x0C;
pub const RP_QUERY: u8 = 0x02;
pub const RP_QUERY_LIST: u8 = 0x03;
pub const RM_FIELD: u8 = 0x00;
pub const RM_EXTENDED_FIELD: u8 = 0x01;
pub const RM_CHARACTER: u8 = 0x02;

pub const SF_DATA_CHUNK: u8 = 0xD0;
pub const SF_OUTBOUND_3270DS: u8 = 0x40;

const CODE_TABLE: [u8; 64] = [
    0x40, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
];

/// バッファのアドレス（4096 未満は 12 ビット、それ以上は 14 ビット）。
pub fn addr_bytes(addr: usize) -> [u8; 2] {
    if addr < 4096 {
        [CODE_TABLE[(addr >> 6) & 0x3F], CODE_TABLE[addr & 0x3F]]
    } else {
        [((addr >> 8) & 0x3F) as u8, (addr & 0xFF) as u8]
    }
}

pub fn decode_addr(b1: u8, b2: u8) -> usize {
    if b1 & 0xC0 == 0 {
        (usize::from(b1 & 0x3F) << 8) | usize::from(b2)
    } else {
        (usize::from(b1 & 0x3F) << 6) | usize::from(b2 & 0x3F)
    }
}

/// フィールド属性の送る符号。
pub fn fa_byte(fa: u8) -> u8 {
    CODE_TABLE[usize::from(fa & 0x3F)]
}
