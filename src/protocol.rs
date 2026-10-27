//! Bounded asynchronous PLC transports. No blocking socket operation runs on the executor.
use crate::{
    codec,
    model::{Device, Point, Port},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::{Duration, timeout},
};
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, StopBits};

pub trait Wire: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Wire for T {}
pub struct Client {
    stream: Box<dyn Wire>,
    protocol: String,
    slave: u8,
    sequence: u16,
    rtu_gap: Duration,
    next_rtu: tokio::time::Instant,
}
const IO_TIMEOUT: Duration = Duration::from_secs(3);
impl Client {
    pub async fn connect(device: &Device) -> Result<Self> {
        timeout(IO_TIMEOUT, Self::connect_inner(device))
            .await
            .context("PLC连接超时")?
    }
    async fn connect_inner(d: &Device) -> Result<Self> {
        ensure!(
            matches!(
                d.protocol.as_str(),
                "modbus_tcp" | "modbus_rtu" | "s7" | "mitsubishi"
            ),
            "不支持的协议: {}",
            d.protocol
        );
        let stream: Box<dyn Wire> = if d.protocol == "modbus_rtu" {
            let port = d
                .com_port
                .as_deref()
                .or(match &d.port {
                    Some(Port::Text(s)) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("COM1");
            let baud: u32 = d.baudrate.unwrap_or(9600).try_into()?;
            ensure!(baud > 0, "invalid baudrate");
            let bits = match d.databits.unwrap_or(8) {
                5 => DataBits::Five,
                6 => DataBits::Six,
                7 => DataBits::Seven,
                8 => DataBits::Eight,
                _ => bail!("invalid databits"),
            };
            let parity = match d.parity.as_deref().unwrap_or("N") {
                "N" => Parity::None,
                "E" => Parity::Even,
                "O" => Parity::Odd,
                _ => bail!("invalid parity"),
            };
            let stop = match d.stopbits.unwrap_or(1) {
                1 => StopBits::One,
                2 => StopBits::Two,
                _ => bail!("invalid stopbits"),
            };
            Box::new(
                tokio_serial::new(port, baud)
                    .data_bits(bits)
                    .parity(parity)
                    .stop_bits(stop)
                    .timeout(IO_TIMEOUT)
                    .open_native_async()?,
            )
        } else {
            let default = match d.protocol.as_str() {
                "s7" => 102,
                "mitsubishi" => 5000,
                _ => 502,
            };
            let socket =
                TcpStream::connect((d.ip.as_deref().unwrap_or("127.0.0.1"), d.tcp_port(default)?))
                    .await?;
            socket.set_nodelay(true)?;
            Box::new(socket)
        };
        let mut c = Self {
            stream,
            protocol: d.protocol.clone(),
            slave: d.slave_id.unwrap_or(1).try_into()?,
            sequence: 0,
            rtu_gap: if d.protocol == "modbus_rtu" {
                rtu_silent_interval(d.baudrate.unwrap_or(9600).try_into()?)
            } else {
                Duration::ZERO
            },
            next_rtu: tokio::time::Instant::now(),
        };
        if d.protocol == "s7" {
            c.s7_handshake(d.rack.unwrap_or(0), d.slot.unwrap_or(1))
                .await?;
        }
        Ok(c)
    }
    pub async fn read(&mut self, point: &Point) -> Result<Value> {
        if self.protocol == "modbus_rtu" {
            tokio::time::sleep_until(self.next_rtu).await;
        }
        let result = timeout(IO_TIMEOUT, self.read_raw(point)).await;
        self.next_rtu = tokio::time::Instant::now() + self.rtu_gap;
        let value = result.context("PLC读取超时")??;
        ensure!(!value.is_null(), "non-finite or absent PLC value");
        let value = codec::scale(value, point.rate, point.offset);
        ensure!(!value.is_null(), "non-finite scaled value");
        Ok(value)
    }
    async fn read_raw(&mut self, p: &Point) -> Result<Value> {
        match self.protocol.as_str() {
            "modbus_tcp" | "modbus_rtu" => {
                let (function, address) = codec::modbus_address(&p.address)?;
                let address: u16 = address.try_into()?;
                let count = codec::word_count(&p.kind);
                let response = if self.protocol == "modbus_rtu" {
                    rtu_exchange(&mut self.stream, self.slave, function, address, count).await?
                } else {
                    self.modbus_tcp(function, address, count).await?
                };
                ensure!(
                    response.len() >= 2 && response[0] == function,
                    "Modbus exception or wrong function"
                );
                let expected = if function <= 2 {
                    (count as usize).div_ceil(8)
                } else {
                    count as usize * 2
                };
                ensure!(
                    response[1] as usize == expected && response.len() == expected + 2,
                    "Modbus byte count mismatch"
                );
                if function <= 2 {
                    let bits: Vec<bool> = response[2..]
                        .iter()
                        .flat_map(|v| (0..8).map(move |i| v & (1 << i) != 0))
                        .collect();
                    codec::decode_bits(&bits, &p.kind)
                } else {
                    codec::decode(&response[2..], &p.kind, "modbus", 0)
                }
            }
            "s7" => self.s7_read(p).await,
            "mitsubishi" => self.mc_read(p).await,
            _ => bail!("unsupported protocol"),
        }
    }
    async fn modbus_tcp(&mut self, function: u8, address: u16, count: u16) -> Result<Vec<u8>> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut req = self.sequence.to_be_bytes().to_vec();
        req.extend([0, 0, 0, 6, self.slave, function]);
        req.extend(address.to_be_bytes());
        req.extend(count.to_be_bytes());
        self.stream.write_all(&req).await?;
        let mut h = [0; 7];
        self.stream.read_exact(&mut h).await?;
        ensure!(
            u16::from_be_bytes([h[0], h[1]]) == self.sequence
                && h[2..4] == [0, 0]
                && h[6] == self.slave,
            "Modbus header mismatch"
        );
        let len = u16::from_be_bytes([h[4], h[5]]) as usize;
        ensure!((3..=254).contains(&len), "invalid Modbus length");
        let mut response = vec![0; len - 1];
        self.stream.read_exact(&mut response).await?;
        Ok(response)
    }
    async fn tpkt_send(&mut self, body: &[u8]) -> Result<()> {
        let mut b = vec![3, 0];
        b.extend(u16::try_from(body.len() + 4)?.to_be_bytes());
        b.extend(body);
        self.stream.write_all(&b).await?;
        Ok(())
    }
    async fn tpkt_receive(&mut self) -> Result<Vec<u8>> {
        let mut h = [0; 4];
        self.stream.read_exact(&mut h).await?;
        ensure!(h[..2] == [3, 0], "invalid TPKT header");
        let n = u16::from_be_bytes([h[2], h[3]]) as usize;
        ensure!((7..=8192).contains(&n), "invalid TPKT length");
        let mut b = vec![0; n - 4];
        self.stream.read_exact(&mut b).await?;
        Ok(b)
    }
    async fn s7_handshake(&mut self, rack: i64, slot: i64) -> Result<()> {
        ensure!(
            (0..=7).contains(&rack) && (0..=31).contains(&slot),
            "invalid S7 rack/slot"
        );
        self.tpkt_send(&[
            0x11,
            0xe0,
            0,
            0,
            0,
            1,
            0,
            0xc1,
            2,
            1,
            0,
            0xc2,
            2,
            1,
            (rack * 32 + slot) as u8,
            0xc0,
            1,
            0x0a,
        ])
        .await?;
        let r = self.tpkt_receive().await?;
        ensure!(r.len() >= 7 && r[1] == 0xd0, "COTP connection rejected");
        let response = self.s7_job(&[0xf0, 0, 0, 1, 0, 1, 1, 0xe0]).await?;
        ensure!(
            response.len() >= 23 && response[15] == 0xf0,
            "S7 setup rejected"
        );
        Ok(())
    }
    async fn s7_job(&mut self, parameters: &[u8]) -> Result<Vec<u8>> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut body = vec![2, 0xf0, 0x80, 0x32, 1, 0, 0];
        body.extend(self.sequence.to_be_bytes());
        body.extend(u16::try_from(parameters.len())?.to_be_bytes());
        body.extend([0, 0]);
        body.extend(parameters);
        self.tpkt_send(&body).await?;
        let mut response = Vec::new();
        loop {
            let part = self.tpkt_receive().await?;
            ensure!(
                part.len() >= 3 && part[0] == 2 && part[1] == 0xf0,
                "invalid COTP data"
            );
            if response.is_empty() {
                response.extend_from_slice(&part)
            } else {
                response.extend_from_slice(&part[3..])
            }
            ensure!(response.len() <= 8192, "S7 reassembly limit");
            if part[2] & 0x80 != 0 {
                break;
            }
        }
        ensure!(
            response.len() >= 15 && response[3] == 0x32 && response[4] == 3,
            "invalid S7 response"
        );
        ensure!(
            response[7..9] == self.sequence.to_be_bytes(),
            "S7 PDU reference mismatch"
        );
        ensure!(response[13..15] == [0, 0], "S7 error class/code");
        let plen = u16::from_be_bytes([response[9], response[10]]) as usize;
        let dlen = u16::from_be_bytes([response[11], response[12]]) as usize;
        ensure!(
            response.len() == 15 + plen + dlen,
            "S7 response length mismatch"
        );
        Ok(response)
    }
    async fn s7_read(&mut self, p: &Point) -> Result<Value> {
        let a = codec::s7_address(&p.address)?;
        let n = codec::byte_count(&p.kind);
        let area = match a.area.as_str() {
            "DB" => 0x84,
            "M" => 0x83,
            "I" | "E" => 0x81,
            "Q" | "A" => 0x82,
            _ => bail!("invalid area"),
        };
        let bit_address = a.start.checked_mul(8).context("S7 address overflow")?;
        ensure!(bit_address <= 0xffffff, "S7 address exceeds 24 bits");
        let mut params = vec![4, 1, 0x12, 0x0a, 0x10, 2];
        params.extend((n as u16).to_be_bytes());
        params.extend(u16::try_from(a.db)?.to_be_bytes());
        params.push(area);
        params.extend(&bit_address.to_be_bytes()[1..]);
        let response = self.s7_job(&params).await?;
        let plen = u16::from_be_bytes([response[9], response[10]]) as usize;
        ensure!(
            plen == 2 && response[15..17] == [4, 1],
            "invalid S7 read parameters"
        );
        let d = &response[15 + plen..];
        ensure!(d.len() >= 4 && d[0] == 0xff, "S7 item read failed");
        let length = u16::from_be_bytes([d[2], d[3]]) as usize;
        let bytes = match d[1] {
            3..=5 => length.div_ceil(8),
            7 | 9 => length,
            _ => bail!("unsupported S7 transport size"),
        };
        ensure!(
            bytes == n && d.len() == 4 + bytes,
            "S7 data length mismatch"
        );
        codec::decode(&d[4..], &p.kind, "s7", a.bit)
    }
    async fn mc_read(&mut self, p: &Point) -> Result<Value> {
        let (code, address) = mc_address(&p.address)?;
        let bit = p.kind == "bool";
        let count = codec::word_count(&p.kind);
        let mut req = vec![
            0x50,
            0,
            0,
            0xff,
            0xff,
            3,
            0,
            12,
            0,
            4,
            0,
            1,
            4,
            if bit { 1 } else { 0 },
            0,
        ];
        req.extend(&address.to_le_bytes()[..3]);
        req.push(code);
        req.extend(count.to_le_bytes());
        self.stream.write_all(&req).await?;
        let mut h = [0; 9];
        self.stream.read_exact(&mut h).await?;
        ensure!(
            h[..7] == [0xd0, 0, 0, 0xff, 0xff, 3, 0],
            "MC header mismatch"
        );
        let len = u16::from_le_bytes([h[7], h[8]]) as usize;
        ensure!((2..=4096).contains(&len), "invalid MC length");
        let mut data = vec![0; len];
        self.stream.read_exact(&mut data).await?;
        ensure!(
            data[..2] == [0, 0],
            "MC end code {:02x}{:02x}",
            data[1],
            data[0]
        );
        let expected = if bit { 1 } else { count as usize * 2 };
        ensure!(data.len() == 2 + expected, "MC data length mismatch");
        let words = if bit {
            ensure!(data[2] & 0xef == 0, "invalid MC bit encoding");
            vec![(data[2] >> 4) as i16]
        } else {
            data[2..]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect()
        };
        codec::decode_mc(&words, &p.kind)
    }
}
/// Modbus serial line V1.02: 3.5 eleven-bit characters, or 1.75 ms above 19200 baud.
pub fn rtu_silent_interval(baud: u32) -> Duration {
    if baud > 19200 {
        Duration::from_micros(1750)
    } else {
        Duration::from_secs_f64(38.5 / f64::from(baud.max(1)))
    }
}
pub fn mc_address(address: &str) -> Result<(u8, u32)> {
    // Q-series device codes used by pymcprotocol.Type3E's default PLC type.
    let kinds = [
        ("LSTS", 0x59, false),
        ("LSTC", 0x58, false),
        ("LSTN", 0x5a, false),
        ("LTS", 0x51, false),
        ("LTC", 0x50, false),
        ("LTN", 0x52, false),
        ("LCS", 0x55, false),
        ("LCC", 0x54, false),
        ("LCN", 0x56, false),
        ("STS", 0xc7, false),
        ("STC", 0xc6, false),
        ("STN", 0xc8, false),
        ("SM", 0x91, false),
        ("SD", 0xa9, false),
        ("TS", 0xc1, false),
        ("TC", 0xc0, false),
        ("TN", 0xc2, false),
        ("CS", 0xc4, false),
        ("CC", 0xc3, false),
        ("CN", 0xc5, false),
        ("SB", 0xa1, true),
        ("SW", 0xb5, true),
        ("DX", 0xa2, true),
        ("DY", 0xa3, true),
        ("ZR", 0xb0, true),
        ("X", 0x9c, true),
        ("Y", 0x9d, true),
        ("M", 0x90, false),
        ("L", 0x92, false),
        ("F", 0x93, false),
        ("V", 0x94, false),
        ("B", 0xa0, true),
        ("D", 0xa8, false),
        ("W", 0xb4, true),
        ("R", 0xaf, false),
        ("Z", 0xcc, false),
        ("S", 0x98, false),
    ];
    for (prefix, code, hex) in kinds {
        if let Some(rest) = address.strip_prefix(prefix) {
            let n = u32::from_str_radix(rest, if hex { 16 } else { 10 })?;
            ensure!(n <= 0xffffff, "MC address out of range");
            return Ok((code, n));
        }
    }
    bail!("无法识别的三菱地址: {address}")
}
pub async fn rtu_exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    slave: u8,
    function: u8,
    address: u16,
    count: u16,
) -> Result<Vec<u8>> {
    let mut request = vec![slave, function];
    request.extend(address.to_be_bytes());
    request.extend(count.to_be_bytes());
    request.extend(codec::crc16(&request).to_le_bytes());
    stream.write_all(&request).await?;
    stream.flush().await?;
    let mut header = [0; 3];
    stream.read_exact(&mut header).await?;
    ensure!(header[0] == slave, "RTU slave mismatch");
    let n = if header[1] & 0x80 != 0 {
        0
    } else {
        header[2] as usize
    };
    ensure!(n <= 250, "RTU response too long");
    let mut tail = vec![0; n + 2];
    stream.read_exact(&mut tail).await?;
    let mut frame = header.to_vec();
    frame.extend(&tail[..n]);
    ensure!(
        codec::crc16(&frame) == u16::from_le_bytes([tail[n], tail[n + 1]]),
        "RTU CRC mismatch"
    );
    ensure!(header[1] == function, "RTU exception or function mismatch");
    Ok(frame[1..].to_vec())
}
