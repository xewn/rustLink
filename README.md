# RustLink

FactoryLink 的 Rust 重写：异步 PLC 采集、MQTT 转发、HTTP/WebSocket 服务与 Rust/WebAssembly 配置界面。保留原版配置字段、接口路径、数据主题、倍率/偏移及点位质量语义。

## 直接运行

源码构建后的 Windows 独立程序位于 `target/release/rustlink.exe`；发行包为 `dist/RustLink-windows-x64.zip`，解压后直接运行根目录 `rustlink.exe`。双击会打开网页并创建托盘图标。默认监听 `127.0.0.1:8000`；端口占用时依次尝试后续端口。网页与 WebAssembly 已嵌入程序，运行时不需要前端文件夹、Python 或 Node.js。

发行包同时包含 `simulator.exe`、`config.simulated.json`、`demo.ps1`、验收文档、测试证据以及对应源码 `source.zip`。在解压目录运行 `powershell -ExecutionPolicy Bypass -File .\demo.ps1` 可启动隔离演示，关闭网关后模拟器随之退出。发行包中的证据目录为 `evidence/`，报告中的 `artifacts/` 指源码工作区路径。

程序默认在可执行文件所在目录保存 `config.json`、`config.json.bak` 和 `logs/`。迁移时将原版 `config.json` 复制到 RustLink 的配置目录；不要让原版与 RustLink 同时管理同一配置文件。

```powershell
.\target\release\rustlink.exe --headless --port 8000 --strict-port --data-dir D:\RustLinkData
```

`--auto-start` 在程序启动时开始采集；`--host 0.0.0.0` 用于受控网络或容器入口。默认仍只监听本机，与原版一致。`--help` 列出全部参数。API 文档为 `/docs`，机器可读 schema 为 `/openapi.json`。

## 离线模拟

```powershell
cargo build --release --examples
.\target\release\examples\simulator.exe
```

模拟器只监听本机：Modbus TCP `1502`、S7 `1102`、MC `15000`、MQTT `18830`、AI `18080`。固定模拟温度 `25.5℃`，压力 `1.2MPa`，布尔点位按原版缩放为数值。

另开终端，准备独立演示目录并运行网关：

```powershell
New-Item -ItemType Directory -Force demo-data
Copy-Item examples/config.simulated.json demo-data/config.json
.\target\release\rustlink.exe --data-dir .\demo-data --auto-start
```

演示界面可查看三类 PLC 数据、MQTT 状态、Excel 导入与 AI 流式配置，无需外部服务或付费 AI 密钥。RTU 的模拟帧测试不需要串口；Linux 另有真实伪终端测试。物理 RS-485/PLC 仍需现场验证。

## 构建与测试

使用 Rust 1.94.0 或兼容版本。前端同样使用 Rust，构建目标为 `wasm32-unknown-unknown`，绑定工具固定为 `wasm-bindgen-cli 0.2.129`。

```powershell
.\build.ps1
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Linux 安装 `libudev-dev`、`pkg-config` 与 C/C++ 构建工具后运行 `sh build.sh`。CI 配置覆盖 Windows 和 Ubuntu。Dockerfile 提供多阶段构建，运行镜像使用普通用户与 `/data` 持久卷。

浏览器及原版差分验收另使用 Python 测试环境：

```powershell
python -m venv .venv
.\.venv\Scripts\python -m pip install -r tests/requirements.txt
.\.venv\Scripts\python -m playwright install chromium
.\.venv\Scripts\python tests/ui_e2e.py
.\.venv\Scripts\python tests/system_differential.py D:\PLCLink\FactoryLink
.\.venv\Scripts\python tools/benchmark.py --duration 30
```

固定基准位于 `tests/fixtures/`。普通 Rust 测试直接读取样本，无需原版仓库或 Python。需要更新基准时先审查原版变化，再运行 `tools/generate_oracle.py` 和 `tools/generate_api_oracle.py`；不应为了让测试通过而修改期望值。

## 可靠性与高可用

- 每台设备独立异步任务；连接/读取有 3 秒期限。单台设备故障不会串行阻塞其他设备。
- 重试采用有上限的指数退避，支持最大次数；停止会取消连接、读取和退避。启停与热加载操作串行化。
- WebSocket 广播环形缓冲为 256 条；慢订阅者跳过旧消息。MQTT 应用队列及客户端队列分别为 512 条；溢出计数可在 `/api/metrics` 查看。
- MQTT 依据 CONNACK 更新连接状态，并自动重连。QoS 0/1/2 按配置使用。队列不承诺停电持久化或端到端零丢失。
- 配置以临时文件同步后原子替换，成功提交后才发布内存快照；保留上一版本，损坏时恢复备份并保留损坏文件。
- 同一配置目录由一个进程独占。备用进程使用 `--standby --auto-start`，在主进程退出后取得锁并接管，锁轮询周期为 250ms。

同机主备示例：两个进程使用相同目录、端口及 `--strict-port --headless --auto-start`，第二个额外加 `--standby`。生产环境需由操作系统服务管理器负责重启主备进程。

这属于进程级高可用，不是跨主机集群或零停机升级。跨主机需要独立仲裁、存储复制、入口切换和 fencing；不能将网络共享盘上的文件锁直接当作已验证的分布式锁。完整边界、行为差异和验收证据见 `docs/VALIDATION.md`、`docs/ACCEPTANCE.md`。

## 目录

| 路径 | 内容 |
| --- | --- |
| `src/` | Rust 协议、采集调度、MQTT、API、配置、日志、AI 与桌面集成 |
| `web/src/` | Rust/WebAssembly 界面逻辑 |
| `web/dist/` | 嵌入程序的界面资源及生成的绑定 |
| `tests/` | 原版对照、协议模拟、故障注入、进程与浏览器测试 |
| `examples/` | 本地模拟器及演示配置 |
| `tools/` | 基准生成与性能对照脚本 |

许可：MPL-2.0。上游归属见 `NOTICE.md`。
