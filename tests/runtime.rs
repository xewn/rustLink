use rustlink::{engine::Engine, model::Gateway};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{Duration, timeout},
};

async fn simulator() -> (u16, tokio::task::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut request = [0; 12];
                while s.read_exact(&mut request).await.is_ok() {
                    let r = [request[0], request[1], 0, 0, 0, 5, 1, 3, 2, 0, 42];
                    if s.write_all(&r).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (port, task)
}
#[tokio::test]
async fn slow_plc_is_isolated_and_stop_cancels_inflight_io() {
    let (port, server) = simulator().await;
    let slow = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_port = slow.local_addr().unwrap().port();
    let cfg:Gateway=serde_json::from_value(json!({"collect_interval":10,"devices":[
        {"id":"slow","name":"slow","protocol":"modbus_tcp","ip":"127.0.0.1","port":slow_port,"points":[{"name":"p","address":"40001","type":"uint16"}]},
        {"id":"fast","name":"fast","protocol":"modbus_tcp","ip":"127.0.0.1","port":port,"points":[{"name":"p","address":"40001","type":"uint16"}]}]})).unwrap();
    let e = Engine::new();
    let mut events = e.subscribe();
    e.start(cfg.clone()).await.unwrap();
    e.start(cfg).await.unwrap();
    timeout(Duration::from_millis(500), async {
        loop {
            let v = events.recv().await.unwrap();
            if v["type"] == "data" && v["device_id"] == "fast" {
                assert_eq!(v["values"]["p"]["value"], 42.0);
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(e.status().await["devices"].as_array().unwrap().len(), 2);
    timeout(Duration::from_millis(250), e.stop()).await.unwrap();
    assert_eq!(e.status().await["running"], false);
    assert!(e.status().await["devices"].as_array().unwrap().is_empty());
    server.abort();
}
#[tokio::test]
async fn reconnect_after_server_appears_and_repeated_lifecycle() {
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let cfg:Gateway=serde_json::from_value(json!({"collect_interval":10,"reconnect":{"base_delay":0,"max_delay":0},"devices":[{"id":"d","name":"device","protocol":"modbus_tcp","ip":"127.0.0.1","port":addr.port(),"points":[{"name":"p","address":"40001","type":"uint16"}]}]})).unwrap();
    let e = Engine::new();
    let mut events = e.subscribe();
    e.start(cfg.clone()).await.unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            if events.recv().await.unwrap()["status"] == "offline" {
                break;
            }
        }
    })
    .await
    .unwrap();
    let l = TcpListener::bind(addr).await.unwrap();
    let server = tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut r = [0; 12];
        s.read_exact(&mut r).await.unwrap();
        s.write_all(&[r[0], r[1], 0, 0, 0, 5, 1, 3, 2, 0, 7])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    timeout(Duration::from_secs(1), async {
        loop {
            let v = events.recv().await.unwrap();
            if v["type"] == "data" && v["values"]["p"]["quality"] == "good" {
                break;
            }
        }
    })
    .await
    .unwrap();
    e.stop().await;
    server.abort();
    for _ in 0..5 {
        e.start(cfg.clone()).await.unwrap();
        e.stop().await;
    }
    assert_eq!(e.status().await["running"], false);
}
#[tokio::test]
async fn retry_limit_and_disabled_devices() {
    let cfg:Gateway=serde_json::from_value(json!({"reconnect":{"max_retries":1,"base_delay":0},"devices":[{"id":"bad","name":"bad","protocol":"modbus_rtu","com_port":"missing-port-xyz"},{"id":"disabled","name":"disabled","protocol":"s7","enabled":false}]})).unwrap();
    let e = Engine::new();
    let mut events = e.subscribe();
    e.start(cfg).await.unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if events.recv().await.unwrap()["status"] == "error" {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(e.status().await["devices"].as_array().unwrap().len(), 1);
    e.stop().await;
}

#[tokio::test]
async fn slow_subscriber_cannot_backpressure_acquisition() {
    let (port, server) = simulator().await;
    let cfg:Gateway=serde_json::from_value(json!({"collect_interval":1,"devices":[{"id":"fast","name":"fast","protocol":"modbus_tcp","ip":"127.0.0.1","port":port,"points":[{"name":"p","address":"40001","type":"uint16"}]}]})).unwrap();
    let e = Engine::new();
    let mut slow = e.subscribe();
    e.start(cfg).await.unwrap();
    timeout(Duration::from_secs(5), async {
        while e.metrics()["samples"].as_u64().unwrap() < 400 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        slow.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
    assert_eq!(e.status().await["devices"][0]["connected"], true);
    e.stop().await;
    server.abort();
}

#[tokio::test]
async fn gateway_name_and_interval_update_without_reconnecting_devices() {
    let (port, server) = simulator().await;
    let cfg:Gateway=serde_json::from_value(json!({"collect_interval":60000,"devices":[{"id":"d","name":"d","protocol":"modbus_tcp","ip":"127.0.0.1","port":port,"points":[{"name":"p","address":"40001","type":"uint16"}]}]})).unwrap();
    let e = Engine::new();
    let mut events = e.subscribe();
    e.start(cfg).await.unwrap();
    timeout(Duration::from_secs(1), async {
        loop {
            if events.recv().await.unwrap()["type"] == "data" {
                break;
            }
        }
    })
    .await
    .unwrap();
    e.update_settings("新网关".into(), 10);
    timeout(Duration::from_millis(300), async {
        loop {
            let v = events.recv().await.unwrap();
            if v["type"] == "data" {
                assert_eq!(v["gateway"], "新网关");
                assert_eq!(v["values"]["p"]["value"], 42.0);
                break;
            }
        }
    })
    .await
    .unwrap();
    e.stop().await;
    server.abort();
}

#[tokio::test]
async fn device_status_preserves_configuration_order_and_last_duplicate() {
    let cfg:Gateway=serde_json::from_value(json!({"devices":[{"id":"z","name":"first","protocol":"s7"},{"id":"a","name":"second","protocol":"s7"},{"id":"z","name":"last","protocol":"s7"}]})).unwrap();
    let e = Engine::new();
    e.start(cfg).await.unwrap();
    let status = e.status().await;
    assert_eq!(status["devices"][0]["device_id"], "z");
    assert_eq!(status["devices"][0]["device_name"], "last");
    assert_eq!(status["devices"][1]["device_id"], "a");
    e.stop().await;
}
