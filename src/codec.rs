//! Legacy-compatible value conversion; wire validation belongs to the transport.
use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub fn modbus_address(address: &str) -> Result<(u8, i64)> {
    let a: i64 = address.trim().parse()?;
    Ok(match a {
        40001..=49999 => (3, a - 40001),
        30001..=39999 => (4, a - 30001),
        1..=9999 => (1, a - 1),
        10001..=19999 => (2, a - 10001),
        _ => (3, a),
    })
}
#[derive(Debug, Clone, PartialEq)]
pub struct S7Address {
    pub area: String,
    pub db: u32,
    pub start: u32,
    pub bit: u32,
    pub hint: &'static str,
}
pub fn s7_address(address: &str) -> Result<S7Address> {
    static DB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^DB(\d+)\.(DB[XDWB])(\d+)(?:\.(\d+))?$").unwrap());
    static IO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^([MIQEA])(\d+)(?:\.(\d+))?$").unwrap());
    if let Some(c) = DB.captures(address) {
        return Ok(S7Address {
            area: "DB".into(),
            db: c[1].parse()?,
            start: c[3].parse()?,
            bit: c.get(4).map_or(Ok(0), |m| m.as_str().parse())?,
            hint: match c[2].to_uppercase().as_str() {
                "DBX" => "bool",
                "DBB" => "uint16",
                "DBW" => "int16",
                _ => "float",
            },
        });
    }
    if let Some(c) = IO.captures(address) {
        return Ok(S7Address {
            area: c[1].to_uppercase(),
            db: 0,
            start: c[2].parse()?,
            bit: c.get(3).map_or(Ok(0), |m| m.as_str().parse())?,
            hint: "bool",
        });
    }
    bail!("无法识别的S7地址格式: {address}，支持格式: DB1.DBD0, DB1.DBX0.0, M0.0, I0.0, Q0.0")
}
fn bytes<const N: usize>(data: &[u8]) -> Result<[u8; N]> {
    data.get(..N)
        .ok_or_else(|| anyhow!("short data"))?
        .try_into()
        .map_err(Into::into)
}
fn rounded(v: f64) -> Value {
    if !v.is_finite() {
        return Value::Null;
    }
    json!(format!("{v:.6}").parse::<f64>().unwrap_or(v))
}
pub fn decode(data: &[u8], kind: &str, protocol: &str, bit: u32) -> Result<Value> {
    Ok(match kind {
        "bool" => {
            if protocol == "modbus" {
                json!(u16::from_be_bytes(bytes(data)?) != 0)
            } else {
                json!(
                    data.first().ok_or_else(|| anyhow!("short data"))?
                        & (1u8.checked_shl(bit).unwrap_or(0))
                        != 0
                )
            }
        }
        "int16" => json!(i16::from_be_bytes(bytes(data)?)),
        "uint16" => json!(u16::from_be_bytes(bytes(data)?)),
        "int32" => json!(i32::from_be_bytes(bytes(data)?)),
        "uint32" => json!(u32::from_be_bytes(bytes(data)?)),
        "float" => rounded(f32::from_be_bytes(bytes(data)?) as f64),
        "double" => rounded(f64::from_be_bytes(bytes(data)?)),
        "string" if protocol == "s7" => Value::String(
            data.iter()
                .filter(|b| b.is_ascii())
                .map(|b| *b as char)
                .collect::<String>()
                .trim_end_matches('\0')
                .into(),
        ),
        _ if protocol == "modbus" => json!(u16::from_be_bytes(bytes(data)?)),
        _ => json!(f32::from_be_bytes(bytes(data)?) as f64),
    })
}
pub fn decode_bits(bits: &[bool], kind: &str) -> Result<Value> {
    let first = bits.first().ok_or_else(|| anyhow!("empty bits"))?;
    if kind == "bool" {
        return Ok(json!(first));
    }
    let value = bits
        .iter()
        .take(16)
        .enumerate()
        .fold(0u16, |n, (i, b)| n | ((*b as u16) << i));
    Ok(if kind == "int16" {
        json!(value as i16)
    } else {
        json!(value)
    })
}
pub fn decode_mc(words: &[i16], kind: &str) -> Result<Value> {
    let first = *words.first().ok_or_else(|| anyhow!("empty words"))?;
    let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    Ok(match kind {
        "bool" => json!(first != 0),
        "int32" => json!(i32::from_le_bytes(bytes(&data)?)),
        "uint32" => json!(u32::from_le_bytes(bytes(&data)?)),
        "float" => rounded(f32::from_le_bytes(bytes(&data)?) as f64),
        "double" => rounded(f64::from_le_bytes(bytes(&data)?)),
        _ => json!(first),
    })
}
pub fn scale(raw: Value, rate: f64, offset: f64) -> Value {
    let n = raw
        .as_f64()
        .or_else(|| raw.as_bool().map(|b| if b { 1.0 } else { 0.0 }));
    match n {
        Some(n) => json!(n * rate + offset),
        None => raw,
    }
}
pub fn byte_count(kind: &str) -> usize {
    match kind {
        "bool" => 1,
        "int16" | "uint16" => 2,
        "double" => 8,
        _ => 4,
    }
}
pub fn word_count(kind: &str) -> u16 {
    match kind {
        "int32" | "uint32" | "float" => 2,
        "double" => 4,
        _ => 1,
    }
}
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for b in data {
        crc ^= *b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}
