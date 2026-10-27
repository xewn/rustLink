use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
fn float() -> String {
    "float".into()
}
fn parity() -> Option<String> {
    Some("N".into())
}
fn bits() -> Option<i64> {
    Some(8)
}
fn some_one() -> Option<i64> {
    Some(1)
}
fn zero() -> Option<i64> {
    Some(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub name: String,
    pub address: String,
    #[serde(rename = "type", default = "float")]
    pub kind: String,
    #[serde(default = "one")]
    pub rate: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Port {
    Number(i64),
    Text(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub protocol: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub port: Option<Port>,
    #[serde(default)]
    pub com_port: Option<String>,
    #[serde(default)]
    pub baudrate: Option<i64>,
    #[serde(default = "parity")]
    pub parity: Option<String>,
    #[serde(default = "bits")]
    pub databits: Option<i64>,
    #[serde(default = "some_one")]
    pub stopbits: Option<i64>,
    #[serde(default = "some_one")]
    pub slave_id: Option<i64>,
    #[serde(default = "zero")]
    pub rack: Option<i64>,
    #[serde(default = "some_one")]
    pub slot: Option<i64>,
    #[serde(default)]
    pub plc_type: Option<String>,
    #[serde(default)]
    pub points: Vec<Point>,
}
impl Device {
    pub fn tcp_port(&self, default: u16) -> anyhow::Result<u16> {
        Ok(match &self.port {
            None => default,
            Some(Port::Number(n)) => (*n).try_into()?,
            Some(Port::Text(s)) => s.parse()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Mqtt {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub topic_prefix: String,
    pub username: String,
    pub password: String,
    pub qos: u8,
    pub enabled: bool,
}
impl Default for Mqtt {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 1883,
            client_id: "gateway-001".into(),
            topic_prefix: "factory/data".into(),
            username: String::new(),
            password: String::new(),
            qos: 1,
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Reconnect {
    pub max_retries: u64,
    pub base_delay: u64,
    pub max_delay: u64,
}
impl Default for Reconnect {
    fn default() -> Self {
        Self {
            max_retries: 0,
            base_delay: 1,
            max_delay: 60,
        }
    }
}
impl Reconnect {
    pub fn delay(&self, retry: u64) -> std::time::Duration {
        let seconds = self
            .base_delay
            .saturating_mul(1u64.checked_shl(retry.min(64) as u32).unwrap_or(u64::MAX))
            .min(self.max_delay);
        std::time::Duration::from_secs(seconds)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Gateway {
    pub gateway_name: String,
    pub devices: Vec<Device>,
    pub mqtt: Mqtt,
    pub collect_interval: u64,
    pub reconnect: Reconnect,
}
impl Default for Gateway {
    fn default() -> Self {
        Self {
            gateway_name: "默认网关".into(),
            devices: vec![],
            mqtt: Mqtt::default(),
            collect_interval: 1000,
            reconnect: Reconnect::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Ai {
    pub enabled: bool,
    pub api_url: String,
    pub api_key: String,
    pub model: String,
}
impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-3.5-turbo".into(),
        }
    }
}

pub fn default_config() -> Value {
    let mut v = serde_json::to_value(Gateway::default()).expect("default config");
    v["ai"] = serde_json::to_value(Ai::default()).expect("default ai");
    v
}
pub fn response(success: bool, message: impl Into<String>, data: Value) -> Value {
    json!({"success":success,"message":message.into(),"data":data})
}
pub fn timestamp() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
