# Linux GUI：第一批真实数据接入

状态：**查看与诊断已接通；不是完整 GUI，也不是生产安全服务**。
用户要求先推进 GUI，因此仅提前使用已实现的读取接口。阶段 4 真机门禁、阶段 5 写操作和目标桌面验收没有因此通过。

## 可用页面

| 页面 | 当前能力 |
| --- | --- |
| Devices | 在线串口和离线登记；名称/描述/VID:PID/serial/端口搜索；全部/待处理/已登记筛选；详情、稳定链接、线路配置、检查结果；未登记设备显示 daemon 提供的草稿，**不会保存** |
| Logs | 设备、事件/前缀、文本、RFC3339 时间范围过滤；显示可信操作者、前后值、备注及坏日志行数量；复制当前结果为 JSONL，不写系统导出目录 |
| Policy and diagnostics | 显示实际策略；读取 check/doctor 结果与下一步；空报告不作为硬件通过证明 |

登记/编辑、保存并应用、暂停/恢复、enforce、停止/启动/自启设置还没有 GUI 操作入口。
Windows 保持现有只读 PnP 设备页面和明确标注的 NSH 验证工具；不会伪装成 Linux 管理器。

## 数据与生命周期

- `daemon/transport.rs` 的自有 OS 线程承载 Tokio；NSH engine 线程只等待有界通道并写 signals，不要求 engine 自带 Tokio reactor。
- 先建立事件订阅，再查询快照。`ListResponse.event_sequence` 是单一 daemon worker 给出的同步边界；已经包含在快照里的排队事件不重复应用，边界之后漏号/重复会令数据失效并重新同步。
- 同一轮查询和订阅核对 socket peer PID/uid。请求/响应编号、协议版本、64 KiB 请求和 4 MiB 响应上限严格检查；不完整、非 UTF-8 或不一致的数据不能当成功。
- 没有游标的旧 daemon 明确拒绝，提示升级，不假装能可靠同步。CLI 仍可读取省略该可选字段的历史响应。
- 意外断开保留上次数据并标明可能过期，按 1/2/4/8/16/30 秒退避重连；**不是设备/服务状态轮询**，从不调用 systemd 启动服务。
- 收到明确的 `service_stopping` 后显示 `Stopped · unprotected` 并停止自动重连。启动时连不上 socket 只写“状态未知”，不猜服务已经停止。
- `Reconnect` 是手动重新连接，**不是 StartUnit**。当前若服务已停止，需在应用外启动，再按 Reconnect。
- selection 按登记名或串行身份保留；无 serial 时绑定 ID_PATH。过滤、重排和 tty 编号复用不会把详情转移到其他设备。列表虚拟化，旧请求结果不得覆盖新的选择或连接会话。
- 刷新、检查、诊断、日志查询有执行中状态，禁用重复提交。页面切换保留搜索和日志过滤草稿；事件不重建整个页面。
- GUI 退出只关闭自己的工作线程/连接，**不发送 shutdown**。Linux 托盘与 `X` 隐藏行为仍待实现/原生验收，不能以 Windows 结果替代。

读取检查/详情会由 daemon 打开 tty 读取 termios，可能导致 DTR 置位。GUI 本身不打开设备，不写 `/etc`、`/sys` 或 `/dev`，不提供固件操作。

## 启动

运行机器需要 GTK 4.14+ 开发文件，GUI 必须是普通用户。Linux 当前系统模式要求已安装并运行**同一 checkout 的 daemon**、socket `root:devguard 0660`，用户属于 devguard 组。

```sh
cargo build -p devguardd -p devguard-gui --locked
cargo run -p devguard-gui --locked -- --socket /run/devguard/ctl.sock
```

这两条命令**不会安装或重启系统服务**。更新旧安装是管理员另行审核的动作，见 [`systemd.md`](systemd.md)。本轮没有替换已安装的 daemon、规则或 polkit 文件。

也可在隔离开发环境中运行：先确认使用未占用的私有目录，两个终端以同一普通用户启动：

```sh
# 终端 1：私有配置/审计树，真实只读 udev 枚举
cargo run -p devguardd --locked -- --root "$HOME/.cache/serialguard-gui-session"

# 终端 2：不会写该目录，只与 daemon 通信
cargo run -p devguard-gui --locked -- --socket "$HOME/.cache/serialguard-gui-session/run/devguard/ctl.sock"
```

沙箱不是 polkit/主机权限安装验收。这里没有假设备开关；没有 USB/登记时应诚实显示空列表。

`App` 是默认 Linux 工作区。`ValidationApp` 是独立阶段 0 工具，其 NSH 配置在 `devguard-gui/validation/nsh.json`；不能在 daemon 连接失败时自动切到它。NSH 0.4.0 仅识别名为 `nsh.json` 的 Rust 根配置，因此没有使用自造的替代文件名。

## 验证记录（2026-11-08）

- Linux/WSL：core/daemon/CLI **153** + GUI **52** = **205** 项通过。
- Windows：core/daemon/CLI **80** + GUI **51** 项通过；Windows 生命周期测试仍通过。
- 两端 core/daemon/CLI/GUI 的 `clippy --all-targets --locked -- -D warnings` 通过（Linux 使用 headless 特性选择）。
- GUI 的隔离 Unix socket 测试使用真实 daemon/协议/持久化，DeviceIo 是惰性测试实现；它们验证集成，**不是真机事件或 termios 验收**。
- 覆盖初次同步、配置事件进入真实 NSH 页面、查询所有页面、未授权读取拒绝、帧错误、正常停止不重连、意外断开重连、退出不停止 daemon、选择/改号/过期回复、时间过滤和主题/尺寸切换。
- 并行回归暴露旧 socket fixture 的 fd 继承窗口：父线程 bind/drop 不保证子进程也已关闭 fd。用有子进程监听的最小例子复现了拒绝；遗留 inode fixture 改为无监听 fd 的确定性构造，并新增“子进程仍监听必须保留”回归。**没有放宽生产接管规则**。
- 这次 WSL 一度停止且启动超时；普通唤醒恢复后完成全部回归。没有重置发行版或重复真机插拔。
- 本机缺 GTK 开发文件，`pkg-config gtk4` 失败；**Linux desktop 功能构建、窗口截图、原生字体/焦点/IME/Wayland/托盘仍未验收**。headless 不是外观证明。
- 当前主机 PATH 没有 NSH CLI，`nsh check` 未完成；上述 Rust 检查/测试不冒充 TSX 工具测试。保留 0.4.0 版本，不为功能改动自动升级工具。

无 GTK 的 Linux 软件验证：

```sh
cargo test -p devguard-core -p devguardd -p devguardctl -p devguard-gui --no-default-features --locked
cargo clippy -p devguard-core -p devguardd -p devguardctl -p devguard-gui --no-default-features --all-targets --locked -- -D warnings
```

`linux-desktop` 默认启用，缺 GTK 时桌面构建仍应失败；`--no-default-features` 仅用于 headless 验证，该构建尝试启动 GUI 时明确退出失败。

后续顺序：登记/编辑表单与差异确认 → polkit 授权保存/应用 → 服务控制与完整停止选项 → 原生 Linux 桌面和真机逐页面验收。
