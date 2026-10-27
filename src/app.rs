use crate::{
    ai::{AiService, ParseRequest},
    engine::Engine,
    logs::LogStore,
    model::{self, Ai, Device, Gateway, Mqtt},
    protocol::Client,
    store::ConfigStore,
};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Multipart, OriginalUri, Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post, put},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    convert::Infallible,
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ConfigStore>,
    pub engine: Engine,
    pub logs: Arc<LogStore>,
    pub ai: AiService,
}
impl AppState {
    pub fn open(dir: &FsPath) -> anyhow::Result<Self> {
        Ok(Self {
            store: Arc::new(ConfigStore::open(&dir.join("config.json"))?),
            engine: Engine::new(),
            logs: Arc::new(LogStore::open(&dir.join("logs"), 10 * 1024 * 1024)?),
            ai: AiService::new()?,
        })
    }
    async fn mutate<F>(&self, change: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Value) -> anyhow::Result<()> + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.update(change)).await??;
        Ok(())
    }
}
fn ok(message: &str) -> Json<Value> {
    Json(model::response(true, message, Value::Null))
}
fn data(value: Value) -> Json<Value> {
    Json(model::response(true, "", value))
}
fn fail(message: impl Into<String>) -> Json<Value> {
    Json(model::response(false, message, Value::Null))
}
fn saved(result: anyhow::Result<()>, message: &str) -> Json<Value> {
    match result {
        Ok(()) => ok(message),
        Err(e) => fail(format!("操作失败: {e}")),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health",get(||async{ok("网关运行中")}))
        .route("/api/config",get(config_get).put(config_put))
        .route("/api/config/reload",post(config_reload))
        .route("/api/config/apply",post(config_apply))
        .route("/api/devices",get(devices_get).post(device_add))
        .route("/api/devices/{device_id}",put(device_update).delete(device_delete))
        .route("/api/devices/test",post(device_test))
        .route("/api/devices/import-excel",post(excel_import))
        .route("/api/devices/template",get(excel_template))
        .route("/api/collect/start",post(collect_start))
        .route("/api/collect/stop",post(collect_stop))
        .route("/api/collect/status",get(collect_status))
        .route("/api/mqtt",get(mqtt_get).put(mqtt_put))
        .route("/api/logs",get(logs_get))
        .route("/api/logs/level",post(logs_level))
        .route("/api/ai/config",get(ai_get).put(ai_put))
        .route("/api/ai/parse",post(ai_parse))
        .route("/api/ai/parse-stream",post(ai_stream))
        .route("/api/system/autostart",get(autostart_get).post(autostart_put))
        .route("/api/metrics",get(metrics))
        .route("/ws/data",get(ws_upgrade))
        .route("/openapi.json",get(||async{Json(serde_json::from_str::<Value>(include_str!("../tests/fixtures/openapi.json")).unwrap())}))
        .route("/docs",get(||async{axum::response::Html("<!doctype html><html><head><title>RustLink API</title><link rel='stylesheet' href='https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css'></head><body><div id='swagger-ui'></div><script src='https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js'></script><script>SwaggerUIBundle({url:'/openapi.json',dom_id:'#swagger-ui'});</script></body></html>")}))
        .fallback(get(static_file))
        .layer(DefaultBodyLimit::max(16*1024*1024))
        .with_state(state)
}
static WEB: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/web/dist");
async fn static_file(OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if matches!(path, "" | "devices" | "settings") {
        "index.html"
    } else {
        path
    };
    match WEB.get_file(path) {
        Some(file) => {
            let mime = match std::path::Path::new(path)
                .extension()
                .and_then(|v| v.to_str())
            {
                Some("html") => "text/html; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("wasm") => "application/wasm",
                _ => "application/octet-stream",
            };
            (
                [("content-type", mime), ("cache-control", "no-cache")],
                file.contents(),
            )
                .into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
async fn excel_template() -> Response {
    (
        [
            (
                "content-type",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            (
                "content-disposition",
                "attachment; filename=points-template.xlsx",
            ),
        ],
        include_bytes!("../assets/point-template.xlsx").as_slice(),
    )
        .into_response()
}
async fn config_get(State(s): State<AppState>) -> Json<Value> {
    data(s.store.snapshot())
}
async fn config_put(State(s): State<AppState>, Json(cfg): Json<Gateway>) -> Json<Value> {
    let name = cfg.gateway_name.clone();
    let interval = cfg.collect_interval;
    let result = s
        .mutate(move |v| {
            v.as_object_mut()
                .unwrap()
                .extend(serde_json::to_value(cfg)?.as_object().unwrap().clone());
            Ok(())
        })
        .await;
    if result.is_ok() {
        s.engine.update_settings(name, interval);
    }
    saved(result, "配置已保存")
}
async fn config_reload(State(s): State<AppState>) -> Json<Value> {
    let store = s.store.clone();
    let result = tokio::task::spawn_blocking(move || store.reload())
        .await
        .unwrap_or_else(|e| Err(e.into()));
    if result.is_ok() {
        let cfg = s.store.snapshot();
        s.engine.update_settings(
            cfg["gateway_name"].as_str().unwrap_or("").into(),
            cfg["collect_interval"].as_u64().unwrap_or(1000),
        );
    }
    saved(result, "配置已重新加载")
}
async fn config_apply(State(s): State<AppState>) -> Json<Value> {
    let cfg = match serde_json::from_value::<Gateway>(s.store.snapshot()) {
        Ok(c) => c,
        Err(e) => return fail(e.to_string()),
    };
    if let Err(e) = s.mutate(|_| Ok(())).await {
        return fail(e.to_string());
    }
    saved(s.engine.restart(cfg).await, "配置已应用，采集器正在重启")
}
async fn devices_get(State(s): State<AppState>) -> Json<Value> {
    data(
        s.store
            .snapshot()
            .get("devices")
            .cloned()
            .unwrap_or(json!([])),
    )
}
async fn device_add(State(s): State<AppState>, Json(d): Json<Device>) -> Json<Value> {
    let value = serde_json::to_value(d).unwrap();
    let out = value.clone();
    match s
        .mutate(move |v| {
            if v.get("devices").is_none() {
                v["devices"] = json!([])
            }
            v["devices"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("devices must be array"))?
                .push(value);
            Ok(())
        })
        .await
    {
        Ok(()) => Json(model::response(true, "设备已添加", out)),
        Err(e) => fail(e.to_string()),
    }
}
async fn device_update(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(mut d): Json<Device>,
) -> Json<Value> {
    d.id = id.clone();
    let store = s.store.clone();
    let result = tokio::task::spawn_blocking(move || {
        store.update_if(|v| {
            let devices = v["devices"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("devices must be array"))?;
            if let Some(old) = devices.iter_mut().find(|d| d["id"] == id) {
                *old = serde_json::to_value(d)?;
                Ok((true, true))
            } else {
                Ok((false, false))
            }
        })
    })
    .await;
    match result {
        Ok(Ok(true)) => ok("设备已更新"),
        Ok(Ok(false)) => fail("设备不存在"),
        _ => fail("设备更新失败"),
    }
}
async fn device_delete(State(s): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    let store = s.store.clone();
    let result = tokio::task::spawn_blocking(move || {
        store.update_if(|v| {
            let devices = v["devices"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("devices must be array"))?;
            let len = devices.len();
            devices.retain(|d| d["id"] != id);
            let removed = len != devices.len();
            Ok((removed, removed))
        })
    })
    .await;
    match result {
        Ok(Ok(true)) => ok("设备已删除"),
        Ok(Ok(false)) => fail("设备不存在"),
        _ => fail("设备删除失败"),
    }
}
#[derive(Deserialize)]
struct TestRequest {
    device: Device,
}
async fn device_test(Json(req): Json<TestRequest>) -> Json<Value> {
    if !matches!(
        req.device.protocol.as_str(),
        "modbus_tcp" | "modbus_rtu" | "s7" | "mitsubishi"
    ) {
        return Json(
            json!({"success":false,"message":format!("不支持的协议: {}",req.device.protocol),"elapsed_ms":0}),
        );
    }
    let start = Instant::now();
    let result = Client::connect(&req.device).await;
    let elapsed = start.elapsed().as_millis() as u64;
    Json(
        json!({"success":result.is_ok(),"message":format!("{}，耗时 {elapsed}ms",if result.is_ok(){"连接成功"}else{"连接失败"}),"elapsed_ms":elapsed}),
    )
}
async fn collect_start(State(s): State<AppState>) -> Json<Value> {
    match serde_json::from_value(s.store.snapshot()) {
        Ok(cfg) => saved(s.engine.start(cfg).await, "采集已启动"),
        Err(e) => fail(e.to_string()),
    }
}
async fn collect_stop(State(s): State<AppState>) -> Json<Value> {
    s.engine.stop().await;
    ok("采集已停止")
}
async fn collect_status(State(s): State<AppState>) -> Json<Value> {
    data(s.engine.status().await)
}
async fn mqtt_get(State(s): State<AppState>) -> Json<Value> {
    data(s.store.snapshot().get("mqtt").cloned().unwrap_or(json!({})))
}
async fn mqtt_put(State(s): State<AppState>, Json(cfg): Json<Mqtt>) -> Json<Value> {
    saved(
        s.mutate(move |v| {
            v["mqtt"] = serde_json::to_value(cfg)?;
            Ok(())
        })
        .await,
        "MQTT配置已保存",
    )
}
async fn ai_get(State(s): State<AppState>) -> Json<Value> {
    data(s.store.snapshot().get("ai").cloned().unwrap_or(json!({})))
}
async fn ai_put(State(s): State<AppState>, Json(cfg): Json<Ai>) -> Json<Value> {
    saved(
        s.mutate(move |v| {
            v["ai"] = serde_json::to_value(cfg)?;
            Ok(())
        })
        .await,
        "AI配置已保存",
    )
}
fn ai_settings(s: &AppState, req: &ParseRequest) -> Ai {
    req.settings(
        serde_json::from_value(s.store.snapshot().get("ai").cloned().unwrap_or(json!({})))
            .unwrap_or_default(),
    )
}
async fn ai_parse(State(s): State<AppState>, Json(req): Json<ParseRequest>) -> Json<Value> {
    let cfg = ai_settings(&s, &req);
    match s.ai.parse(req, cfg).await {
        Ok(v) => Json(model::response(true, "解析成功", v)),
        Err(e) => fail(e),
    }
}
async fn ai_stream(State(s): State<AppState>, Json(req): Json<ParseRequest>) -> Response {
    let cfg = ai_settings(&s, &req);
    let stream =
        s.ai.stream(req, cfg)
            .map(|v| Ok::<_, Infallible>(Event::default().data(v.to_string())));
    Sse::new(stream).into_response()
}
async fn logs_get(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let lines = q.get("lines").and_then(|s| s.parse().ok()).unwrap_or(200);
    let logs = s.logs.clone();
    match tokio::task::spawn_blocking(move || logs.read(q.get("level").map(String::as_str), lines))
        .await
    {
        Ok(Ok(v)) => data(json!(v)),
        _ => data(json!([])),
    }
}
async fn logs_level(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let level = q.get("level").map(String::as_str).unwrap_or("INFO");
    s.logs.set_level(level);
    ok(&format!("日志级别已设置为 {level}"))
}
async fn excel_import(mut multipart: Multipart) -> Json<Value> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("").to_string();
        let bytes = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return fail(format!("Excel解析失败: {e}")),
        };
        return match tokio::task::spawn_blocking(move || crate::excel::import(&name, &bytes)).await
        {
            Ok(v) => Json(v),
            Err(e) => fail(e.to_string()),
        };
    }
    fail("未选择文件")
}
async fn autostart_get() -> Json<Value> {
    data(json!({"enabled":autostart_enabled()}))
}
async fn autostart_put(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let enabled = !matches!(
        q.get("enabled").map(String::as_str),
        Some("false" | "0" | "False")
    );
    saved(
        set_autostart(enabled),
        if enabled {
            "已设置开机自启动"
        } else {
            "已取消开机自启动"
        },
    )
}
#[cfg(windows)]
fn autostart_enabled() -> bool {
    use winreg::{RegKey, enums::*};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .and_then(|k| k.get_value::<String, _>("工业数据采集网关"))
        .is_ok()
}
#[cfg(not(windows))]
fn autostart_enabled() -> bool {
    false
}
#[cfg(windows)]
fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    use winreg::{RegKey, enums::*};
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        KEY_SET_VALUE,
    )?;
    if enabled {
        let exe = std::env::current_exe()?;
        key.set_value("工业数据采集网关", &format!("\"{}\"", exe.display()))?;
    } else {
        match key.delete_value("工业数据采集网关") {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn set_autostart(_: bool) -> anyhow::Result<()> {
    anyhow::bail!("当前系统不支持 Windows 开机自启动")
}
async fn metrics(State(s): State<AppState>) -> Json<Value> {
    data(s.engine.metrics())
}
async fn ws_upgrade(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, s))
}
async fn ws_session(mut socket: WebSocket, s: AppState) {
    let mut rx = s.engine.subscribe();
    loop {
        tokio::select! {
            input=socket.recv()=>match input{Some(Ok(Message::Text(t))) if t=="ping"=>{if tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Text("pong".into()))).await.is_err(){break}},Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}},
            event=rx.recv()=>match event{Ok(v)=>{if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Text(v.to_string().into()))).await,Ok(Ok(()))){break}},Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break}
        }
    }
}
