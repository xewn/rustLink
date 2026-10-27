use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use futures_util::{SinkExt, StreamExt};
use rustlink::{
    ai::{AiService, ParseRequest},
    app::{AppState, router},
    engine::Engine,
    model::{Ai, Gateway},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};

async fn start(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    (
        format!("http://{addr}"),
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() }),
    )
}
#[tokio::test]
async fn ai_http_json_and_fragmented_utf8_sse() {
    async fn mock(headers: HeaderMap, Json(body): Json<Value>) -> axum::response::Response {
        assert_eq!(headers["authorization"], "Bearer test-key");
        assert_eq!(body["model"], "mock");
        assert_eq!(body["temperature"], 0.1);
        assert_eq!(body["messages"][0]["content"], rustlink::ai::SYSTEM_PROMPT);
        if body["stream"] == true {
            let config = "{\"name\":\"温度设备\",\"points\":[]}";
            let mut text = String::new();
            for chunk in [config.get(..9).unwrap(), config.get(9..).unwrap()] {
                text.push_str(&format!(
                    "data: {}\r\n\r\n",
                    json!({"choices":[{"delta":{"content":chunk}}]})
                ));
            }
            text.push_str("data: [DONE]\n\n");
            let chunks = text
                .into_bytes()
                .into_iter()
                .map(|b| Ok::<_, std::io::Error>(vec![b]));
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(futures_util::stream::iter(chunks)),
            )
                .into_response()
        } else {
            Json(json!({"choices":[{"message":{"content":"```json\n{\"name\":\"温度设备\"}\n```"}}]})).into_response()
        }
    }
    let (url, server) = start(Router::new().route("/chat/completions", post(mock))).await;
    let cfg = Ai {
        api_url: url,
        api_key: "test-key".into(),
        model: "mock".into(),
        ..Ai::default()
    };
    let req = ParseRequest {
        input: "测试".into(),
        api_url: None,
        api_key: None,
        model: None,
    };
    let service = AiService::new().unwrap();
    let config = service.parse(req.clone(), cfg.clone()).await.unwrap();
    assert_eq!(
        config,
        json!({"name":"温度设备","enabled":true,"points":[]})
    );
    let stream = service.stream(req, cfg);
    tokio::pin!(stream);
    let mut events = vec![];
    while let Some(v) = stream.next().await {
        events.push(v);
    }
    assert_eq!(
        events.last().unwrap(),
        &json!({"type":"done","config":config})
    );
    assert_eq!(events.iter().filter(|v| v["type"] == "chunk").count(), 2);
    server.abort();
}
#[tokio::test]
async fn ai_http_errors_and_malformed_json() {
    for (status, message) in [
        (401, "API Key无效，请检查AI设置"),
        (404, "API地址无效，请检查AI设置中的API地址"),
        (503, "AI API请求失败: HTTP 503"),
        (200, "AI返回格式异常，请重试"),
    ] {
        let (url, server) = start(Router::new().route(
            "/chat/completions",
            post(move || async move { (StatusCode::from_u16(status).unwrap(), "invalid") }),
        ))
        .await;
        let cfg = Ai {
            api_url: url,
            api_key: "key".into(),
            ..Ai::default()
        };
        let req = ParseRequest {
            input: "test".into(),
            api_url: None,
            api_key: None,
            model: None,
        };
        assert_eq!(
            AiService::new().unwrap().parse(req, cfg).await.unwrap_err(),
            message
        );
        server.abort();
    }
}
#[tokio::test]
async fn websocket_ping_and_real_acquisition_status() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path()).unwrap();
    let (url, server) = start(router(state.clone())).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("{}/ws/data", url.replace("http:", "ws:")))
            .await
            .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text("ping".into()))
        .await
        .unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap().to_text().unwrap(), "pong");
    let cfg:Gateway=serde_json::from_value(json!({"reconnect":{"max_retries":1,"base_delay":0},"devices":[{"id":"bad","name":"bad","protocol":"modbus_rtu","com_port":"not-a-real-port"}]})).unwrap();
    state.engine.start(cfg).await.unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            let event: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            if event["type"] == "status" && event["status"] == "error" {
                break;
            }
        }
    })
    .await
    .unwrap();
    ws.close(None).await.unwrap();
    state.engine.stop().await;
    server.abort();
}
async fn packet(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let first = stream.read_u8().await.unwrap();
    let mut len = 0;
    let mut shift = 0;
    loop {
        let n = stream.read_u8().await.unwrap();
        len |= ((n & 127) as usize) << shift;
        if n & 128 == 0 {
            break;
        }
        shift += 7;
        assert!(shift <= 21);
    }
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).await.unwrap();
    (first, payload)
}
#[tokio::test]
async fn mqtt_local_broker_qos_auth_payload_and_connection_state() {
    for qos in 0..=2 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let broker = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (kind, connect) = packet(&mut socket).await;
            assert_eq!(kind, 0x10);
            assert_eq!(&connect[..7], &[0, 4, b'M', b'Q', b'T', b'T', 4]);
            assert_eq!(connect[7] & 0xc0, 0xc0);
            assert!(connect.windows(4).any(|w| w == b"user"));
            assert!(connect.windows(4).any(|w| w == b"pass"));
            tokio::time::sleep(Duration::from_millis(1100)).await;
            socket.write_all(&[0x20, 2, 0, 0]).await.unwrap();
            let mut found = false;
            for _ in 0..6 {
                let (kind, payload) = packet(&mut socket).await;
                if kind >> 4 == 6 {
                    assert_eq!(qos, 2);
                    assert_eq!(payload.len(), 2);
                    socket
                        .write_all(&[0x70, 2, payload[0], payload[1]])
                        .await
                        .unwrap();
                    if found { break } else { continue }
                }
                assert_eq!(kind >> 4, 3);
                assert_eq!((kind >> 1) & 3, qos);
                let n = u16::from_be_bytes([payload[0], payload[1]]) as usize;
                let topic = std::str::from_utf8(&payload[2..n + 2]).unwrap();
                let offset = if qos == 0 { n + 2 } else { n + 4 };
                let data: Value = serde_json::from_slice(&payload[offset..]).unwrap();
                if qos > 0 {
                    let id = &payload[n + 2..n + 4];
                    socket
                        .write_all(&[if qos == 2 { 0x50 } else { 0x40 }, 2, id[0], id[1]])
                        .await
                        .unwrap();
                }
                if topic == "test/plc/status" {
                    assert_eq!(data["device_id"], "plc");
                    assert!(data.get("type").is_none());
                    found = true;
                    if qos < 2 {
                        break;
                    }
                }
            }
            assert!(found);
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let cfg:Gateway=serde_json::from_value(json!({"mqtt":{"enabled":true,"host":"127.0.0.1","port":port,"username":"user","password":"pass","topic_prefix":"test","qos":qos},"reconnect":{"max_retries":2,"base_delay":1},"devices":[{"id":"plc","name":"plc","protocol":"modbus_rtu","com_port":"no-such-port"}]})).unwrap();
        let e = Engine::new();
        e.start(cfg).await.unwrap();
        timeout(Duration::from_secs(3), async {
            while e.status().await["mqtt_connected"] != true {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        timeout(Duration::from_secs(3), broker)
            .await
            .unwrap()
            .unwrap();
        e.stop().await;
    }
}

#[tokio::test]
async fn mqtt_disconnect_is_reported_and_reconnect_recovers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (drop_first, when_drop) = tokio::sync::oneshot::channel();
    let (stop_second, when_stop) = tokio::sync::oneshot::channel();
    let broker = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        assert_eq!(packet(&mut first).await.0, 0x10);
        first.write_all(&[0x20, 2, 0, 0]).await.unwrap();
        when_drop.await.unwrap();
        drop(first);
        let (mut second, _) = listener.accept().await.unwrap();
        assert_eq!(packet(&mut second).await.0, 0x10);
        second.write_all(&[0x20, 2, 0, 0]).await.unwrap();
        let _ = when_stop.await;
    });
    let e = Engine::new();
    let cfg: Gateway =
        serde_json::from_value(json!({"mqtt":{"enabled":true,"host":"127.0.0.1","port":port}}))
            .unwrap();
    e.start(cfg).await.unwrap();
    async fn wait(e: &Engine, value: bool) {
        timeout(Duration::from_secs(4), async {
            while e.status().await["mqtt_connected"] != value {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    wait(&e, true).await;
    drop_first.send(()).unwrap();
    wait(&e, false).await;
    wait(&e, true).await;
    e.stop().await;
    let _ = stop_second.send(());
    broker.await.unwrap();
}
