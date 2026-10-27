use crate::{
    model::{Device, Gateway, Mqtt, Reconnect, timestamp},
    protocol::Client,
};
use anyhow::{Result, ensure};
use indexmap::IndexMap;
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock, broadcast, mpsc, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const EVENT_CAPACITY: usize = 256;
const MQTT_CAPACITY: usize = 512;
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}
struct Inner {
    lifecycle: Mutex<Option<Run>>,
    running: AtomicBool,
    devices: RwLock<IndexMap<String, (String, bool)>>,
    events: broadcast::Sender<Value>,
    mqtt_connected: AtomicBool,
    dropped: AtomicU64,
    samples: AtomicU64,
    settings: watch::Sender<(String, Duration)>,
}
struct Run {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}
impl Engine {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                lifecycle: Mutex::new(None),
                running: AtomicBool::new(false),
                devices: RwLock::new(IndexMap::new()),
                events,
                mqtt_connected: AtomicBool::new(false),
                dropped: AtomicU64::new(0),
                samples: AtomicU64::new(0),
                settings: watch::channel(("默认网关".into(), Duration::from_secs(1))).0,
            }),
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.inner.events.subscribe()
    }
    pub async fn start(&self, cfg: Gateway) -> Result<()> {
        let mut run = self.inner.lifecycle.lock().await;
        self.start_locked(cfg, &mut run).await
    }
    async fn start_locked(&self, cfg: Gateway, run: &mut Option<Run>) -> Result<()> {
        if run.is_some() {
            return Ok(());
        }
        ensure!(
            cfg.collect_interval > 0,
            "collect_interval must be positive"
        );
        ensure!(cfg.mqtt.qos <= 2, "MQTT QoS must be 0, 1 or 2");
        self.update_settings(cfg.gateway_name.clone(), cfg.collect_interval);
        let cancel = CancellationToken::new();
        let mut tasks = vec![];
        let (tx, rx) = mpsc::channel(MQTT_CAPACITY);
        if cfg.mqtt.enabled && !cfg.mqtt.host.is_empty() {
            let e = self.clone();
            let c = cancel.clone();
            let mqtt = cfg.mqtt.clone();
            tasks.push(tokio::spawn(async move { e.mqtt_loop(mqtt, rx, c).await }));
        } else {
            drop(rx)
        }
        // Original collector map uses the last configuration for a duplicate ID.
        let mut unique = IndexMap::new();
        for d in cfg.devices {
            if d.enabled
                && matches!(
                    d.protocol.as_str(),
                    "modbus_tcp" | "modbus_rtu" | "s7" | "mitsubishi"
                )
            {
                unique.insert(d.id.clone(), d);
            }
        }
        {
            let mut devices = self.inner.devices.write().await;
            for d in unique.values() {
                devices.insert(d.id.clone(), (d.name.clone(), false));
            }
        }
        for d in unique.into_values() {
            let e = self.clone();
            let c = cancel.clone();
            let reconnect = cfg.reconnect.clone();
            let t = tx.clone();
            tasks.push(tokio::spawn(async move {
                e.device_loop(d, reconnect, t, c).await
            }));
        }
        self.inner.running.store(true, Ordering::Release);
        *run = Some(Run { cancel, tasks });
        Ok(())
    }
    pub async fn stop(&self) {
        let mut guard = self.inner.lifecycle.lock().await;
        self.stop_locked(&mut guard).await;
    }
    async fn stop_locked(&self, guard: &mut Option<Run>) {
        if let Some(run) = guard.take() {
            run.cancel.cancel();
            for task in run.tasks {
                let _ = task.await;
            }
        }
        self.inner.devices.write().await.clear();
        self.inner.mqtt_connected.store(false, Ordering::Release);
        self.inner.running.store(false, Ordering::Release);
    }
    pub async fn restart(&self, cfg: Gateway) -> Result<()> {
        ensure!(
            cfg.collect_interval > 0 && cfg.mqtt.qos <= 2,
            "invalid interval or MQTT QoS"
        );
        let mut guard = self.inner.lifecycle.lock().await;
        self.stop_locked(&mut guard).await;
        self.start_locked(cfg, &mut guard).await
    }
    pub fn update_settings(&self, name: String, interval_ms: u64) {
        self.inner
            .settings
            .send_replace((name, Duration::from_millis(interval_ms.max(1))));
    }
    pub async fn status(&self) -> Value {
        let devices=self.inner.devices.read().await.iter().map(|(id,(name,connected))|json!({"device_id":id,"device_name":name,"connected":connected})).collect::<Vec<_>>();
        json!({"running":self.inner.running.load(Ordering::Acquire),"devices":devices,"mqtt_connected":self.inner.mqtt_connected.load(Ordering::Acquire)})
    }
    pub fn metrics(&self) -> Value {
        json!({"samples":self.inner.samples.load(Ordering::Relaxed),"mqtt_dropped":self.inner.dropped.load(Ordering::Relaxed),"event_capacity":EVENT_CAPACITY,"mqtt_capacity":MQTT_CAPACITY})
    }
    fn publish(&self, event: Value, mqtt: &mpsc::Sender<Value>) {
        let _ = self.inner.events.send(event.clone());
        if let Err(mpsc::error::TrySendError::Full(_)) = mqtt.try_send(event) {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    async fn status_event(
        &self,
        d: &Device,
        status: &str,
        message: String,
        mqtt: &mpsc::Sender<Value>,
    ) {
        if let Some(entry) = self.inner.devices.write().await.get_mut(&d.id) {
            entry.1 = status == "online";
        }
        self.publish(json!({"type":"status","device_id":d.id,"status":status,"message":message,"timestamp":timestamp()}),mqtt);
    }
    async fn device_loop(
        &self,
        d: Device,
        reconnect: Reconnect,
        mqtt: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) {
        let mut attempt = 0u64;
        let mut ever_connected = false;
        let mut settings = self.inner.settings.subscribe();
        'connect: loop {
            let connected = tokio::select! {biased;_=cancel.cancelled()=>break,result=Client::connect(&d)=>result};
            let mut client = match connected {
                Ok(c) => {
                    attempt = 0;
                    self.status_event(
                        &d,
                        "online",
                        if ever_connected {
                            "重连成功"
                        } else {
                            "连接成功"
                        }
                        .into(),
                        &mqtt,
                    )
                    .await;
                    ever_connected = true;
                    c
                }
                Err(_) => {
                    if reconnect.max_retries > 0 && attempt >= reconnect.max_retries {
                        self.status_event(&d, "error", "连接失败，已达最大重试次数".into(), &mqtt)
                            .await;
                        break;
                    }
                    let delay = reconnect.delay(attempt);
                    self.status_event(
                        &d,
                        "offline",
                        format!("连接失败，{}秒后重试", delay.as_secs()),
                        &mqtt,
                    )
                    .await;
                    // A configured zero delay must still yield, to avoid a retry busy loop.
                    tokio::select! {biased;_=cancel.cancelled()=>break,_=tokio::time::sleep(delay.max(Duration::from_millis(10)))=>{}}
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            };
            let mut interval = settings.borrow_and_update().1;
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {biased;
                    _=cancel.cancelled()=>break 'connect,
                    changed=settings.changed()=>{
                        if changed.is_err(){break 'connect}
                        interval=settings.borrow_and_update().1;
                        ticker=tokio::time::interval(interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        continue;
                    },
                    _=ticker.tick()=>{}
                }
                let gateway = settings.borrow().0.clone();
                let time = timestamp();
                let mut values = serde_json::Map::new();
                let mut good = false;
                for p in &d.points {
                    let result = tokio::select! {biased;_=cancel.cancelled()=>break 'connect,r=client.read(p)=>r};
                    let value = match result {
                        Ok(v) => {
                            good = true;
                            json!({"value":v,"unit":p.unit,"quality":"good"})
                        }
                        Err(_) => json!({"value":null,"unit":p.unit,"quality":"bad"}),
                    };
                    values.insert(p.name.clone(), value);
                }
                self.inner.samples.fetch_add(1, Ordering::Relaxed);
                self.publish(json!({"type":"data","gateway":gateway,"device_id":d.id,"device_name":d.name,"timestamp":time,"values":values}),&mqtt);
                if !good && !d.points.is_empty() {
                    self.status_event(&d, "offline", "所有点位读取失败".into(), &mqtt)
                        .await;
                    // A PLC accepting connections but rejecting reads must not spin.
                    tokio::select! {biased;_=cancel.cancelled()=>break 'connect,_=tokio::time::sleep(interval.min(Duration::from_secs(1)))=>{}}
                    continue 'connect;
                }
            }
        }
        if cancel.is_cancelled() {
            self.status_event(&d, "offline", "已断开".into(), &mqtt)
                .await;
        }
    }
    async fn mqtt_loop(&self, cfg: Mqtt, mut rx: mpsc::Receiver<Value>, cancel: CancellationToken) {
        let mut options = MqttOptions::new(&cfg.client_id, &cfg.host, cfg.port);
        options.set_keep_alive(Duration::from_secs(60));
        if !cfg.username.is_empty() {
            options.set_credentials(&cfg.username, &cfg.password);
        }
        let (client, mut eventloop) = AsyncClient::new(options, MQTT_CAPACITY);
        let qos = match cfg.qos {
            0 => QoS::AtMostOnce,
            2 => QoS::ExactlyOnce,
            _ => QoS::AtLeastOnce,
        };
        let mut next_poll = tokio::time::Instant::now();
        'connection: loop {
            // poll() may be in a connection handshake. Preserve its future when a
            // producer wakes us; dropping it can close a partially opened socket.
            let poll_at = next_poll;
            let polling = async {
                tokio::time::sleep_until(poll_at).await;
                eventloop.poll().await
            };
            tokio::pin!(polling);
            let result = loop {
                tokio::select! {biased;
                    _=cancel.cancelled()=>break 'connection,
                    result=&mut polling=>break result,
                    Some(event)=rx.recv()=>{
                        for(topic,payload) in mqtt_messages(&cfg.topic_prefix,event){
                            // The same task must keep polling; never await publish here.
                            if client.try_publish(topic,qos,false,payload).is_err(){self.inner.dropped.fetch_add(1,Ordering::Relaxed);}
                        }
                    }
                }
            };
            match result {
                Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                    self.inner.mqtt_connected.store(true, Ordering::Release);
                }
                Ok(_) => {}
                Err(_) => {
                    self.inner.mqtt_connected.store(false, Ordering::Release);
                    next_poll = tokio::time::Instant::now() + Duration::from_secs(1);
                }
            }
        }
        self.inner.mqtt_connected.store(false, Ordering::Release);
    }
}
pub fn mqtt_messages(prefix: &str, mut event: Value) -> Vec<(String, Vec<u8>)> {
    let kind = event
        .as_object_mut()
        .and_then(|o| o.remove("type"))
        .unwrap_or(Value::Null);
    let id = event["device_id"].as_str().unwrap_or("unknown").to_owned();
    let topic = format!("{prefix}/{id}");
    if kind == "data" {
        let status = json!({"device_id":id,"status":"online","timestamp":event["timestamp"]});
        vec![
            (
                topic.clone(),
                serde_json::to_vec(&event).unwrap_or_default(),
            ),
            (
                format!("{topic}/status"),
                serde_json::to_vec(&status).unwrap_or_default(),
            ),
        ]
    } else {
        vec![(
            format!("{topic}/status"),
            serde_json::to_vec(&event).unwrap_or_default(),
        )]
    }
}
