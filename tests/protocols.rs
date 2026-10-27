use rustlink::{
    codec::crc16,
    model::{Device, Point},
    protocol::{Client, rtu_exchange},
};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn device(protocol: &str, port: u16) -> Device {
    serde_json::from_value(
        json!({"id":"plc","name":"模拟PLC","protocol":protocol,"ip":"127.0.0.1","port":port}),
    )
    .unwrap()
}
fn point(address: &str, kind: &str) -> Point {
    serde_json::from_value(json!({"name":"温度","address":address,"type":kind})).unwrap()
}

#[tokio::test]
async fn modbus_tcp_real_socket_fragmented_float_and_boolean() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut req = [0; 12];
        s.read_exact(&mut req).await.unwrap();
        assert_eq!(&req[2..], &[0, 0, 0, 6, 1, 3, 0, 0, 0, 2]);
        let response = [req[0], req[1], 0, 0, 0, 7, 1, 3, 4, 0x41, 0xcc, 0, 0];
        for byte in response {
            s.write_all(&[byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        s.read_exact(&mut req).await.unwrap();
        assert_eq!(&req[7..], &[1, 0, 0, 0, 1]);
        s.write_all(&[req[0], req[1], 0, 0, 0, 4, 1, 1, 1, 1])
            .await
            .unwrap();
    });
    let mut c = Client::connect(&device("modbus_tcp", port)).await.unwrap();
    assert_eq!(c.read(&point("40001", "float")).await.unwrap(), json!(25.5));
    assert_eq!(c.read(&point("00001", "bool")).await.unwrap(), json!(1.0));
    server.await.unwrap();
}

#[tokio::test]
async fn modbus_rejects_wrong_transaction_slave_and_exception() {
    for fault in 0..4 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut req = [0; 12];
            s.read_exact(&mut req).await.unwrap();
            let mut res = vec![req[0], req[1], 0, 0, 0, 5, 1, 3, 2, 0, 42];
            match fault {
                0 => res[1] = res[1].wrapping_add(1),
                1 => res[6] = 2,
                2 => res = vec![req[0], req[1], 0, 0, 0, 3, 1, 0x83, 2],
                _ => res[8] = 4,
            }
            s.write_all(&res).await.unwrap();
        });
        let mut c = Client::connect(&device("modbus_tcp", port)).await.unwrap();
        assert!(
            c.read(&point("40001", "uint16")).await.is_err(),
            "fault {fault}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn rtu_frames_crc_and_exception_are_checked() {
    assert_eq!(crc16(&[1, 3, 0, 0, 0, 1]), 0x0a84);
    for corrupt in [false, true] {
        let (mut a, mut b) = tokio::io::duplex(64);
        let server = tokio::spawn(async move {
            let mut req = [0; 8];
            b.read_exact(&mut req).await.unwrap();
            assert_eq!(req, [1, 3, 0, 0, 0, 1, 0x84, 0x0a]);
            let mut res = vec![1, 3, 2, 0, 42];
            let crc = crc16(&res);
            res.extend(crc.to_le_bytes());
            if corrupt {
                res[6] ^= 1;
            }
            b.write_all(&res).await.unwrap();
        });
        let got = rtu_exchange(&mut a, 1, 3, 0, 1).await;
        if corrupt {
            assert!(got.is_err())
        } else {
            assert_eq!(got.unwrap(), vec![3, 2, 0, 42]);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn mc_binary_3e_word_and_bit_frames() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut req = [0; 21];
        s.read_exact(&mut req).await.unwrap();
        assert_eq!(
            req,
            [
                0x50, 0, 0, 0xff, 0xff, 3, 0, 12, 0, 4, 0, 1, 4, 0, 0, 100, 0, 0, 0xa8, 2, 0
            ]
        );
        s.write_all(&[0xd0, 0, 0, 0xff, 0xff, 3, 0, 6, 0, 0, 0, 0, 0, 0xcc, 0x41])
            .await
            .unwrap();
        s.read_exact(&mut req).await.unwrap();
        assert_eq!(&req[11..], &[1, 4, 1, 0, 16, 0, 0, 0x9c, 1, 0]);
        s.write_all(&[0xd0, 0, 0, 0xff, 0xff, 3, 0, 3, 0, 0, 0, 0x10])
            .await
            .unwrap();
    });
    let mut c = Client::connect(&device("mitsubishi", port)).await.unwrap();
    assert_eq!(c.read(&point("D100", "float")).await.unwrap(), json!(25.5));
    assert_eq!(c.read(&point("X10", "bool")).await.unwrap(), json!(1.0));
    server.await.unwrap();
}

async fn tpkt_read(s: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut h = [0; 4];
    s.read_exact(&mut h).await.unwrap();
    let n = u16::from_be_bytes([h[2], h[3]]) as usize;
    let mut b = vec![0; n - 4];
    s.read_exact(&mut b).await.unwrap();
    b
}
async fn tpkt_write(s: &mut tokio::net::TcpStream, b: &[u8]) {
    let mut v = vec![3, 0];
    v.extend(((b.len() + 4) as u16).to_be_bytes());
    v.extend(b);
    s.write_all(&v).await.unwrap();
}

#[tokio::test]
async fn s7_cotp_setup_and_read_db_real_socket() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let conn = tpkt_read(&mut s).await;
        assert_eq!(conn[1], 0xe0);
        assert!(conn.windows(4).any(|x| x == [0xc2, 2, 1, 1]));
        let mut ack = conn;
        ack[1] = 0xd0;
        tpkt_write(&mut s, &ack).await;
        let setup = tpkt_read(&mut s).await;
        assert_eq!(&setup[13..], &[0xf0, 0, 0, 1, 0, 1, 1, 0xe0]);
        let mut ack = vec![
            2, 0xf0, 0x80, 0x32, 3, 0, 0, setup[7], setup[8], 0, 8, 0, 0, 0, 0,
        ];
        ack.extend([0xf0, 0, 0, 1, 0, 1, 1, 0xe0]);
        tpkt_write(&mut s, &ack).await;
        let read = tpkt_read(&mut s).await;
        assert_eq!(
            &read[13..],
            &[4, 1, 0x12, 0x0a, 0x10, 2, 0, 4, 0, 1, 0x84, 0, 0, 0]
        );
        let mut ack = vec![
            2, 0xf0, 0x80, 0x32, 3, 0, 0, read[7], read[8], 0, 2, 0, 8, 0, 0, 4, 1, 0xff, 4, 0, 32,
        ];
        ack.extend([0x41, 0xcc, 0, 0]);
        tpkt_write(&mut s, &ack).await;
    });
    let mut c = Client::connect(&device("s7", port)).await.unwrap();
    assert_eq!(
        c.read(&point("DB1.DBD0", "float")).await.unwrap(),
        json!(25.5)
    );
    server.await.unwrap();
}

#[test]
fn backoff_saturates_without_overflow() {
    let r = rustlink::model::Reconnect::default();
    let got: Vec<_> = (0..8).map(|i| r.delay(i).as_secs()).collect();
    assert_eq!(got, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    assert_eq!(r.delay(u64::MAX).as_secs(), 60);
}

#[test]
fn rtu_silence_follows_serial_line_timing() {
    use rustlink::protocol::rtu_silent_interval;
    assert!(rtu_silent_interval(9600) >= std::time::Duration::from_micros(4010));
    assert!(rtu_silent_interval(19200) >= std::time::Duration::from_micros(2005));
    assert_eq!(
        rtu_silent_interval(115200),
        std::time::Duration::from_micros(1750)
    );
}

#[tokio::test]
async fn stalled_read_has_a_finite_deadline() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let mut c = Client::connect(&device("modbus_tcp", port)).await.unwrap();
    let _peer = l.accept().await.unwrap();
    let begin = std::time::Instant::now();
    assert!(
        c.read(&point("40001", "float"))
            .await
            .unwrap_err()
            .to_string()
            .contains("超时")
    );
    assert!(begin.elapsed() < std::time::Duration::from_secs(4));
}

#[cfg(unix)]
#[tokio::test]
async fn modbus_rtu_opens_and_reads_a_real_pseudoterminal() {
    use tokio_serial::SerialPort;
    let (mut master, slave) = tokio_serial::SerialStream::pair().unwrap();
    let name = slave.name().unwrap();
    drop(slave);
    let config:Device=serde_json::from_value(json!({"id":"serial","name":"serial","protocol":"modbus_rtu","com_port":name,"baudrate":9600,"slave_id":1})).unwrap();
    let mut client = Client::connect(&config).await.unwrap();
    let server = tokio::spawn(async move {
        let mut request = [0; 8];
        master.read_exact(&mut request).await.unwrap();
        assert_eq!(request, [1, 3, 0, 0, 0, 1, 0x84, 0x0a]);
        let mut reply = vec![1, 3, 2, 0, 42];
        reply.extend(crc16(&reply).to_le_bytes());
        master.write_all(&reply).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
    assert_eq!(
        client.read(&point("40001", "uint16")).await.unwrap(),
        json!(42.0)
    );
    server.await.unwrap();
}
