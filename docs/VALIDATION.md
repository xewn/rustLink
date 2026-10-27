# RustLink 验证报告

验证日期：2026-10-27。基线为 `D:/PLCLink/FactoryLink`，提交 `dcfeb2c4bda1c8e92151892bf6363b84df2f9628`。原仓库未修改。Rust 1.94.0；Windows x64 与 WSL Ubuntu 24.04 x64。

## 实现与测试方法

后端、PLC 协议、MQTT 集成、配置持久化、桌面入口和网页业务逻辑均使用 Rust；网页编译为 WebAssembly，仅保留生成的 JavaScript 加载绑定。运行程序不依赖原 Python/Vue 项目。保留原版接口、配置和 MQTT 业务消息；增加故障恢复、资源上限、同机主备和运行指标。

先从原版生成固定期望值及模拟数据，再实现 Rust 并逐层验证。保留原版行为的证据包括 1,052 个模型/地址/数值解码/倍率样本、26 步 HTTP 操作序列、7 个 Excel 工作簿。浮点对照采用测试中明确的数值容差，JSON 对象不要求键顺序，动态时间戳不要求相等。

| 检查 | 最终结果 | 证据 |
| --- | --- | --- |
| Windows Rust 单元及集成测试 | 46 通过，0 失败 | `artifacts/windows-tests.log` |
| Linux Rust 单元及集成测试 | 48 通过，0 失败 | `artifacts/linux-tests.log` |
| Windows/Linux Clippy，警告视为错误 | 通过 | `artifacts/windows-clippy.log`、`linux-clippy.log` |
| WebAssembly Clippy 与全工作区格式检查 | 通过 | `artifacts/wasm-clippy.log`；`cargo fmt --all --check` |
| 原版真实协议库与 Rust 对照 | Modbus TCP、S7、MC 的业务包一致 | `artifacts/differential.json`、`differential.log` |
| 浏览器端到端 | 设备增删改、点位、持久化、Excel 预览、启停；无浏览器异常 | `artifacts/ui.log`、`ui-home.png`、`ui-settings.png` |
| Windows 独立发行构建 | 通过 | `artifacts/windows-release.log` |
| 发行包解压到临时目录独立运行 | 内嵌 HTML/WASM、三类 PLC 数据、MQTT、对应源码均通过 | `artifacts/package-verification.log` |

46/48 是测试函数数量，包含上述多组固定样本，并非 46/48 个输入覆盖全部状态空间。Linux 多出的两项为真实串口伪终端收发及 SIGTERM 优雅退出。浏览器回归在修复页面异步加载/导航覆盖输入问题后连续三次通过，最终发行构建后又通过一次。

## 已验证的故障行为

- PLC 短帧、异常码、错误事务/从站、RTU CRC、读取超时；S7 握手及传输分片。RTU 请求间遵守串行静默间隔。
- 故障 PLC 与正常 PLC 并行运行，停止可取消正在读取/重试的任务；反复启停、重试上限、运行中名称/周期变更。
- MQTT 用户密码、QoS 0/1/2 确认流程、连接状态、断线重连。连接握手保留未完成的 poll future，延迟 CONNACK 期间到来的状态消息不再中断握手。
- WebSocket ping/pong、状态通知、落后订阅者隔离；有界队列避免慢消费者无限增加内存。
- 配置并发更新、真实写入失败、失败不发布内存状态、备份恢复、损坏文件留存、跨进程互斥。
- 杀死主进程后，备用进程取得同一目录锁、恢复相同配置并在同一端口开始采集；测试要求接管小于 3 秒。Linux SIGTERM 在有界期限内退出并释放锁。
- AI 普通 JSON、Markdown 清洗、缺失密钥、HTTP/格式错误、UTF-8 跨块 SSE；本地模拟器可验证完整调用链。

## 性能测量

原始数据：`artifacts/benchmark.json`。Windows 11、24 个逻辑 CPU、Python 3.14.3；Rust 使用 release 构建。两者依次连接同一个本机 Modbus 模拟服务，各运行 30 秒；50 台设备，每台 10 个 uint16 点，配置采集间隔 100ms，模拟每次响应等待 2ms，MQTT 关闭。先预热 1 秒，再通过真实 WebSocket 统计值为 42 且质量 good 的点位，每约 50ms 请求健康接口。最终轮次没有同时运行编译或测试任务。

| 指标 | 原版 Python | Rust |
| --- | ---: | ---: |
| 测量时长（秒） | 30.012 | 30.118 |
| 收到有效点位值 | 2,300 | 127,350 |
| 有效点位值/秒 | 76.64 | 4,228.43 |
| 观察到的设备数 | 50 | 50 |
| 峰值进程树 RSS（MiB） | 83.01 | 19.27 |
| 进程树 CPU 时间（秒） | 2.859 | 10.469 |
| 健康接口 P50（ms） | 21.123 | 23.581 |
| 健康接口 P95（ms） | 43.586 | 49.934 |
| 健康接口最大值（ms） | 52.741 | 99.527 |

本轮有效点位吞吐约为原版 **55.17 倍**，峰值 RSS 约减少 **76.8%**。Rust 并行采集消除了原版按设备串行阻塞的主要瓶颈；它完成了更多工作，总 CPU 时间也更高。健康请求延迟没有改善，不宣称各项指标全面更快。

该结果是单机回环短测，不是生产容量上限或持续可用性承诺。Windows 定时器、测试客户端与模拟器同机竞争会影响实际 2ms 等待和 HTTP 延迟；采集间隔也包含读取耗时。真实 PLC 扫描周期、连接数限制、RS-485 波特率及跨机网络会改变结果。需要现场负载和长时间压测才能制定生产容量指标。

## 兼容边界与待现场验收

1. 已验证的是固定输入、模拟网络和列明业务流程，不能据此声称所有输入、PLC 型号与故障组合均已穷举。未生成代码覆盖率百分比。
2. 保留原版布尔值经过倍率/偏移后为数值、MC uint16 的底层有符号字行为、重复设备 ID 最后值及首次出现顺序等兼容细节。Rust 修复原版可能永久跳过断线设备、MQTT 误报在线等问题。
3. Rust 对非法端口/负数、非有限值、损坏协议帧、超大文件更严格；422 错误正文和底层库异常文本不保证与 Pydantic/Python 逐字相同。请求上传上限 16 MiB，Excel 解压上限 64 MiB，AI 响应上限 1 MiB。真正 BIFF `.xls` 和原版一样不受支持。
4. S7 支持配置端口；原版封装实际固定 102。对照模拟使用 1102，因此测试调用原版库时显式指定 TCP 端口，不修改原源码。真实 S7 权限、PLC 保护设置及具体 CPU 型号仍需现场联调。
5. 已验证同机进程主备，不是跨主机集群。未验证网络分区、共享盘锁、仲裁/fencing、主机宕机和跨主机入口切换；不承诺零停机或停电零数据丢失。MQTT/WS 队列有界且可能丢弃旧消息或溢出消息；指标提供 MQTT 溢出计数，未实现持久化消息外发队列。
6. 真实 PLC、Windows COM/RS-485 电气层尚未验证；Linux 伪终端证明操作系统串口路径，但不能代替硬件。Windows 托盘与自动启动代码已编译，未自动点击系统托盘或实际更改登录启动项。应在交付机器人工验收这些系统集成。
7. 网页功能重写并有浏览器验收，不承诺与原 Vue 界面像素级一致。Dockerfile 和 CI 工作流已提供；本机 Docker 守护进程未运行，未宣称容器镜像或远程 CI 已执行成功。

## 复现

普通测试：`cargo test --locked --workspace`。Windows 浏览器：安装 `tests/requirements.txt` 后运行 `python tests/ui_e2e.py`。原版对照：`python tests/system_differential.py D:/PLCLink/FactoryLink`。性能：先构建 release，再执行 `python tools/benchmark.py --duration 30`。详细启动、迁移和主备配置见 `README.md`。

性能对照使用隔离临时目录加载原版，不访问生产设备；原版 SHA-256 已存入 `tests/fixtures/oracle.json`。源代码、固定基准、模拟器和全部测试均随工作区交付。
