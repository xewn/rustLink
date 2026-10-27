use crate::model::Ai;
use anyhow::{Context, Result};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

pub const SYSTEM_PROMPT: &str = include_str!("ai_prompt.txt");
pub const MISSING_KEY: &str = "请先在系统设置中配置AI API Key";
const LIMIT: usize = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseRequest {
    pub input: String,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}
impl ParseRequest {
    pub fn settings(&self, mut saved: Ai) -> Ai {
        if let Some(v) = self.api_url.as_ref().filter(|s| !s.is_empty()) {
            saved.api_url = v.clone();
        }
        if let Some(v) = self.api_key.as_ref().filter(|s| !s.is_empty()) {
            saved.api_key = v.clone();
        }
        if let Some(v) = self.model.as_ref().filter(|s| !s.is_empty()) {
            saved.model = v.clone();
        }
        saved
    }
}
pub fn normalize_content(content: &str) -> Result<Value> {
    let mut clean = content.trim().to_string();
    if clean.starts_with("```") {
        let mut lines: Vec<_> = clean.lines().skip(1).collect();
        if lines.last().is_some_and(|s| s.trim() == "```") {
            lines.pop();
        }
        clean = lines.join("\n");
    }
    let mut config: Value = serde_json::from_str(&clean)?;
    let obj = config.as_object_mut().context("config must be an object")?;
    obj.entry("enabled").or_insert(json!(true));
    obj.entry("points").or_insert(json!([]));
    for point in obj
        .get_mut("points")
        .unwrap()
        .as_array_mut()
        .context("points must be array")?
    {
        let p = point.as_object_mut().context("point must be object")?;
        for (k, v) in [
            ("type", json!("float")),
            ("rate", json!(1.0)),
            ("offset", json!(0.0)),
            ("unit", json!("")),
        ] {
            p.entry(k).or_insert(v);
        }
    }
    Ok(config)
}
#[derive(Clone)]
pub struct AiService {
    client: reqwest::Client,
}
impl AiService {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()?,
        })
    }
    fn request(&self, req: &ParseRequest, cfg: &Ai, stream: bool) -> reqwest::RequestBuilder {
        let mut payload = json!({"model":cfg.model,"messages":[{"role":"system","content":SYSTEM_PROMPT},{"role":"user","content":req.input}],"temperature":0.1,"max_tokens":2000});
        if stream {
            payload["stream"] = json!(true);
        }
        self.client
            .post(format!("{}/chat/completions", cfg.api_url))
            .bearer_auth(&cfg.api_key)
            .json(&payload)
    }
    pub async fn parse(&self, req: ParseRequest, cfg: Ai) -> Result<Value, String> {
        if cfg.api_key.is_empty() {
            return Err(MISSING_KEY.into());
        }
        let response = self
            .request(&req, &cfg, false)
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(status_error(response.status().as_u16(), false));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = vec![];
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(request_error)?;
            if bytes.len() + chunk.len() > LIMIT {
                return Err("AI返回内容过大".into());
            }
            bytes.extend(chunk);
        }
        let data: Value =
            serde_json::from_slice(&bytes).map_err(|_| "AI返回格式异常，请重试".to_string())?;
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("AI返回格式异常，请重试")?;
        normalize_content(content).map_err(|_| "AI返回格式异常，请重试".into())
    }
    pub fn stream(&self, req: ParseRequest, cfg: Ai) -> impl Stream<Item = Value> + Send + 'static {
        let service = self.clone();
        async_stream::stream! {
            if cfg.api_key.is_empty(){yield json!({"type":"error","message":MISSING_KEY});return}
            let response=match service.request(&req,&cfg,true).send().await{Ok(r)=>r,Err(e)=>{yield json!({"type":"error","message":request_error(e)});return}};
            if !response.status().is_success(){yield json!({"type":"error","message":status_error(response.status().as_u16(),true)});return}
            let mut incoming=response.bytes_stream();let mut buffer=vec![];let mut content=String::new();let mut finished=false;
            while let Some(chunk)=incoming.next().await {
                let chunk=match chunk{Ok(c)=>c,Err(e)=>{yield json!({"type":"error","message":request_error(e)});return}};
                buffer.extend(chunk);if buffer.len()+content.len()>LIMIT{yield json!({"type":"error","message":"AI返回内容过大"});return}
                while let Some(pos)=buffer.iter().position(|b|*b==b'\n'){
                    let bytes:Vec<u8>=buffer.drain(..=pos).collect();let line=String::from_utf8_lossy(&bytes);let line=line.trim_end_matches(['\r','\n']);
                    if let Some(data)=line.strip_prefix("data: "){
                        if data.trim()=="[DONE]"{finished=true;break}
                        if let Ok(value)=serde_json::from_str::<Value>(data)&& let Some(delta)=value["choices"][0]["delta"]["content"].as_str().filter(|s|!s.is_empty()){
                            content.push_str(delta);yield json!({"type":"chunk","content":delta});
                        }
                    }
                }
                if finished{break}
            }
            // aiter_lines also delivers a final line without a newline.
            if !finished && !buffer.is_empty()&& let Ok(line)=std::str::from_utf8(&buffer)&& let Some(data)=line.strip_prefix("data: ")&& let Ok(value)=serde_json::from_str::<Value>(data)&& let Some(delta)=value["choices"][0]["delta"]["content"].as_str(){content.push_str(delta);yield json!({"type":"chunk","content":delta});}
            match normalize_content(&content){Ok(config)=>yield json!({"type":"done","config":config}),Err(_)=>yield json!({"type":"error","message":"AI返回格式异常，请重试"})}
        }
    }
}
fn status_error(status: u16, stream: bool) -> String {
    match status {
        401 => "API Key无效，请检查AI设置".into(),
        404 if !stream => "API地址无效，请检查AI设置中的API地址".into(),
        _ => format!("AI API请求失败: HTTP {status}"),
    }
}
fn request_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "AI API请求超时，请检查网络或API地址".into()
    } else {
        "AI解析失败: 网络请求失败".into()
    }
}
