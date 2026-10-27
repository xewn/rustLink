use anyhow::{Context, Result, bail};
use rustlink::{
    app::{AppState, router},
    desktop,
};
use std::{path::PathBuf, time::Duration};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Options {
    directory: PathBuf,
    host: String,
    port: u16,
    headless: bool,
    strict: bool,
    standby: bool,
    auto_start: bool,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut value = Self {
            directory: std::env::current_exe()?
                .parent()
                .context("executable directory")?
                .to_path_buf(),
            host: "127.0.0.1".into(),
            port: 8000,
            headless: false,
            strict: false,
            standby: false,
            auto_start: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    value.directory =
                        PathBuf::from(args.next().context("--data-dir requires path")?)
                }
                "--host" => value.host = args.next().context("--host requires address")?,
                "--port" => value.port = args.next().context("--port requires number")?.parse()?,
                "--headless" => value.headless = true,
                "--strict-port" => value.strict = true,
                "--standby" => value.standby = true,
                "--auto-start" => value.auto_start = true,
                "--help" | "-h" => {
                    println!(
                        "RustLink 工业数据采集网关\n\n--data-dir PATH  配置和日志目录（默认程序所在目录）\n--host ADDRESS   监听地址（默认127.0.0.1）\n--port PORT      首选端口（默认8000）\n--strict-port    端口占用时退出\n--headless       不显示托盘，不打开浏览器\n--auto-start     启动后立即采集\n--standby        配置被其他实例占用时等待，释放后接管"
                    );
                    std::process::exit(0)
                }
                "--version" => {
                    println!("RustLink {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0)
                }
                _ => bail!("unknown option: {arg}"),
            }
        }
        Ok(value)
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::parse()?;
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! { _=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{} }
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let state = loop {
        match AppState::open(&options.directory) {
            Ok(s) => break s,
            Err(e) => {
                let locked = e
                    .chain()
                    .filter_map(|e| e.downcast_ref::<std::io::Error>())
                    .any(|e| e.raw_os_error() == fs2::lock_contended_error().raw_os_error());
                if !options.standby || !locked {
                    return Err(e);
                }
                tokio::select! {_=shutdown.cancelled()=>return Ok(()),_=tokio::time::sleep(Duration::from_millis(250))=>{}}
            }
        }
    };
    let mut listener = None;
    for offset in 0..if options.strict { 1 } else { 100 } {
        let Some(port) = options.port.checked_add(offset) else {
            break;
        };
        match TcpListener::bind((options.host.as_str(), port)).await {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let listener = listener.context("没有可用监听端口")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    let _ = state
        .logs
        .write("INFO", &format!("网关已启动，监听 {}:{port}", options.host));
    println!(
        "RustLink {} · {url}\n配置目录: {}",
        env!("CARGO_PKG_VERSION"),
        options.directory.display()
    );
    if options.auto_start {
        state
            .engine
            .start(serde_json::from_value(state.store.snapshot())?)
            .await?;
    }
    let mut events = state.engine.subscribe();
    let logs = state.logs.clone();
    let done = shutdown.clone();
    let logger = tokio::spawn(async move {
        loop {
            tokio::select! {_=done.cancelled()=>break,event=events.recv()=>match event{
                Ok(v) if v["type"]=="status"=>{let logs=logs.clone();let _=tokio::task::spawn_blocking(move||logs.write(if v["status"]=="error"{"ERROR"}else{"INFO"},&format!("设备 {}: {}",v["device_id"].as_str().unwrap_or(""),v["message"].as_str().unwrap_or("")))).await;},
                Ok(_)|Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{},Err(_)=>break
            }}
        }
    });
    if !options.headless {
        desktop::start(state.clone(), url.clone(), shutdown.clone());
        desktop::open_browser(&url);
    }
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, router(state.clone()))
            .with_graceful_shutdown(shutdown.clone().cancelled_owned()),
    );
    tokio::pin!(server);
    let result = tokio::select! {
        result=&mut server=>result,
        _=shutdown.cancelled()=>{
            state.engine.stop().await;
            tokio::time::timeout(Duration::from_secs(5),&mut server).await.unwrap_or(Ok(()))
        }
    };
    shutdown.cancel();
    state.engine.stop().await;
    let _ = logger.await;
    let _ = state.logs.write("INFO", "网关已停止");
    result?;
    Ok(())
}
