use chrono::{Duration, NaiveDate};
use encoding_rs::Encoding;

#[derive(Clone, Debug)]
pub enum SqliteValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// Maps a SQL Server `xtype` (`system_type_id`) to a SQLite column type affinity for `CREATE TABLE`.
pub fn sql_type_to_sqlite_decl(xtype: u8) -> &'static str {
    match xtype {
        48 | 52 | 56 | 104 | 127 => "INTEGER",
        59 | 60 | 62 | 122 => "REAL",
        106 | 108 => "NUMERIC",
        34 | 165 | 173 | 189 => "BLOB",
        // Strings, dates, times, GUIDs, XML -> TEXT (UTF-8)
        _ => "TEXT",
    }
}

/// Returns whether a SQL Server `xtype` is stored in the Fixed-Length data section of a record
/// (unless the column is `SPARSE`).
/// Note: In `sys.sysrscols`, `leaf_offset > 0` already tells us definitively whether a column
/// is stored in the fixed-length section (`leaf_offset >= 4`) or variable-length section (`leaf_offset < 0`).
pub fn fixed_type_size(xtype: u8, max_length: i16) -> usize {
    match xtype {
        48 => 1,                        // tinyint
        52 => 2,                        // smallint
        56 => 4,                        // int
        127 => 8,                       // bigint
        104 => 1,                       // bit (1 byte shared by up to 8 bit columns)
        59 => 4,                        // real
        62 => 8,                        // float
        60 => 8,                        // money
        122 => 4,                       // smallmoney
        58 => 4,                        // smalldatetime
        61 => 8,                        // datetime
        40 => 3,                        // date
        36 => 16,                       // uniqueidentifier
        189 => 8,                       // timestamp / rowversion
        _ => max_length.max(0) as usize, // char, nchar, binary, decimal, numeric, time, datetime2, datetimeoffset
    }
}

/// Resolves a SQL Server `collation_id` to the corresponding `encoding_rs::Encoding`
/// so single-byte / DBCS Windows strings are decoded accurately into UTF-8.
pub fn encoding_from_collation(collation_id: i32, default_collation_id: i32) -> &'static Encoding {
    let cid = if collation_id != 0 {
        collation_id
    } else {
        default_collation_id
    };

    // SQL Server 2019 UTF-8 collations have bit 0x0800_0000 set
    if (cid & 0x0800_0000) != 0 {
        return encoding_rs::UTF_8;
    }

    let lcid = (cid & 0xFFFF) as u16;
    let primary_lang = lcid & 0x03FF;

    match lcid {
        0x0411 => encoding_rs::SHIFT_JIS, // Japanese (CP932)
        0x0804 | 0x1004 => encoding_rs::GBK, // Simplified Chinese (CP936)
        0x0412 => encoding_rs::EUC_KR,    // Korean (CP949)
        0x0404 | 0x0C04 | 0x1404 => encoding_rs::BIG5, // Traditional Chinese (CP950)
        _ => match primary_lang {
            // Central & Eastern European (Polish, Czech, Hungarian, Slovak, Slovenian, Croatian, Romanian, Albanian)
            0x05 | 0x0E | 0x15 | 0x18 | 0x1A | 0x1B | 0x1C | 0x24 => encoding_rs::WINDOWS_1250,
            // Cyrillic (Russian, Ukrainian, Belarusian, Bulgarian, Macedonian, Serbian, Kazakh, Tatar)
            0x02 | 0x19 | 0x22 | 0x23 | 0x2F | 0x3F | 0x40 | 0x44 => encoding_rs::WINDOWS_1251,
            // Greek
            0x08 => encoding_rs::WINDOWS_1253,
            // Turkish, Azeri, Uzbek
            0x1F | 0x2C | 0x43 => encoding_rs::WINDOWS_1254,
            // Hebrew
            0x0D => encoding_rs::WINDOWS_1255,
            // Arabic, Farsi, Urdu
            0x01 | 0x20 | 0x29 => encoding_rs::WINDOWS_1256,
            // Baltic (Estonian, Latvian, Lithuanian)
            0x25 | 0x26 | 0x27 => encoding_rs::WINDOWS_1257,
            // Vietnamese
            0x2A => encoding_rs::WINDOWS_1258,
            // Thai
            0x1E => encoding_rs::WINDOWS_874,
            // Default: Western European / Latin1_General / Italian / German / English / French / Spanish (CP1252)
            _ => encoding_rs::WINDOWS_1252,
        },
    }
}

/// Decodes a UTF-16LE byte slice (`NCHAR`, `NVARCHAR`, `NTEXT`, `sysname`) into a valid UTF-8 `String`.
pub fn decode_utf16le(bytes: &[u8], trim_trailing_nul: bool) -> String {
    let even_len = bytes.len() & !1;
    let (cow, _) = encoding_rs::UTF_16LE.decode_without_bom_handling(&bytes[..even_len]);
    if trim_trailing_nul {
        cow.trim_end_matches('\0').to_string()
    } else {
        cow.into_owned()
    }
}

/// Decodes a single-byte or DBCS string (`CHAR`, `VARCHAR`, `TEXT`) into a valid UTF-8 `String`.
pub fn decode_ansi(
    bytes: &[u8],
    collation_id: i32,
    default_collation_id: i32,
    trim_trailing_nul: bool,
) -> String {
    let enc = encoding_from_collation(collation_id, default_collation_id);
    let (cow, _, _) = enc.decode(bytes);
    if trim_trailing_nul {
        cow.trim_end_matches('\0').to_string()
    } else {
        cow.into_owned()
    }
}

/// Decodes a raw byte slice of a SQL Server column into a `SqliteValue`.
pub fn decode_sql_value(
    xtype: u8,
    bytes: &[u8],
    scale: u8,
    leaf_bitpos: u8,
    collation_id: i32,
    default_collation_id: i32,
) -> SqliteValue {
    match xtype {
        // tinyint (1B unsigned)
        48 => {
            if bytes.is_empty() {
                SqliteValue::Null
            } else {
                SqliteValue::Integer(bytes[0] as i64)
            }
        }
        // smallint (2B signed LE)
        52 => {
            if bytes.len() < 2 {
                SqliteValue::Null
            } else {
                let v = i16::from_le_bytes([bytes[0], bytes[1]]) as i64;
                SqliteValue::Integer(v)
            }
        }
        // int (4B signed LE)
        56 => {
            if bytes.len() < 4 {
                SqliteValue::Null
            } else {
                let v = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64;
                SqliteValue::Integer(v)
            }
        }
        // bigint (8B signed LE)
        127 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let v = i64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                SqliteValue::Integer(v)
            }
        }
        // bit (boolean bit at `leaf_bitpos` 0..7)
        104 => {
            if bytes.is_empty() {
                SqliteValue::Null
            } else {
                let bit = (bytes[0] >> (leaf_bitpos & 7)) & 1;
                SqliteValue::Integer(bit as i64)
            }
        }
        // real (4B IEEE 754 float)
        59 => {
            if bytes.len() < 4 {
                SqliteValue::Null
            } else {
                let v = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64;
                if v.is_nan() {
                    SqliteValue::Null
                } else {
                    SqliteValue::Real(v)
                }
            }
        }
        // float (8B IEEE 754 double)
        62 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let v = f64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                if v.is_nan() {
                    SqliteValue::Null
                } else {
                    SqliteValue::Real(v)
                }
            }
        }
        // smallmoney (4B signed LE, 1/10,000th currency unit)
        122 => {
            if bytes.len() < 4 {
                SqliteValue::Null
            } else {
                let raw = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64;
                SqliteValue::Real(format_scaled_i128(raw as i128, 4).parse().unwrap_or(0.0))
            }
        }
        // money (8B mixed-word LE: bytes 0..4 = high i32, bytes 4..8 = low u32, 1/10,000th unit)
        60 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let high = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64;
                let low = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as i64;
                let raw = (high << 32) | low;
                SqliteValue::Real(format_scaled_i128(raw as i128, 4).parse().unwrap_or(0.0))
            }
        }
        // decimal (106) & numeric (108)
        106 | 108 => decode_decimal(bytes, scale),
        // date (3B unsigned LE days since 0001-01-01)
        40 => {
            if bytes.len() < 3 {
                SqliteValue::Null
            } else {
                let days = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
                if let Some(d) = date_from_days_0001(days) {
                    SqliteValue::Text(d.format("%Y-%m-%d").to_string())
                } else {
                    SqliteValue::Null
                }
            }
        }
        // time(n) (3..5B unsigned LE ticks since midnight, 1s = 10^scale ticks)
        41 => {
            if bytes.len() < 3 {
                SqliteValue::Null
            } else {
                let ticks = read_u64_le_var(&bytes[..bytes.len().min(5)]);
                SqliteValue::Text(format_time_ticks(ticks, scale))
            }
        }
        // datetime2(n) (T bytes time + 3 bytes date)
        42 => {
            if bytes.len() < 6 {
                SqliteValue::Null
            } else {
                let t_len = bytes.len() - 3;
                let ticks = read_u64_le_var(&bytes[..t_len]);
                let days = u32::from_le_bytes([bytes[t_len], bytes[t_len + 1], bytes[t_len + 2], 0]);
                if let Some(d) = date_from_days_0001(days) {
                    let t_str = format_time_ticks(ticks, scale);
                    SqliteValue::Text(format!("{} {}", d.format("%Y-%m-%d"), t_str))
                } else {
                    SqliteValue::Null
                }
            }
        }
        // datetimeoffset(n) (T bytes UTC time + 3 bytes UTC date + 2 bytes i16 tz offset minutes)
        43 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let t_len = bytes.len() - 5;
                let ticks = read_u64_le_var(&bytes[..t_len]);
                let days = u32::from_le_bytes([bytes[t_len], bytes[t_len + 1], bytes[t_len + 2], 0]);
                let tz_min = i16::from_le_bytes([bytes[t_len + 3], bytes[t_len + 4]]);
                if let Some(d) = date_from_days_0001(days) {
                    let t_str = format_time_ticks(ticks, scale);
                    let sign = if tz_min >= 0 { '+' } else { '-' };
                    let abs_min = tz_min.unsigned_abs();
                    SqliteValue::Text(format!(
                        "{} {} {}{:02}:{:02}",
                        d.format("%Y-%m-%d"),
                        t_str,
                        sign,
                        abs_min / 60,
                        abs_min % 60
                    ))
                } else {
                    SqliteValue::Null
                }
            }
        }
        // smalldatetime (4B: 2B u16 minutes + 2B u16 days since 1900-01-01)
        58 => {
            if bytes.len() < 4 {
                SqliteValue::Null
            } else {
                let mins = u16::from_le_bytes([bytes[0], bytes[1]]) as i64;
                let days = u16::from_le_bytes([bytes[2], bytes[3]]) as i64;
                if let Some(base) = NaiveDate::from_ymd_opt(1900, 1, 1) {
                    if let Some(d) = base.checked_add_signed(Duration::days(days)) {
                        let hh = (mins / 60) % 24;
                        let mm = mins % 60;
                        return SqliteValue::Text(format!(
                            "{} {:02}:{:02}:00",
                            d.format("%Y-%m-%d"),
                            hh,
                            mm
                        ));
                    }
                }
                SqliteValue::Null
            }
        }
        // datetime (8B: 4B i32 ticks of 1/300s + 4B i32 days since 1900-01-01)
        61 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let ticks = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as i64;
                let days = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as i64;
                if let Some(base) = NaiveDate::from_ymd_opt(1900, 1, 1) {
                    if let Some(d) = base.checked_add_signed(Duration::days(days)) {
                        let total_ms = ((ticks * 10) + 1) / 3;
                        let total_secs = total_ms.div_euclid(1000);
                        let ms = total_ms.rem_euclid(1000);
                        let hh = (total_secs / 3600) % 24;
                        let mm = (total_secs % 3600) / 60;
                        let ss = total_secs % 60;
                        return SqliteValue::Text(format!(
                            "{} {:02}:{:02}:{:02}.{:03}",
                            d.format("%Y-%m-%d"),
                            hh,
                            mm,
                            ss,
                            ms
                        ));
                    }
                }
                SqliteValue::Null
            }
        }
        // uniqueidentifier (16B Microsoft mixed-endian GUID)
        36 => {
            if bytes.len() < 16 {
                SqliteValue::Null
            } else {
                let d1 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let d2 = u16::from_le_bytes([bytes[4], bytes[5]]);
                let d3 = u16::from_le_bytes([bytes[6], bytes[7]]);
                SqliteValue::Text(format!(
                    "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                    d1,
                    d2,
                    d3,
                    bytes[8],
                    bytes[9],
                    bytes[10],
                    bytes[11],
                    bytes[12],
                    bytes[13],
                    bytes[14],
                    bytes[15]
                ))
            }
        }
        // char (175), varchar (167), text (35) -> convert Windows codepage to UTF-8
        35 | 167 | 175 => {
            let trim_nul = xtype == 175;
            SqliteValue::Text(decode_ansi(
                bytes,
                collation_id,
                default_collation_id,
                trim_nul,
            ))
        }
        // nchar (239), nvarchar (231), ntext (99) -> convert UTF-16LE to UTF-8
        99 | 231 | 239 => {
            let trim_nul = xtype == 239;
            SqliteValue::Text(decode_utf16le(bytes, trim_nul))
        }
        // xml (241)
        241 => decode_xml(bytes),
        // sql_variant (98)
        98 => decode_sql_variant(bytes, default_collation_id),
        // binary (173), varbinary (165), image (34), timestamp/rowversion (189), or unknown binary
        _ => SqliteValue::Blob(bytes.to_vec()),
    }
}

fn decode_decimal(bytes: &[u8], scale: u8) -> SqliteValue {
    if bytes.is_empty() {
        return SqliteValue::Null;
    }
    let positive = bytes[0] != 0;
    let mut mag_bytes = [0u8; 16];
    let copy_len = (bytes.len() - 1).min(16);
    mag_bytes[..copy_len].copy_from_slice(&bytes[1..1 + copy_len]);
    let mag = u128::from_le_bytes(mag_bytes);

    if scale == 0 {
        if positive {
            if let Ok(v) = i64::try_from(mag) {
                return SqliteValue::Integer(v);
            }
        } else if mag == (i64::MIN as u128) {
            return SqliteValue::Integer(i64::MIN);
        } else if let Ok(v) = i64::try_from(mag) {
            return SqliteValue::Integer(-v);
        }
    }

    let formatted = format_scaled_u128(mag, positive, scale);
    if let Ok(f) = formatted.parse::<f64>() {
        if f.is_finite() {
            return SqliteValue::Real(f);
        }
    }
    SqliteValue::Text(formatted)
}

fn format_scaled_i128(val: i128, scale: u8) -> String {
    let positive = val >= 0;
    let mag = val.unsigned_abs();
    format_scaled_u128(mag, positive, scale)
}

fn format_scaled_u128(mag: u128, positive: bool, scale: u8) -> String {
    let sign_prefix = if positive || mag == 0 { "" } else { "-" };
    let s = mag.to_string();
    let sc = scale as usize;
    if sc == 0 {
        return format!("{}{}", sign_prefix, s);
    }
    if s.len() <= sc {
        let zeros = "0".repeat(sc - s.len());
        format!("{}0.{}{}", sign_prefix, zeros, s)
    } else {
        let split = s.len() - sc;
        format!("{}{}.{}", sign_prefix, &s[..split], &s[split..])
    }
}

fn date_from_days_0001(days: u32) -> Option<NaiveDate> {
    let base = NaiveDate::from_ymd_opt(1, 1, 1)?;
    base.checked_add_signed(Duration::days(days as i64))
}

fn read_u64_le_var(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(buf)
}

fn format_time_ticks(ticks: u64, scale: u8) -> String {
    let factor = 10u64.pow((scale as u32).min(7));
    let total_secs = if factor > 0 { ticks / factor } else { ticks };
    let frac = if factor > 0 { ticks % factor } else { 0 };
    let hh = (total_secs / 3600) % 24;
    let mm = (total_secs % 3600) / 60;
    let ss = total_secs % 60;
    if scale == 0 {
        format!("{:02}:{:02}:{:02}", hh, mm, ss)
    } else {
        format!(
            "{:02}:{:02}:{:02}.{:0width$}",
            hh,
            mm,
            ss,
            frac,
            width = scale as usize
        )
    }
}

fn decode_xml(bytes: &[u8]) -> SqliteValue {
    if bytes.is_empty() {
        return SqliteValue::Text(String::new());
    }
    // Check if UTF-16LE XML text
    if bytes.len() >= 2 && (bytes[0] == b'<' && bytes[1] == 0) {
        return SqliteValue::Text(decode_utf16le(bytes, true));
    }
    // Check if UTF-8 XML text
    if let Ok(s) = std::str::from_utf8(bytes) {
        return SqliteValue::Text(s.to_string());
    }
    // SQL Server Binary XML (starts with 0xDF 0xFF): extract readable text or store as Blob
    SqliteValue::Blob(bytes.to_vec())
}

fn decode_sql_variant(bytes: &[u8], default_collation_id: i32) -> SqliteValue {
    if bytes.len() < 2 {
        return SqliteValue::Null;
    }
    let base_type = bytes[0];
    let version = bytes[1];
    if version != 1 {
        return SqliteValue::Blob(bytes.to_vec());
    }
    match base_type {
        48 | 52 | 56 | 127 | 104 | 59 | 62 | 60 | 122 | 58 | 61 | 40 | 36 => {
            decode_sql_value(base_type, &bytes[2..], 0, 0, 0, default_collation_id)
        }
        41 | 42 | 43 => {
            if bytes.len() < 3 {
                SqliteValue::Null
            } else {
                let scale = bytes[2];
                decode_sql_value(base_type, &bytes[3..], scale, 0, 0, default_collation_id)
            }
        }
        106 | 108 => {
            if bytes.len() < 4 {
                SqliteValue::Null
            } else {
                let scale = bytes[3];
                decode_sql_value(base_type, &bytes[4..], scale, 0, 0, default_collation_id)
            }
        }
        167 | 175 | 231 | 239 => {
            if bytes.len() < 8 {
                SqliteValue::Null
            } else {
                let collation_id = i32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                decode_sql_value(base_type, &bytes[8..], 0, 0, collation_id, default_collation_id)
            }
        }
        _ => SqliteValue::Blob(bytes[2..].to_vec()),
    }
}
