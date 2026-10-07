//! Telnet・TN3270E・3270 の符号（模擬ホストの側。試験する側の実装は使わない）。

pub const IAC: u8 = 255;
pub const DONT: u8 = 254;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const SB: u8 = 250;
pub const SE: u8 = 240;
pub const EOR: u8 = 239;

pub const OPT_BINARY: u8 = 0;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_EOR: u8 = 25;
pub const OPT_TN3270E: u8 = 40;
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

pub const DT_3270_DATA: u8 = 0;
pub const DT_SCS_DATA: u8 = 1;
pub const DT_RESPONSE: u8 = 2;
pub const DT_PRINT_EOJ: u8 = 8;
pub const RSP_NO_RESPONSE: u8 = 0;
pub const RSP_ALWAYS_RESPONSE: u8 = 2;
pub const RSP_POSITIVE: u8 = 0;

// コマンド（SNA の符号）
pub const CMD_W: u8 = 0xF1;
pub const CMD_EW: u8 = 0xF5;
pub const CMD_WSF: u8 = 0xF3;

pub const WCC_RESET: u8 = 0x40;
pub const WCC_START_PRINTER: u8 = 0x08;
pub const WCC_ALARM: u8 = 0x04;
pub const WCC_RESTORE: u8 = 0x02;
pub const WCC_RESET_MDT: u8 = 0x01;

pub const ORDER_SBA: u8 = 0x11;
pub const ORDER_IC: u8 = 0x13;
pub const ORDER_SF: u8 = 0x1D;
pub const ORDER_SFE: u8 = 0x29;

pub const FA_PROTECT: u8 = 0x20;
pub const FA_NUMERIC: u8 = 0x10;
pub const FA_INTENSIFIED: u8 = 0x08;
pub const FA_NONDISPLAY: u8 = 0x0C;

pub const XA_3270: u8 = 0xC0;
pub const XA_HIGHLIGHTING: u8 = 0x41;
pub const XA_FOREGROUND: u8 = 0x42;
pub const XA_CHARSET: u8 = 0x43;
pub const HL_UNDERSCORE: u8 = 0xF4;
pub const CS_DBCS: u8 = 0xF8;
pub const COLOR_GREEN: u8 = 0xF4;
pub const COLOR_TURQUOISE: u8 = 0xF5;
pub const COLOR_WHITE: u8 = 0xF7;
pub const COLOR_YELLOW: u8 = 0xF6;

pub const AID_ENTER: u8 = 0x7D;
pub const AID_CLEAR: u8 = 0x6D;
pub const AID_SF: u8 = 0x88;
pub const AID_PF3: u8 = 0xF3;

pub const SF_DATA_CHUNK: u8 = 0xD0;
pub const SF_OUTBOUND_3270DS: u8 = 0x40;

const CODE_TABLE: [u8; 64] = [
    0x40, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
    0x50, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F,
    0x60, 0x61, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
    0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x7F,
];

/// 12 ビットのアドレス。
pub fn addr_bytes(addr: usize) -> [u8; 2] {
    [CODE_TABLE[(addr >> 6) & 0x3F], CODE_TABLE[addr & 0x3F]]
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
