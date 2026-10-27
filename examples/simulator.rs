//! Deterministic local PLC/MQTT/AI services for exercising the entire gateway offline.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    response::{IntoResponse, Sse, sse::Event},
    routing::post,
};
use serde_json::{Value, json};
use std::convert::Infallible;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::main]
async fn main() -> Result<()> {
    for (port, protocol) in [
        (1502, "modbus"),
        (1102, "s7"),
        (15000, "mc"),
        (18830, "mqtt"),
    ] {
        let l = TcpListener::bind(("127.0.0.1", port)).await?;
        println!("{protocol} 模拟服务: 127.0.0.1:{port}");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = l.accept().await {
                tokio::spawn(async move {
                    let _ = match protocol {
                        "modbus" => modbus(&mut socket).await,
                        "s7" => s7(&mut socket).await,
                        "mc" => mc(&mut socket).await,
                        _ => mqtt(&mut socket).await,
                    };
                });
            }
        });
    }
    let l = TcpListener::bind("127.0.0.1:18080").await?;
    println!(
        "AI 模拟服务: http://127.0.0.1:18080/v1\n载入 examples/config.simulated.json 后启动采集；Ctrl+C 结束模拟服务。"
    );
    axum::serve(l, Router::new().route("/v1/chat/completions", post(ai)))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
async fn modbus(s: &mut TcpStream) -> Result<()> {
    loop {
        let mut h = [0; 7];
        s.read_exact(&mut h).await?;
        let n = u16::from_be_bytes([h[4], h[5]]) as usize;
        ensure!((2..=254).contains(&n), "length");
        let mut req = vec![0; n - 1];
        s.read_exact(&mut req).await?;
        ensure!(req.len() == 5, "request");
        let count = u16::from_be_bytes([req[3], req[4]]) as usize;
        ensure!(count <= 125, "count");
        let address = u16::from_be_bytes([req[1], req[2]]) as usize;
        let mut data = vec![req[0]];
        if req[0] <= 2 {
            data.push(count.div_ceil(8) as u8);
            data.extend(vec![0x55; count.div_ceil(8)]);
        } else {
            data.push((count * 2) as u8);
            for i in 0..count {
                let word = match (address + i) % 4 {
                    0 => 0x41ccu16,
                    1 => 0,
                    2 => 12,
                    _ => 42,
                };
                data.extend(word.to_be_bytes());
            }
        }
        let mut response = h[..4].to_vec();
        response.extend(((data.len() + 1) as u16).to_be_bytes());
        response.push(h[6]);
        response.extend(data);
        s.write_all(&response).await?;
    }
}
async fn tpkt(s: &mut TcpStream) -> Result<Vec<u8>> {
    let mut h = [0; 4];
    s.read_exact(&mut h).await?;
    let n = u16::from_be_bytes([h[2], h[3]]) as usize;
    ensure!((7..=8192).contains(&n), "length");
    let mut data = vec![0; n - 4];
    s.read_exact(&mut data).await?;
    Ok(data)
}
async fn send_tpkt(s: &mut TcpStream, data: &[u8]) -> Result<()> {
    let mut packet = vec![3, 0];
    packet.extend(((data.len() + 4) as u16).to_be_bytes());
    packet.extend(data);
    s.write_all(&packet).await?;
    Ok(())
}
async fn s7(s: &mut TcpStream) -> Result<()> {
    let mut connection = tpkt(s).await?;
    ensure!(connection.len() >= 7, "COTP");
    connection[1] = 0xd0;
    send_tpkt(s, &connection).await?;
    loop {
        let request = tpkt(s).await?;
        ensure!(request.len() >= 14, "S7");
        let (reference, function) = ([request[7], request[8]], request[13]);
        let (params, data) = if function == 0xf0 {
            (vec![0xf0, 0, 0, 1, 0, 1, 1, 0xe0], vec![])
        } else {
            ensure!(request.len() >= 27, "read item");
            let n = u16::from_be_bytes([request[19], request[20]]) as usize;
            ensure!(n <= 200, "read size");
            let address =
                u32::from_be_bytes([0, request[24], request[25], request[26]]) as usize / 8;
            let memory = [0x41, 0xcc, 0, 0, 0, 12, 0, 42, 1, 0, 0, 0, 0, 0, 0, 0];
            let mut data = vec![0xff, 4];
            data.extend(((n * 8) as u16).to_be_bytes());
            for i in 0..n {
                data.push(*memory.get(address + i).unwrap_or(&0));
            }
            (vec![4, 1], data)
        };
        let mut ack = vec![2, 0xf0, 0x80, 0x32, 3, 0, 0];
        ack.extend(reference);
        ack.extend((params.len() as u16).to_be_bytes());
        ack.extend((data.len() as u16).to_be_bytes());
        ack.extend([0, 0]);
        ack.extend(params);
        ack.extend(data);
        send_tpkt(s, &ack).await?;
    }
}
async fn mc(s: &mut TcpStream) -> Result<()> {
    loop {
        let mut h = [0; 9];
        s.read_exact(&mut h).await?;
        let n = u16::from_le_bytes([h[7], h[8]]) as usize;
        ensure!(n == 12, "MC length");
        let mut req = vec![0; n];
        s.read_exact(&mut req).await?;
        let count = u16::from_le_bytes([req[10], req[11]]) as usize;
        ensure!(count <= 100, "count");
        let mut data = vec![0, 0];
        if req[4] == 1 {
            data.extend(vec![0x10; count.div_ceil(2)]);
        } else {
            for i in 0..count {
                data.extend((if i % 2 == 0 { 0u16 } else { 0x41ccu16 }).to_le_bytes());
            }
        }
        let mut response = vec![0xd0, 0, 0, 0xff, 0xff, 3, 0];
        response.extend((data.len() as u16).to_le_bytes());
        response.extend(data);
        s.write_all(&response).await?;
    }
}
async fn mqtt(s: &mut TcpStream) -> Result<()> {
    loop {
        let first = s.read_u8().await?;
        let mut size = 0;
        let mut shift = 0;
        loop {
            let b = s.read_u8().await?;
            size |= ((b & 127) as usize) << shift;
            if b & 128 == 0 {
                break;
            }
            shift += 7;
            ensure!(shift <= 21, "MQTT length");
        }
        ensure!(size < 1024 * 1024, "MQTT packet limit");
        let mut p = vec![0; size];
        s.read_exact(&mut p).await?;
        match first >> 4 {
            1 => s.write_all(&[0x20, 2, 0, 0]).await?,
            3 => {
                ensure!(p.len() >= 2, "topic");
                let n = u16::from_be_bytes([p[0], p[1]]) as usize;
                ensure!(p.len() >= n + 2, "topic");
                let qos = (first >> 1) & 3;
                if qos > 0 {
                    ensure!(p.len() >= n + 4, "packet id");
                    s.write_all(&[if qos == 2 { 0x50 } else { 0x40 }, 2, p[n + 2], p[n + 3]])
                        .await?;
                }
            }
            6 => {
                ensure!(p.len() == 2, "pubrel");
                s.write_all(&[0x70, 2, p[0], p[1]]).await?;
            }
            12 => s.write_all(&[0xd0, 0]).await?,
            14 => return Ok(()),
            _ => {}
        }
    }
}
async fn ai(Json(request): Json<Value>) -> axum::response::Response {
    let content=json!({"name":"AI生成的模拟设备","protocol":"modbus_tcp","ip":"127.0.0.1","port":1502,"points":[{"name":"温度","address":"40001","type":"float","unit":"℃"}]}).to_string();
    if request["stream"] == true {
        let events = content
            .chars()
            .map(|c| {
                Event::default()
                    .data(json!({"choices":[{"delta":{"content":c.to_string()}}]}).to_string())
            })
            .chain(std::iter::once(Event::default().data("[DONE]")))
            .map(Ok::<_, Infallible>)
            .collect::<Vec<_>>();
        Sse::new(futures_util::stream::iter(events)).into_response()
    } else {
        Json(json!({"choices":[{"message":{"content":content}}]})).into_response()
    }
}
