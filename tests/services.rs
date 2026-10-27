use rustlink::{ai::normalize_content, logs::LogStore};
use serde_json::json;

#[test]
fn ai_markdown_and_point_defaults_match_original() {
    assert_eq!(
        normalize_content(
            "```json\n{\"name\":\"PLC\",\"points\":[{\"name\":\"T\",\"address\":\"40001\"}]}\n```"
        )
        .unwrap(),
        json!({"name":"PLC","enabled":true,"points":[{"name":"T","address":"40001","type":"float","rate":1.0,"offset":0.0,"unit":""}]})
    );
    assert_eq!(
        normalize_content("{\"enabled\":false}").unwrap(),
        json!({"enabled":false,"points":[]})
    );
    for input in ["not json", "null", "[]", "{\"points\":null}"] {
        assert!(normalize_content(input).is_err());
    }
}
#[test]
fn logs_tail_before_filter_persist_and_rotate() {
    let dir = tempfile::tempdir().unwrap();
    let log = LogStore::open(dir.path(), 200).unwrap();
    log.write("INFO", "first").unwrap();
    log.write("ERROR", "second").unwrap();
    log.write("INFO", "third").unwrap();
    assert!(log.read(Some("ERROR"), 1).unwrap().is_empty());
    assert_eq!(log.read(Some("error"), 2).unwrap().len(), 1);
    log.set_level("ERROR");
    log.write("INFO", "hidden").unwrap();
    assert!(
        !log.read(None, 200)
            .unwrap()
            .iter()
            .any(|l| l.contains("hidden"))
    );
    log.write("ERROR", &"large".repeat(80)).unwrap();
    log.write("ERROR", "after rotation").unwrap();
    assert!(dir.path().join("gateway.log.1").exists());
    drop(log);
    assert!(
        LogStore::open(dir.path(), 200)
            .unwrap()
            .read(None, 200)
            .unwrap()
            .iter()
            .any(|l| l.contains("after rotation"))
    );
}
#[test]
fn mqtt_topics_and_payloads_match_original() {
    let messages = rustlink::engine::mqtt_messages(
        "factory/data",
        json!({"type":"data","device_id":"d","gateway":"车间","device_name":"PLC","timestamp":123,"values":{}}),
    );
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].0, "factory/data/d");
    assert_eq!(messages[1].0, "factory/data/d/status");
    let data: serde_json::Value = serde_json::from_slice(&messages[0].1).unwrap();
    assert!(data.get("type").is_none());
    assert_eq!(data["gateway"], "车间");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&messages[1].1).unwrap(),
        json!({"device_id":"d","timestamp":123,"status":"online"})
    );
}
