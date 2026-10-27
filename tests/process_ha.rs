use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Process(Child);
// Both tests deliberately allocate adjacent fallback ports; serialize the
// reservations so the fallback process cannot occupy the other test's port.
static PORT_RESERVATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn launch(dir: &Path, port: u16, standby: bool, strict: bool) -> Process {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rustlink"));
    c.args(["--headless", "--auto-start", "--data-dir"])
        .arg(dir)
        .args(["--port", &port.to_string()]);
    if standby {
        c.arg("--standby");
    }
    if strict {
        c.arg("--strict-port");
    }
    c.stdout(Stdio::null()).stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    Process(c.spawn().unwrap())
}
async fn healthy(port: u16) -> Value {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/api/collect/status"))
            .send()
            .await
            && let Ok(value) = response.json::<Value>().await
        {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "process did not become healthy on {port}"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
#[tokio::test]
async fn primary_crash_releases_lock_and_standby_takes_over_same_port() {
    let _ports = PORT_RESERVATIONS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut primary = launch(dir.path(), port, false, true);
    assert_eq!(healthy(port).await["data"]["running"], true);
    let client = reqwest::Client::new();
    let saved = client
        .put(format!("http://127.0.0.1:{port}/api/config"))
        .json(&json!({"gateway_name":"主备测试"}))
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success());
    let mut standby = launch(dir.path(), port, true, true);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(standby.0.try_wait().unwrap().is_none());
    primary.0.kill().unwrap();
    primary.0.wait().unwrap();
    let begin = Instant::now();
    assert_eq!(healthy(port).await["data"]["running"], true);
    assert!(
        begin.elapsed() < Duration::from_secs(3),
        "failover took {:?}",
        begin.elapsed()
    );
    let config: Value = client
        .get(format!("http://127.0.0.1:{port}/api/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["data"]["gateway_name"], "主备测试");
    assert!(standby.0.try_wait().unwrap().is_none());
}
#[tokio::test]
async fn occupied_port_falls_back_and_strict_mode_exits() {
    let _ports = PORT_RESERVATIONS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port();
    if port == 65535 {
        return;
    }
    let _p = launch(dir.path(), port, false, false);
    assert_eq!(healthy(port + 1).await["success"], true);
    let dir2 = tempfile::tempdir().unwrap();
    let mut strict = launch(dir2.path(), port, false, true);
    let start = Instant::now();
    loop {
        if let Some(status) = strict.0.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_performs_bounded_graceful_shutdown() {
    let _ports = PORT_RESERVATIONS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut process = launch(dir.path(), port, false, true);
    healthy(port).await;
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(7));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(rustlink::store::ConfigStore::open(&dir.path().join("config.json")).is_ok());
}
