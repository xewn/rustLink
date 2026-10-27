use rustlink::{codec, model};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/oracle.json")).unwrap()
}
fn equivalent(actual: Value, expected: &Value, case: &Value) {
    if let (Some(a), Some(b)) = (actual.as_f64(), expected.as_f64()) {
        assert!(
            (a - b).abs() <= 1e-12_f64.max(b.abs() * 1e-14),
            "case={case}, actual={a}, expected={b}"
        );
    } else {
        assert_eq!(&actual, expected, "case={case}");
    }
}

#[test]
fn original_modbus_address_boundaries() {
    for c in fixture()["modbus_addresses"].as_array().unwrap() {
        let got = codec::modbus_address(c["input"].as_str().unwrap())
            .map(|(f, a)| json!([f, a, 1]))
            .unwrap_or(Value::Null);
        assert_eq!(got, c["expected"], "{c}");
    }
}
#[test]
fn original_s7_address_grammar() {
    for c in fixture()["s7_addresses"].as_array().unwrap() {
        let got = codec::s7_address(c["input"].as_str().unwrap())
            .map(|a| json!([a.area, a.db, a.start, a.bit, a.hint]))
            .unwrap_or(Value::Null);
        assert_eq!(got, c["expected"], "{c}");
    }
}
#[test]
fn original_numeric_decoders_seeded_corpus() {
    for c in fixture()["decode"].as_array().unwrap() {
        let data: Vec<u8> = serde_json::from_value(c["bytes"].clone()).unwrap();
        let got = codec::decode(
            &data,
            c["type"].as_str().unwrap(),
            c["protocol"].as_str().unwrap(),
            0,
        )
        .unwrap_or(Value::Null);
        equivalent(got, &c["expected"], c);
    }
}
#[test]
fn original_coil_decoding() {
    for c in fixture()["bits"].as_array().unwrap() {
        let bits: Vec<bool> = serde_json::from_value(c["bits"].clone()).unwrap();
        equivalent(
            codec::decode_bits(&bits, c["type"].as_str().unwrap()).unwrap_or(Value::Null),
            &c["expected"],
            c,
        );
    }
}
#[test]
fn original_mitsubishi_signed_words() {
    for c in fixture()["mc"].as_array().unwrap() {
        let words: Vec<i16> = serde_json::from_value(c["words"].clone()).unwrap();
        let got = codec::decode_mc(&words, c["type"].as_str().unwrap())
            .map(|v| {
                codec::scale(
                    v,
                    c["rate"].as_f64().unwrap(),
                    c["offset"].as_f64().unwrap(),
                )
            })
            .unwrap_or(Value::Null);
        equivalent(got, &c["expected"], c);
    }
}
#[test]
fn original_scaling_including_booleans() {
    for c in fixture()["scaling"].as_array().unwrap() {
        equivalent(
            codec::scale(
                c["raw"].clone(),
                c["rate"].as_f64().unwrap(),
                c["offset"].as_f64().unwrap(),
            ),
            &c["expected"],
            c,
        );
    }
}
#[test]
fn original_model_defaults_and_nullable_fields() {
    for c in fixture()["models"].as_array().unwrap() {
        let v = c["input"].clone();
        let got = match c["model"].as_str().unwrap() {
            "PointConfig" => {
                serde_json::to_value(serde_json::from_value::<model::Point>(v).unwrap())
            }
            "DeviceConfig" => {
                serde_json::to_value(serde_json::from_value::<model::Device>(v).unwrap())
            }
            "GatewayConfig" => {
                serde_json::to_value(serde_json::from_value::<model::Gateway>(v).unwrap())
            }
            "MqttConfig" => serde_json::to_value(serde_json::from_value::<model::Mqtt>(v).unwrap()),
            "AIConfig" => serde_json::to_value(serde_json::from_value::<model::Ai>(v).unwrap()),
            _ => unreachable!(),
        }
        .unwrap();
        assert_eq!(got, c["expected"], "{c}");
    }
}
#[test]
fn mandatory_fields_are_not_silently_defaulted() {
    assert!(serde_json::from_value::<model::Device>(json!({"name":"x"})).is_err());
    assert!(serde_json::from_value::<model::Point>(json!({"name":"x"})).is_err());
}
