use rustlink::{model::default_config, store::ConfigStore};
use serde_json::json;

#[test]
fn initializes_backs_up_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let s = ConfigStore::open(&path).unwrap();
    assert_eq!(s.snapshot(), default_config());
    s.update(|v| {
        v["gateway_name"] = json!("车间一");
        Ok(())
    })
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(path.with_extension("json.bak")).unwrap()
        )
        .unwrap(),
        default_config()
    );
    drop(s);
    assert_eq!(
        ConfigStore::open(&path).unwrap().snapshot()["gateway_name"],
        "车间一"
    );
}
#[test]
fn concurrent_writers_do_not_lose_updates() {
    let dir = tempfile::tempdir().unwrap();
    let s = std::sync::Arc::new(ConfigStore::open(&dir.path().join("config.json")).unwrap());
    std::thread::scope(|scope| {
        for i in 0..24 {
            let s = s.clone();
            scope.spawn(move || {
                s.update(|v| {
                    v["devices"].as_array_mut().unwrap().push(json!({"id":i}));
                    Ok(())
                })
                .unwrap()
            });
        }
    });
    assert_eq!(s.snapshot()["devices"].as_array().unwrap().len(), 24);
}
#[test]
fn failed_transaction_leaves_memory_and_disk_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let s = ConfigStore::open(&path).unwrap();
    let before = s.snapshot();
    assert!(
        s.update(|v| -> anyhow::Result<()> {
            v["gateway_name"] = json!("invalid");
            anyhow::bail!("injected failure")
        })
        .is_err()
    );
    assert_eq!(s.snapshot(), before);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap(),
        before
    );
}

#[test]
fn actual_disk_failure_does_not_publish_uncommitted_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let s = ConfigStore::open(&path).unwrap();
    let before = s.snapshot();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        s.update(|v| {
            v["gateway_name"] = json!("must-not-publish");
            Ok(())
        })
        .is_err()
    );
    assert_eq!(s.snapshot(), before);
}
#[test]
fn damaged_primary_recovers_backup_and_preserves_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let s = ConfigStore::open(&path).unwrap();
    s.update(|v| {
        v["gateway_name"] = json!("new");
        Ok(())
    })
    .unwrap();
    drop(s);
    std::fs::write(&path, "{broken").unwrap();
    let s = ConfigStore::open(&path).unwrap();
    assert_eq!(s.snapshot(), default_config());
    assert!(
        dir.path().read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("corrupt"))
    );
}
#[test]
fn another_process_cannot_own_the_same_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let first = ConfigStore::open(&path).unwrap();
    assert!(ConfigStore::open(&path).is_err());
    drop(first);
    assert!(ConfigStore::open(&path).is_ok());
}
#[test]
fn reload_keeps_unknown_legacy_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let s = ConfigStore::open(&path).unwrap();
    let mut v = s.snapshot();
    v["custom"] = json!({"nested":[1,2]});
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
    s.reload().unwrap();
    assert_eq!(s.snapshot(), v);
}
