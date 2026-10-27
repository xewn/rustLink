#[cfg(target_arch = "wasm32")]
mod browser;

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn default_device() -> serde_json::Value {
    serde_json::json!({"id":"","name":"","protocol":"modbus_tcp","enabled":true,"ip":"","port":502,"slave_id":1,"com_port":"COM1","baudrate":9600,"parity":"N","databits":8,"stopbits":1,"rack":0,"slot":1,"plc_type":"FX5U","points":[]})
}

#[cfg(test)]
mod tests {
    #[test]
    fn escape_user_device_names() {
        assert_eq!(
            super::escape("<script>\"&'"),
            "&lt;script&gt;&quot;&amp;&#39;"
        );
    }
    #[test]
    fn device_form_defaults() {
        let v = super::default_device();
        assert_eq!(v["port"], 502);
        assert_eq!(v["enabled"], true);
        assert!(v["points"].as_array().unwrap().is_empty());
    }
}
