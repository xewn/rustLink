use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use rustlink::app::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn original_api_transaction_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path()).unwrap();
    let app = router(state.clone());
    let cases: Value = serde_json::from_str(include_str!("fixtures/api_oracle.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let request = Request::builder()
            .method(c["method"].as_str().unwrap())
            .uri(c["path"].as_str().unwrap())
            .header("content-type", "application/json")
            .body(if c["body"].is_null() {
                Body::empty()
            } else {
                Body::from(c["body"].to_string())
            })
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            c["status"].as_u64().unwrap() as u16,
            "{c}"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value, c["expected"], "{} {}", c["method"], c["path"]);
    }
    state.engine.stop().await;
}
#[tokio::test]
async fn original_excel_import_responses() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(dir.path()).unwrap());
    let cases: Value = serde_json::from_str(include_str!("fixtures/excel_oracle.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let filename = c["file"].as_str().unwrap();
        let file = std::fs::read(format!("tests/fixtures/{filename}")).unwrap();
        let mut body=format!("--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n").into_bytes();
        body.extend(file);
        body.extend(b"\r\n--boundary--\r\n");
        let request = Request::post("/api/devices/import-excel")
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            c["expected"],
            "{filename}"
        );
    }
}
#[tokio::test]
async fn missing_fields_have_422_and_never_mutate_config() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path()).unwrap();
    let before = state.store.snapshot();
    let app = router(state.clone());
    for path in ["/api/devices", "/api/devices/test", "/api/ai/parse"] {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 422, "{path}");
    }
    assert_eq!(state.store.snapshot(), before);
}
#[tokio::test]
async fn missing_ai_key_stream_is_sse_error() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(dir.path()).unwrap());
    let response = app
        .oneshot(
            Request::post("/api/ai/parse-stream")
                .header("content-type", "application/json")
                .body(Body::from(json!({"input":"PLC"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("text/event-stream")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let s = std::str::from_utf8(&body).unwrap();
    let value: Value = serde_json::from_str(s.trim().strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(
        value,
        json!({"type":"error","message":"请先在系统设置中配置AI API Key"})
    );
}

#[tokio::test]
async fn missing_device_operations_preserve_last_valid_backup() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path()).unwrap();
    state
        .store
        .update(|v| {
            v["gateway_name"] = json!("changed");
            Ok(())
        })
        .unwrap();
    let backup = dir.path().join("config.json.bak");
    let before = std::fs::read(&backup).unwrap();
    let app = router(state);
    let response = app
        .oneshot(
            Request::delete("/api/devices/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(std::fs::read(backup).unwrap(), before);
}
