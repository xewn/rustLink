use crate::{default_device, escape};
use futures_util::StreamExt;
use gloo_net::http::Request;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Element, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, MessageEvent, WebSocket,
};

#[derive(Default)]
struct Ui {
    page: String,
    config: Value,
    status: Value,
    latest: HashMap<String, Value>,
    edit: Option<Value>,
    editing_id: Option<String>,
    preview: Option<Vec<Value>>,
    autostart: bool,
}
struct Session {
    _socket: WebSocket,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _close: Closure<dyn FnMut(web_sys::Event)>,
}
thread_local! {static UI:RefCell<Ui>=RefCell::new(Ui{page:"home".into(),..Ui::default()});static SOCKET:RefCell<Option<Session>>=const{RefCell::new(None)};}
fn document() -> web_sys::Document {
    web_sys::window().unwrap().document().unwrap()
}
fn element(id: &str) -> Option<Element> {
    document().get_element_by_id(id)
}
fn string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}
fn html(v: &Value) -> String {
    escape(&string(v))
}
fn input(id: &str) -> String {
    let Some(e) = element(id) else {
        return String::new();
    };
    if let Some(v) = e.dyn_ref::<HtmlInputElement>() {
        v.value()
    } else if let Some(v) = e.dyn_ref::<HtmlSelectElement>() {
        v.value()
    } else if let Some(v) = e.dyn_ref::<HtmlTextAreaElement>() {
        v.value()
    } else {
        String::new()
    }
}
fn checked(id: &str) -> bool {
    element(id)
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
        .is_some_and(|e| e.checked())
}
fn number(id: &str) -> Result<Value, String> {
    let s = input(id);
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map(|n| {
            if n.fract() == 0.0 {
                json!(n as i64)
            } else {
                json!(n)
            }
        })
        .ok_or_else(|| format!("请输入有效数值：{id}"))
}
fn field(id: &str, label: &str, value: &Value, kind: &str) -> String {
    if kind == "checkbox" {
        format!(
            "<label class='field'>{label}<input id='{id}' type='checkbox' {}></label>",
            if value == true { "checked" } else { "" }
        )
    } else {
        format!(
            "<label class='field'>{label}<input id='{id}' type='{kind}' value='{}'></label>",
            html(value)
        )
    }
}
fn select(id: &str, label: &str, value: &Value, options: &[(&str, &str)]) -> String {
    format!(
        "<label class='field'>{label}<select id='{id}'>{}</select></label>",
        options
            .iter()
            .map(|(v, l)| format!(
                "<option value='{}' {}>{}</option>",
                escape(v),
                if string(value) == *v { "selected" } else { "" },
                escape(l)
            ))
            .collect::<String>()
    )
}
fn button(action: &str, label: &str) -> String {
    format!("<button data-action='{action}'>{label}</button>")
}
fn notice(message: &str, error: bool) {
    if let Some(e) = element("notice") {
        e.set_text_content(Some(message));
        e.set_class_name(if error { "error" } else { "" });
        let _ = e.set_attribute("style", "display:block");
        let owned = message.to_string();
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            if let Some(e) = element("notice")
                && e.text_content().as_deref() == Some(&owned)
            {
                let _ = e.set_attribute("style", "display:none");
            }
        });
    }
}
async fn api(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let request = match method {
        "POST" => Request::post(path),
        "PUT" => Request::put(path),
        "DELETE" => Request::delete(path),
        _ => Request::get(path),
    };
    let response = if let Some(body) = body {
        request.json(&body).map_err(|e| e.to_string())?.send().await
    } else {
        request.send().await
    }
    .map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err(format!(
            "请求失败（HTTP {}），请检查输入",
            response.status()
        ));
    }
    let value: Value = response.json().await.map_err(|e| e.to_string())?;
    if value["success"] == false {
        return Err(string(&value["message"]));
    }
    Ok(value)
}
async fn refresh() -> Result<(), String> {
    let cfg = api("GET", "/api/config", None).await?;
    let status = api("GET", "/api/collect/status", None).await?;
    let auto = api("GET", "/api/system/autostart", None).await?;
    UI.with_borrow_mut(|u| {
        u.config = cfg["data"].clone();
        u.status = status["data"].clone();
        u.autostart = auto["data"]["enabled"] == true;
    });
    render();
    Ok(())
}
fn render() {
    let markup=UI.with_borrow(|u|{
        if u.config.is_null(){return "<header><strong>RustLink 工业数据采集网关</strong></header><main><p>正在加载配置…</p></main><div id='notice' role='status'></div>".to_string();}
        let nav=[("home","实时数据"),("devices","设备配置"),("settings","系统设置")].iter().map(|(id,label)|format!("<button data-action='nav:{id}' class='{}'>{label}</button>",if u.page==*id{"active"}else{""})).collect::<String>();
        let content=match u.page.as_str(){"devices"=>devices(u),"settings"=>settings(u),_=>home(u)};
        let dialog=if let Some(d)=&u.edit{editor(d,u.editing_id.is_some())}else if let Some(points)=&u.preview{format!("<div class='overlay'><section class='dialog'><h2>Excel 导入预览 · {} 个点位</h2>{}<div class='form-actions'>{}{}</div></section></div>",points.len(),point_table(points,false),button("import-confirm","确认导入"),button("cancel","取消"))}else{String::new()};
        format!("<header><div><strong>RustLink 工业数据采集网关</strong><small>设备连接 · 实时采集 · 数据转发</small></div><span id='global-status' class='status'>服务已连接</span></header><aside>{nav}</aside><main>{content}</main>{dialog}<div id='notice' role='status'></div>")
    });
    element("app").unwrap().set_inner_html(&markup);
    update_live();
}
fn devices(u: &Ui) -> String {
    let devices = u.config["devices"].as_array().cloned().unwrap_or_default();
    let rows=devices.iter().enumerate().map(|(i,d)|format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class='actions'>{}{}{}{}</td></tr>",html(&d["name"]),html(&d["protocol"]),html(if d["protocol"]=="modbus_rtu"{&d["com_port"]}else{&d["ip"]}),d["points"].as_array().map_or(0,Vec::len),if d["enabled"]==false{"禁用"}else{"启用"},button(&format!("edit:{i}"),"编辑"),button(&format!("toggle:{i}"),if d["enabled"]==false{"启用"}else{"禁用"}),button(&format!("test:{i}"),"测试连接"),button(&format!("delete:{i}"),"删除"))).collect::<String>();
    let ai = if u.config["ai"]["enabled"] == true {
        format!(
            "<section class='card'><h3>AI 配置助手</h3><p class='muted'>描述设备型号、地址和需要采集的点位，生成配置后可编辑确认。</p><textarea id='ai-input' placeholder='例如：添加一台西门子 S7-1200，IP 192.168.1.100，采集温度和运行状态'></textarea><div class='form-actions'><button id='ai-generate' data-action='ai-generate' class='primary'>生成设备配置</button></div><pre id='ai-output'></pre><div id='ai-preview' hidden><h3>编辑并确认配置</h3><textarea id='ai-json' style='min-height:240px'></textarea><div class='form-actions'>{}</div></div></section>",
            button("ai-confirm", "填入设备表单")
        )
    } else {
        String::new()
    };
    format!(
        "<div class='toolbar'><h2>设备配置</h2><button class='primary' data-action='add'>添加设备</button><label class='button'>导入Excel<input id='excel-file' type='file' accept='.xlsx,.xls' style='display:none'></label><a class='button' href='/api/devices/template' download='点位表模板.xlsx'>下载模板</a></div>{ai}<section class='card scroll'>{}</section>",
        if devices.is_empty() {
            "<div class='empty'>暂无设备，请先添加设备。</div>".into()
        } else {
            format!(
                "<table><thead><tr><th>设备名称</th><th>协议</th><th>地址</th><th>点位数</th><th>状态</th><th>操作</th></tr></thead><tbody>{rows}</tbody></table>"
            )
        }
    )
}
fn home(u: &Ui) -> String {
    let devices = u.config["devices"].as_array().cloned().unwrap_or_default();
    let cards=devices.iter().enumerate().map(|(i,d)|{
        let points=d["points"].as_array().cloned().unwrap_or_default();
        let rows=points.iter().enumerate().map(|(j,p)|format!("<tr><td>{}</td><td id='value-{i}-{j}'>—</td><td>{}</td><td id='quality-{i}-{j}'>等待采集</td></tr>",html(&p["name"]),html(&p["unit"]))).collect::<String>();
        format!("<section class='card'><div class='toolbar'><h3>{}</h3><span id='device-status-{i}' class='status'>离线</span></div><p class='muted'>{} · {}</p><table><thead><tr><th>点位</th><th>值</th><th>单位</th><th>质量</th></tr></thead><tbody>{rows}</tbody></table><p id='time-{i}' class='muted small'>尚未收到数据</p></section>",html(&d["name"]),html(&d["protocol"]),html(&d["ip"]))
    }).collect::<String>();
    format!(
        "<div class='toolbar'><h2>实时数据</h2><span id='mqtt-status' class='status'>MQTT 未连接</span><button id='collect-toggle' class='primary' data-action='collect'>{}</button></div><div class='cards'>{}</div>",
        if u.status["running"] == true {
            "停止采集"
        } else {
            "启动采集"
        },
        if cards.is_empty() {
            "<section class='card empty'>尚未配置设备。请在设备配置中添加 PLC。</section>".into()
        } else {
            cards
        }
    )
}
fn settings(u: &Ui) -> String {
    let m = &u.config["mqtt"];
    let a = &u.config["ai"];
    let r = &u.config["reconnect"];
    let mut mqtt = field("mqtt-enabled", "启用 MQTT", &m["enabled"], "checkbox");
    for (k, l, t) in [
        ("host", "服务器地址", "text"),
        ("port", "端口", "number"),
        ("client_id", "客户端 ID", "text"),
        ("topic_prefix", "Topic 前缀", "text"),
        ("username", "用户名", "text"),
        ("password", "密码", "password"),
    ] {
        mqtt += &field(&format!("mqtt-{k}"), l, &m[k], t);
    }
    mqtt += &select(
        "mqtt-qos",
        "QoS",
        &m["qos"],
        &[
            ("0", "0 - 最多一次"),
            ("1", "1 - 至少一次"),
            ("2", "2 - 仅一次"),
        ],
    );
    let mut ai = field("ai-enabled", "启用 AI 助手", &a["enabled"], "checkbox");
    for (k, l, t) in [
        ("api_url", "API 地址", "url"),
        ("api_key", "API Key", "password"),
        ("model", "模型名称", "text"),
    ] {
        ai += &field(&format!("ai-{k}"), l, &a[k], t);
    }
    let reconnect = [
        ("max_retries", "最大重试次数（0 = 无限）"),
        ("base_delay", "基础延迟（秒）"),
        ("max_delay", "最大延迟（秒）"),
    ]
    .iter()
    .map(|(k, l)| field(&format!("reconnect-{k}"), l, &r[k], "number"))
    .collect::<String>();
    format!(
        "<h2>系统设置</h2><div class='section-grid'><section class='card'><h3>MQTT 配置</h3><div class='form'>{mqtt}</div><div class='form-actions'>{}</div></section><section class='card'><h3>采集设置</h3><div class='form'>{}{}</div><div class='form-actions'>{}</div><h3 style='margin-top:32px'>重连策略</h3><div class='form'>{reconnect}</div><div class='form-actions'>{}</div></section><section class='card'><h3>AI 配置助手</h3><div class='form'>{ai}</div><div class='form-actions'>{}</div></section><section class='card'><h3>开机自启动</h3>{}<div class='form-actions'>{}</div><h3 style='margin-top:32px'>应用配置</h3><p class='muted'>保存修改后，应用配置以重新启动采集器。</p><button class='primary' data-action='apply'>应用配置（保存并重启采集）</button></section><section class='card full'><div class='toolbar'><h3>日志查看</h3>{}{}{}{}</div><pre id='logs' class='logs'>点击刷新读取日志</pre></section></div>",
        button("save-mqtt", "保存MQTT配置"),
        field(
            "collect-gateway_name",
            "网关名称",
            &u.config["gateway_name"],
            "text"
        ),
        field(
            "collect-collect_interval",
            "采集间隔（毫秒）",
            &u.config["collect_interval"],
            "number"
        ),
        button("save-collect", "保存采集设置"),
        button("save-reconnect", "保存重连策略"),
        button("save-ai", "保存AI配置"),
        field(
            "autostart",
            "启用开机自启动",
            &json!(u.autostart),
            "checkbox"
        ),
        button("save-autostart", "保存启动设置"),
        select(
            "log-level",
            "级别",
            &json!(""),
            &[
                ("", "全部级别"),
                ("INFO", "INFO"),
                ("WARNING", "WARNING"),
                ("ERROR", "ERROR"),
                ("DEBUG", "DEBUG")
            ]
        ),
        button("logs", "刷新"),
        button("clear-logs", "清空显示"),
        button("set-log-level", "设置日志级别")
    )
}
fn point_table(points: &[Value], editable: bool) -> String {
    let rows = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let cells = ["name", "address", "type", "rate", "offset", "unit"]
                .iter()
                .map(|k| {
                    if editable {
                        let control = if *k == "type" {
                            select(
                                &format!("point-{i}-{k}"),
                                "",
                                &p[k],
                                &[
                                    ("bool", "bool"),
                                    ("int16", "int16"),
                                    ("uint16", "uint16"),
                                    ("int32", "int32"),
                                    ("uint32", "uint32"),
                                    ("float", "float"),
                                    ("double", "double"),
                                ],
                            )
                        } else {
                            format!(
                                "<input id='point-{i}-{k}' value='{}' {}>",
                                html(&p[k]),
                                if matches!(*k, "rate" | "offset") {
                                    "type='number' step='any'"
                                } else {
                                    ""
                                }
                            )
                        };
                        format!("<td>{control}</td>")
                    } else {
                        format!("<td>{}</td>", html(&p[k]))
                    }
                })
                .collect::<String>();
            format!(
                "<tr>{cells}{}</tr>",
                if editable {
                    format!("<td>{}</td>", button(&format!("remove-point:{i}"), "移除"))
                } else {
                    String::new()
                }
            )
        })
        .collect::<String>();
    format!(
        "<div class='scroll'><table><thead><tr><th>点位名称</th><th>地址</th><th>类型</th><th>倍率</th><th>偏移</th><th>单位</th>{}</tr></thead><tbody>{rows}</tbody></table></div>",
        if editable { "<th>操作</th>" } else { "" }
    )
}
fn editor(d: &Value, editing: bool) -> String {
    let mut fields = field("dev-name", "设备名称", &d["name"], "text");
    fields += &select(
        "dev-protocol",
        "通信协议",
        &d["protocol"],
        &[
            ("modbus_tcp", "Modbus TCP"),
            ("modbus_rtu", "Modbus RTU"),
            ("s7", "西门子 S7"),
            ("mitsubishi", "三菱 MC"),
        ],
    );
    fields += &field("dev-enabled", "启用设备", &d["enabled"], "checkbox");
    let protocol = string(&d["protocol"]);
    if protocol == "modbus_rtu" {
        for (k, l, t) in [
            ("com_port", "串口号", "text"),
            ("baudrate", "波特率", "number"),
            ("databits", "数据位", "number"),
            ("stopbits", "停止位", "number"),
            ("slave_id", "从站 ID", "number"),
        ] {
            fields += &field(&format!("dev-{k}"), l, &d[k], t);
        }
        fields += &select(
            "dev-parity",
            "校验位",
            &d["parity"],
            &[("N", "无校验"), ("E", "偶校验"), ("O", "奇校验")],
        );
    } else {
        fields += &field("dev-ip", "IP 地址", &d["ip"], "text");
        fields += &field("dev-port", "端口", &d["port"], "number");
        if protocol == "s7" {
            fields += &field("dev-rack", "机架", &d["rack"], "number");
            fields += &field("dev-slot", "插槽", &d["slot"], "number");
        } else if protocol == "mitsubishi" {
            fields += &field("dev-plc_type", "PLC 型号", &d["plc_type"], "text");
        } else {
            fields += &field("dev-slave_id", "从站 ID", &d["slave_id"], "number");
        }
    }
    let points = d["points"].as_array().cloned().unwrap_or_default();
    format!(
        "<div class='overlay'><section class='dialog'><div class='toolbar'><h2>{}</h2>{}</div><div class='form'>{fields}</div><div class='toolbar' style='margin-top:28px'><h3>采集点位</h3>{}</div>{}<div class='form-actions'><button data-action='save-device' class='primary'>保存设备</button>{}{}</div></section></div>",
        if editing {
            "编辑设备"
        } else {
            "添加设备"
        },
        button("cancel", "取消"),
        button("add-point", "添加点位"),
        point_table(&points, true),
        button("test-form", "测试连接"),
        "<span class='muted'>保存后可在系统设置中应用配置。</span>"
    )
}
fn capture_device() -> Result<Value, String> {
    let mut d = UI.with_borrow(|u| u.edit.clone()).ok_or("未打开设备表单")?;
    for k in ["name", "protocol", "ip", "com_port", "parity", "plc_type"] {
        let id = format!("dev-{k}");
        if element(&id).is_some() {
            d[k] = json!(input(&id));
        }
    }
    for k in [
        "port", "slave_id", "baudrate", "databits", "stopbits", "rack", "slot",
    ] {
        let id = format!("dev-{k}");
        if element(&id).is_some() {
            d[k] = number(&id)?;
        }
    }
    d["enabled"] = json!(checked("dev-enabled"));
    if let Some(points) = d["points"].as_array_mut() {
        for (i, p) in points.iter_mut().enumerate() {
            for k in ["name", "address", "type", "unit"] {
                p[k] = json!(input(&format!("point-{i}-{k}")));
            }
            for k in ["rate", "offset"] {
                p[k] = number(&format!("point-{i}-{k}"))?;
            }
        }
    }
    Ok(d)
}
fn update_live() {
    UI.with_borrow(|u| {
        if let Some(e) = element("global-status") {
            e.set_text_content(Some(if u.status["running"] == true {
                "正在采集"
            } else {
                "采集已停止"
            }));
        }
        if let Some(e) = element("collect-toggle") {
            e.set_text_content(Some(if u.status["running"] == true {
                "停止采集"
            } else {
                "启动采集"
            }));
        }
        if let Some(e) = element("mqtt-status") {
            e.set_text_content(Some(if u.status["mqtt_connected"] == true {
                "MQTT 已连接"
            } else {
                "MQTT 未连接"
            }));
            e.set_class_name(if u.status["mqtt_connected"] == true {
                "status good"
            } else {
                "status"
            });
        }
        for (i, d) in u.config["devices"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .enumerate()
        {
            let connected = u.status["devices"].as_array().is_some_and(|ds| {
                ds.iter()
                    .any(|s| s["device_id"] == d["id"] && s["connected"] == true)
            });
            if let Some(e) = element(&format!("device-status-{i}")) {
                e.set_text_content(Some(if connected { "在线" } else { "离线" }));
                e.set_class_name(if connected { "status good" } else { "status" });
            }
            if let Some(data) = u.latest.get(&string(&d["id"])) {
                for (j, p) in d["points"].as_array().unwrap_or(&vec![]).iter().enumerate() {
                    let v = &data["values"][string(&p["name"])];
                    if let Some(e) = element(&format!("value-{i}-{j}")) {
                        let value = if v["value"].is_null() {
                            "—".to_string()
                        } else {
                            string(&v["value"])
                        };
                        e.set_text_content(Some(&value));
                    }
                    if let Some(e) = element(&format!("quality-{i}-{j}")) {
                        e.set_text_content(Some(if v["quality"] == "good" {
                            "正常"
                        } else {
                            "读取失败"
                        }));
                    }
                }
                if let Some(e) = element(&format!("time-{i}")) {
                    let date = js_sys::Date::new(&JsValue::from_f64(
                        data["timestamp"].as_f64().unwrap_or(0.0),
                    ));
                    e.set_text_content(Some(&format!(
                        "更新于 {}",
                        date.to_locale_time_string("zh-CN")
                            .as_string()
                            .unwrap_or_default()
                    )));
                }
            }
        }
    });
}
async fn action(action: String) -> Result<(), String> {
    let parts: Vec<_> = action.split(':').collect();
    let index = parts
        .get(1)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    match parts[0] {
        "nav" => {
            let page = parts.get(1).copied().unwrap_or("home");
            let route = match page {
                "devices" => "/devices",
                "settings" => "/settings",
                _ => "/",
            };
            let location = web_sys::window().unwrap().location();
            if location.hash().unwrap_or_default() == format!("#{route}") {
                UI.with_borrow_mut(|u| {
                    u.page = page.into();
                    u.edit = None;
                    u.preview = None;
                });
                render();
            } else {
                let _ = location.set_hash(route);
            }
        }
        "add" => {
            UI.with_borrow_mut(|u| {
                u.edit = Some(default_device());
                u.editing_id = None;
            });
            render();
        }
        "edit" => {
            UI.with_borrow_mut(|u| {
                let d = u.config["devices"][index].clone();
                u.editing_id = Some(string(&d["id"]));
                u.edit = Some(d);
            });
            render();
        }
        "cancel" => {
            UI.with_borrow_mut(|u| {
                u.edit = None;
                u.preview = None;
                u.editing_id = None;
            });
            render();
        }
        "add-point" | "remove-point" => {
            let mut d = capture_device()?;
            let points = d["points"].as_array_mut().ok_or("点位格式错误")?;
            if parts[0] == "add-point" {
                points.push(
                    json!({"name":"","address":"","type":"float","rate":1,"offset":0,"unit":""}),
                );
            } else if index < points.len() {
                points.remove(index);
            }
            UI.with_borrow_mut(|u| u.edit = Some(d));
            render();
        }
        "protocol" => {
            let mut d = capture_device()?;
            d["port"] = json!(match d["protocol"].as_str() {
                Some("s7") => 102,
                Some("mitsubishi") => 5000,
                _ => 502,
            });
            UI.with_borrow_mut(|u| u.edit = Some(d));
            render();
        }
        "save-device" => {
            let mut d = capture_device()?;
            if string(&d["name"]).trim().is_empty() {
                return Err("请输入设备名称".into());
            }
            if d["points"].as_array().is_some_and(|p| {
                p.iter()
                    .any(|p| string(&p["name"]).is_empty() || string(&p["address"]).is_empty())
            }) {
                return Err("请完整填写点位名称和地址".into());
            }
            let id = UI.with_borrow(|u| u.editing_id.clone());
            if id.is_none() {
                d["id"] = json!(format!(
                    "dev-{}-{}",
                    js_sys::Date::now() as u64,
                    (js_sys::Math::random() * 1e6) as u32
                ));
            }
            let path = id
                .as_ref()
                .map(|id| format!("/api/devices/{}", js_sys::encode_uri_component(id)))
                .unwrap_or("/api/devices".into());
            api(if id.is_some() { "PUT" } else { "POST" }, &path, Some(d)).await?;
            UI.with_borrow_mut(|u| u.edit = None);
            refresh().await?;
            notice("设备已保存", false);
        }
        "delete" | "toggle" => {
            let mut d = UI.with_borrow(|u| u.config["devices"][index].clone());
            let path = format!(
                "/api/devices/{}",
                js_sys::encode_uri_component(&string(&d["id"]))
            );
            if parts[0] == "delete" {
                if !web_sys::window()
                    .unwrap()
                    .confirm_with_message("确定删除这台设备？")
                    .unwrap_or(false)
                {
                    return Ok(());
                }
                api("DELETE", &path, None).await?;
            } else {
                d["enabled"] = json!(d["enabled"] == false);
                api("PUT", &path, Some(d)).await?;
            }
            refresh().await?;
        }
        "test" | "test-form" => {
            let mut d = if parts[0] == "test-form" {
                capture_device()?
            } else {
                UI.with_borrow(|u| u.config["devices"][index].clone())
            };
            if d.get("id").is_none() {
                d["id"] = json!("test")
            };
            let r = api("POST", "/api/devices/test", Some(json!({"device":d}))).await?;
            notice(
                &format!(
                    "{}，耗时 {} ms",
                    string(&r["message"]),
                    string(&r["elapsed_ms"])
                ),
                false,
            );
        }
        "collect" => {
            let running = UI.with_borrow(|u| u.status["running"] == true);
            api(
                "POST",
                if running {
                    "/api/collect/stop"
                } else {
                    "/api/collect/start"
                },
                None,
            )
            .await?;
            refresh().await?;
        }
        "apply" => {
            let r = api("POST", "/api/config/apply", None).await?;
            refresh().await?;
            notice(&string(&r["message"]), false);
        }
        "save-mqtt" => {
            let mut v = json!({"enabled":checked("mqtt-enabled")});
            for k in ["host", "client_id", "topic_prefix", "username", "password"] {
                v[k] = json!(input(&format!("mqtt-{k}")));
            }
            for k in ["port", "qos"] {
                v[k] = number(&format!("mqtt-{k}"))?;
            }
            api("PUT", "/api/mqtt", Some(v)).await?;
            refresh().await?;
            notice("MQTT配置已保存", false);
        }
        "save-ai" => {
            let mut v = json!({"enabled":checked("ai-enabled")});
            for k in ["api_url", "api_key", "model"] {
                v[k] = json!(input(&format!("ai-{k}")));
            }
            api("PUT", "/api/ai/config", Some(v)).await?;
            refresh().await?;
            notice("AI配置已保存", false);
        }
        "save-collect" | "save-reconnect" => {
            let mut changes = json!({});
            if parts[0] == "save-collect" {
                changes["gateway_name"] = json!(input("collect-gateway_name"));
                changes["collect_interval"] = number("collect-collect_interval")?;
            } else {
                changes["reconnect"] = json!({});
                for k in ["max_retries", "base_delay", "max_delay"] {
                    changes["reconnect"][k] = number(&format!("reconnect-{k}"))?;
                }
            }
            let mut cfg = api("GET", "/api/config", None).await?["data"].clone();
            cfg.as_object_mut()
                .ok_or("配置格式错误")?
                .extend(changes.as_object().unwrap().clone());
            api("PUT", "/api/config", Some(cfg)).await?;
            refresh().await?;
            notice("配置已保存", false);
        }
        "save-autostart" => {
            let r = api(
                "POST",
                &format!("/api/system/autostart?enabled={}", checked("autostart")),
                None,
            )
            .await?;
            notice(&string(&r["message"]), false);
        }
        "logs" => {
            let r = api(
                "GET",
                &format!("/api/logs?lines=200&level={}", input("log-level")),
                None,
            )
            .await?;
            if let Some(e) = element("logs") {
                e.set_text_content(Some(
                    &r["data"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .map(string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));
            }
        }
        "clear-logs" => {
            if let Some(e) = element("logs") {
                e.set_text_content(Some(""));
            }
        }
        "set-log-level" => {
            let level = input("log-level");
            let r = api(
                "POST",
                &format!(
                    "/api/logs/level?level={}",
                    if level.is_empty() { "INFO" } else { &level }
                ),
                None,
            )
            .await?;
            notice(&string(&r["message"]), false);
        }
        "import" => {
            let file = element("excel-file")
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
                .and_then(|e| e.files())
                .and_then(|f| f.get(0))
                .ok_or("请选择Excel文件")?;
            let form = web_sys::FormData::new().map_err(js_error)?;
            form.append_with_blob_and_filename("file", &file, &file.name())
                .map_err(js_error)?;
            let r: Value = Request::post("/api/devices/import-excel")
                .body(form)
                .map_err(|e| e.to_string())?
                .send()
                .await
                .map_err(|e| e.to_string())?
                .json()
                .await
                .map_err(|e| e.to_string())?;
            if r["success"] != true {
                return Err(string(&r["message"]));
            }
            UI.with_borrow_mut(|u| u.preview = r["data"]["points"].as_array().cloned());
            render();
        }
        "import-confirm" => {
            UI.with_borrow_mut(|u| {
                let mut d = default_device();
                d["points"] = json!(u.preview.take().unwrap_or_default());
                u.edit = Some(d);
                u.editing_id = None;
            });
            render();
        }
        "ai-generate" => ai_generate().await?,
        "ai-confirm" => {
            let v: Value = serde_json::from_str(&input("ai-json")).map_err(|e| e.to_string())?;
            let mut d = default_device();
            let fields = v.as_object().ok_or("配置必须是JSON对象")?;
            d.as_object_mut().unwrap().extend(fields.clone());
            UI.with_borrow_mut(|u| {
                u.edit = Some(d);
                u.editing_id = None;
            });
            render();
        }
        _ => {}
    }
    Ok(())
}
fn js_error(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}
async fn ai_generate() -> Result<(), String> {
    let text = input("ai-input");
    if text.trim().is_empty() {
        return Err("请先描述需要采集的设备".into());
    }
    if let Some(e) = element("ai-generate") {
        let _ = e.set_attribute("disabled", "");
    }
    if let Some(e) = element("ai-output") {
        e.set_text_content(Some(""));
    }
    let result = async {
        let response = Request::post("/api/ai/parse-stream")
            .json(&json!({"input":text}))
            .map_err(|e| e.to_string())?
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let raw: web_sys::Response = response.into();
        let body = raw.body().ok_or("服务器未返回数据流")?;
        let mut stream = wasm_streams::ReadableStream::from_raw(body).into_stream();
        let mut buffer = vec![];
        let mut content = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(js_error)?;
            buffer.extend(js_sys::Uint8Array::new(&chunk).to_vec());
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line = buffer.drain(..=end).collect::<Vec<_>>();
                let line = String::from_utf8_lossy(&line);
                if let Some(data) = line.trim().strip_prefix("data: ") {
                    let event: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
                    match event["type"].as_str() {
                        Some("chunk") => {
                            content += &string(&event["content"]);
                            if let Some(e) = element("ai-output") {
                                e.set_text_content(Some(&content));
                            }
                        }
                        Some("error") => return Err(string(&event["message"])),
                        Some("done") => {
                            if let Some(e) = element("ai-preview") {
                                let _ = e.remove_attribute("hidden");
                            }
                            if let Some(e) = element("ai-json")
                                .and_then(|e| e.dyn_into::<HtmlTextAreaElement>().ok())
                            {
                                e.set_value(
                                    &serde_json::to_string_pretty(&event["config"])
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    if let Some(e) = element("ai-generate") {
        let _ = e.remove_attribute("disabled");
    }
    result
}
fn connect_ws() {
    let location = web_sys::window().unwrap().location();
    let scheme = if location.protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let Ok(socket) = WebSocket::new(&format!(
        "{scheme}://{}/ws/data",
        location.host().unwrap_or_default()
    )) else {
        return;
    };
    let message = Closure::<dyn FnMut(MessageEvent)>::new(|event: MessageEvent| {
        if let Some(text) = event.data().as_string()
            && let Ok(v) = serde_json::from_str::<Value>(&text)
        {
            UI.with_borrow_mut(|u| {
                if v["type"] == "data" {
                    u.latest.insert(string(&v["device_id"]), v);
                } else if v["type"] == "status"
                    && let Some(devices) = u.status["devices"].as_array_mut()
                    && let Some(d) = devices
                        .iter_mut()
                        .find(|d| d["device_id"] == v["device_id"])
                {
                    d["connected"] = json!(v["status"] == "online");
                }
            });
            update_live();
        }
    });
    let close = Closure::<dyn FnMut(web_sys::Event)>::new(|_| {
        spawn_local(async {
            gloo_timers::future::TimeoutFuture::new(2000).await;
            connect_ws();
        });
    });
    socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
    socket.set_onclose(Some(close.as_ref().unchecked_ref()));
    SOCKET.with_borrow_mut(|s| {
        *s = Some(Session {
            _socket: socket,
            _message: message,
            _close: close,
        })
    });
}
#[wasm_bindgen(start)]
pub fn start() {
    let navigate = || {
        let location = web_sys::window().unwrap().location();
        let hash = location.hash().unwrap_or_default();
        let route = if hash.is_empty() {
            location.pathname().unwrap_or_default()
        } else {
            hash.trim_start_matches('#').to_string()
        };
        UI.with_borrow_mut(|u| {
            u.page = match route.as_str() {
                "/devices" => "devices",
                "/settings" => "settings",
                _ => "home",
            }
            .into();
            u.edit = None;
            u.preview = None;
        });
        render();
    };
    navigate();
    let hashchange = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| navigate());
    web_sys::window()
        .unwrap()
        .add_event_listener_with_callback("hashchange", hashchange.as_ref().unchecked_ref())
        .unwrap();
    hashchange.forget();
    let click = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        let Some(target) = event.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        let Ok(Some(button)) = target.closest("[data-action]") else {
            return;
        };
        let Some(action_name) = button.get_attribute("data-action") else {
            return;
        };
        event.prevent_default();
        if button.has_attribute("disabled") {
            return;
        }
        let _ = button.set_attribute("disabled", "");
        spawn_local(async move {
            if let Err(e) = action(action_name).await {
                notice(&e, true);
            }
            let _ = button.remove_attribute("disabled");
        });
    });
    document()
        .add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
        .unwrap();
    click.forget();
    let change = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        let Some(target) = event.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        let action_name = match target.id().as_str() {
            "excel-file" => "import",
            "dev-protocol" => "protocol",
            "log-level" => "logs",
            _ => return,
        };
        spawn_local(async move {
            if let Err(e) = action(action_name.into()).await {
                notice(&e, true);
            }
        });
    });
    document()
        .add_event_listener_with_callback("change", change.as_ref().unchecked_ref())
        .unwrap();
    change.forget();
    render();
    spawn_local(async {
        if let Err(e) = refresh().await {
            notice(&e, true);
        }
        connect_ws();
        loop {
            gloo_timers::future::TimeoutFuture::new(3000).await;
            if let Ok(v) = api("GET", "/api/collect/status", None).await {
                UI.with_borrow_mut(|u| u.status = v["data"].clone());
                update_live();
            }
        }
    });
}
