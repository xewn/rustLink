# 来源与许可

RustLink 是 FactoryLink 的 Rust 重写版本。行为基线为本地 FactoryLink 仓库提交 `dcfeb2c4bda1c8e92151892bf6363b84df2f9628`，上游为 https://github.com/qinshihu/FactoryLink。

本项目使用 MPL-2.0，完整许可见 LICENSE。原版 AI 系统提示词、API schema 和测试基准来自上游；相关作者权利与许可继续保留。Rust 后端、Rust/WebAssembly 界面、协议实现与测试在本目录中维护。

运行程序不依赖 Python、Node.js、Vue 或原版目录。Python 脚本仅用于生成行为基准、自动化验收和性能对照；WebAssembly 的少量 JavaScript 是 wasm-bindgen 自动生成的浏览器接口。
