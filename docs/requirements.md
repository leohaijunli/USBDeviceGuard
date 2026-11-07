# devguard — 产品需求与验收基线

记录日期：2026-11-06。来源：用户在本会话提供的规格及后续“记录到 md 文件，开发这个 app”。

本文作为开发基线。服务控制采用用户修订后的第 4 节；协议、GUI、授权和验收中的旧描述均以该修订为准。实测与需求冲突时在 [`design.md`](design.md) 记录事实、证据和待确认决策。阶段进度见 [`progress.md`](progress.md)。

## 1. 目标与平台

构建类似 USBGuard 的 USB 串口设备策略管理器，增加 **NSH Rust-first GUI** 和 **限时暂停**。

- 平台：Linux，udev + systemd；目标开发桌面为 Arch/Omarchy、Wayland/Hyprland，GTK 4.14+。
- 设备：PX4 飞控（USB CDC-ACM）、RadioLink / SiK 等 USB 数传、FTDI / CP210x / CH340 串口适配器。
- 已登记设备须具备稳定端口名、正确权限和可检查的串口参数；配置可记录、修改，全部变更可审计。
- 运行时为本机离线应用，不实现联网、云同步。
- 工程师于 2026-11-07 明确：本应用不用于烧录设备固件，不提供固件下载、刷写或升级入口；仅做 USB 串口设备管理与诊断。

## 2. 必须遵守的架构

```text
devguardd（Rust，root，systemd） <-- Unix socket / JSON-lines --> devguard-gui（NSH，Rust，普通用户）
           |                                                   devguardctl（CLI，同一协议）
           +-- udev 规则 / sysfs authorized / termios / journal

devguard-gui -- zbus / systemd D-Bus --> devguardd.service 启动与自启设置
```

工作区四个 crate：

| Crate | 职责 |
| --- | --- |
| `devguard-core` | 配置 schema、严格校验、协议类型；三端共用 |
| `devguardd` | 特权操作、策略、事件、审计 |
| `devguardctl` | 命令行客户端；同一协议；`run` 启动串口使用程序 |
| `devguard-gui` | NSH Rust GUI、事件订阅、systemd 服务控制 |

约束：

- socket：`/run/devguard/ctl.sock`，模式 `0660`，组 `devguard`。
- 服务端用 `SO_PEERCRED` 获取调用者 uid；用户名由可信本机账户查询取得，不信任客户端自报身份。
- 查询允许 `devguard` 组成员；修改、暂停、恢复、停止等需 polkit 授权，拒绝同样记录。
- GUI 不得 root 运行，不直接写 `/etc`、`/sys`、`/dev`。
- GUI 状态从 `subscribe_events` 事件流更新，不轮询设备或服务状态。倒计时可由已接收截止时间本地显示。
- 不使用 systemd socket 激活。

## 3. 关键设计决策

1. **身份键**：优先 `VID:PID + ID_SERIAL_SHORT`；无序列号退化为 `VID:PID + ID_PATH`。GUI 标注“绑定物理口”。
2. **PX4 多身份**：同一飞控可登记多个 `ids`，覆盖 app 和 bootloader 的不同 PID，避免刷固件被拦截。
3. **termios 的持久性**：端口关闭后不保证保留参数。`apply` 设置并回读校验；生成 `/run/devguard/<name>.env`，包含 `DEVICE=`、`BAUD=`；`devguardctl run <name> -- <cmd>` 先设置再 exec；`check` 比较实际值。CDC-ACM 的 baud 为 `n/a`，不校验 baud。
4. **ModemManager / brltty**：已登记设备规则包含 `ENV{ID_MM_DEVICE_IGNORE}="1"`；CH340 被 brltty 抢占时 `doctor` 给出明确提示。
5. **自动挂起**：已登记设备设置 `power/control=on`、`power/autosuspend_delay_ms=-1`。
6. **udev 轻量化**：规则只设置 `SYMLINK`、`GROUP`、`MODE`、ModemManager 标记，以及 `TAG+="systemd", ENV{SYSTEMD_WANTS}="devguard-apply@%k.service"`。长任务交给服务。
7. **范围**：只管理 tty 子系统中的 `ttyACM*` / `ttyUSB*`，不得阻断键盘、鼠标、hub、存储，包括复合设备中共享同一个 USB 父设备的这些功能。
8. **策略**：默认 `audit`（只记录），可切 `enforce`（未登记串口设备 `authorized=0`）；切换前 `check` 必须无告警。

## 4. 暂停与服务控制（修订版）

### 4.1 三种状态

- `Running`：audit 或 enforce。
- `Paused { kind, until }`：kind 为 `enforcement` 或 `service`。
- `Stopped { autostart }`：进程已退出，autostart 表示是否开机自启。

### 4.2 暂停

- 提供 5 / 15 / 30 / 60 分钟；上限 60 分钟，禁止永久暂停。
- 到期自动恢复；允许“立即恢复”。
- `enforcement`：暂停未登记设备的拦截，放行并记录；已登记设备的应用和偏差修复继续。
- `service`：进程仍在，但不拦截、不修复、不写 sysfs / termios；只写 journal 与事件。
- 暂停状态只存内存，进程或机器重启均回到 Running。
- 暂停时新插入的设备记录 `allowed_during_pause`。
- 恢复时 GUI 对这些设备提供“登记 / 拦截 / 保持放行到拔出”。
- 暂停期间禁止修改策略模式。

### 4.3 停止与启动

停止对话框必须说明：停止后不再拦截，不再维护波特率与自动挂起；udev 符号链接和权限仍有效；设备不会因停止操作被断开。

停止选项：

1. “放行已被拦截的设备”（默认勾选）或“保持拦截”。
2. “仅本次停止”或“停止并禁用开机自启”；禁用自启需二次确认。
3. 可填写备注，全部选项与操作者进入审计记录。

停止流程：

```text
GUI -> shutdown { release_blocked, note }
daemon -> journal service_stopping（操作者、选项、备注）
       -> 按选项处理已拦截设备
       -> 通知所有订阅者 service_stopping
       -> 正常退出，exit code 0
```

- 启用/禁用自启由 GUI 直接通过 systemd D-Bus `EnableUnitFiles` / `DisableUnitFiles`，不经守护进程代办。
- 服务停止时，用户按“启动”后，GUI 通过 systemd D-Bus `StartUnit` 启动，并由 polkit 授权。
- 启动写 `service_started`，对账停机前后设备并写 `startup_reconcile`（新增、消失）。
- 用户主动停止后 GUI 不自动重连，也不自动启动服务，显示“服务已停止”和“启动”。
- 意外断开采用指数退避重连。
- systemd：`Restart=on-failure`，合法 shutdown 以 0 退出。

### 4.4 授权与审计

- 动作命名：`dev.devguard.modify`、`dev.devguard.pause`、`dev.devguard.service-stop`、`dev.devguard.service-start`、`dev.devguard.service-autostart`。
- systemd 启停实际使用 `org.freedesktop.systemd1.manage-units`；自启使用 `org.freedesktop.systemd1.manage-unit-files`。
- 随包提供仅针对 `devguardd.service` 的 `devguard` 组管理规则示例，默认不启用。
- 暂停、恢复、停止、启动、启用/禁用自启均须授权；所有操作的 uid / 用户名、选项、备注均可在日志页查询。
- 自定义动作与 systemd 实际动作的衔接、服务离线时审计身份的可信来源须在设计和阶段 4/5 验证中明确。

## 5. 配置 schema

路径：`/etc/devguard/devices.d/<name>.toml`。

```toml
name = "px4-main"            # [a-z0-9-]+，也是 /dev/devguard/<name> 的名称
kind = "px4"                 # px4 | radio | serial-adapter
description = "Pixhawk 6X on bench"

[[ids]]
vid = "26ac"
pid = "0032"
serial = "..."               # ID_SERIAL_SHORT；无序列号用 path = "<ID_PATH>"
role = "app"

[[ids]]
vid = "26ac"
pid = "0011"
serial = "..."
role = "bootloader"

[serial]
baud = "n/a"                 # CDC-ACM；数传示例为整数 57600
data_bits = 8
parity = "N"
stop_bits = 1
flow = "none"
raw = true

[access]
group = "auto"               # 探测 dialout / uucp
mode = "0660"
```

校验：未知键报错；baud 属于支持的 termios 集合；name 唯一；同一身份不得属于两台设备。写入前备份至 `/var/lib/devguard/backups/`，临时文件写入后原子 rename。

## 6. JSON-lines 协议

- 版本化的请求、响应、事件类型统一定义于 `devguard-core`。
- 每个响应有 `ok`；错误给出原因与下一步。
- 查询/操作：`list`、`show`、`learn_candidate`、`save_device`、`delete_device`、`apply`、`check`、`policy_get`、`policy_set`、`log_query`、`doctor`、`subscribe_events`。
- 服务控制：`pause { kind, minutes, note }`、`resume`、`shutdown { release_blocked, note }`、`service_state`。
- 事件：`device_added`、`device_removed`、`drift_detected`、`blocked`、`pause_started`、`pause_ended`、`config_changed`、`policy_changed`、`service_stopping`、`service_started`、`startup_reconcile`；暂停放行记录 `allowed_during_pause`。
- 补充协议如恢复后的“保持放行到拔出”，须在实现前写清生命周期、授权和事件语义。

## 7. GUI

先安装 NSH 和官方 coding agent skills；读 Rust 编码指南、设计指南和组件文档；API 名称以所用版本源码/文档为准。

### 用户追加要求（2026-11-06）

“界面要优化，便捷”：以少跳转、可快速定位、常用操作直接可达为设计目标。具体交互与验证项见 [`ui-design.md`](ui-design.md)，不能用视觉装饰代替易用性。

### 页面

1. **设备**：在线与已登记列表；已管理 / 未登记 / 冲突 / 偏差 / 已拦截状态；暂停入口和倒计时。
2. **设备详情/编辑**：多身份（含 bootloader）、串口参数、权限、符号链接、实际 devnode / termios / 驱动 / USB 路径；保存并应用前展示差异。
3. **登记向导**：选在线未登记设备 → 读取 udev 属性 → 选择 PX4 / 数传 / 适配器和合理默认值 → 确认。
4. **日志**：journal 按设备、事件、时间过滤，可导出。
5. **策略与诊断**：audit/enforce 切换前显示 check；doctor 检查用户组、ModemManager、brltty、过期规则、离线登记设备。

### 服务交互

- 顶部常驻：运行（audit/enforce） / 暂停（倒计时） / 已停止（红色“未受保护”）。
- 设备页、策略页均提供“暂停…”和“停止服务…”及规格所述对话框。
- 停止后保留最近快照，标记“数据可能过期”；修改按钮置灰，保留“启动”。
- 工程师于 2026-11-07 修订：窗口 `X` 仅隐藏 GUI 到系统托盘后台，正常退出只能通过托盘图标右键 `Exit`；隐藏和退出 GUI 均不停止 daemon。托盘可恢复同一窗口，不能在无可用托盘入口时隐藏。该修订替代先前的关闭确认提示。
- 工程师确认新增设备提示：仅新出现的 USB 串口触发非模态 Toast，点击 `View` 打开对应设备详情；同一实例 ID 五分钟内不重复提示。首次启动只建立基线，重复快照/改号不算新增。后台使用系统通知，用户点击后才恢复窗口，不抢焦点、不放行/拦截或修改设备。
- GUI 使用紧凑桌面密度：收紧无信息的留白，相关字段集中为标签—值行，同时保留完整值、可见操作、字体缩放和滚动入口。
- 所有修改授权失败给出明确反馈，事件流驱动状态，不存在误阻断输入设备的操作路径。
- NSH 若缺托盘/系统通知等能力，在 `design.md` 记录，采用“守护进程经 D-Bus 发桌面通知”的替代，不自造托盘/通知框架。
- 正式页面连接真实 daemon，不以假数据替代；阶段 0 的组件/事件试验必须清晰标注为验证工具。

## 8. Journal

位置：`/var/lib/devguard/journal.jsonl`。

字段：`ts, event, name, devnode, vid, pid, serial, details, uid, user`。

- 配置变更包含修改前后值。
- 暂停记录时长、kind、备注、操作者。
- 停止、启动、自启变更记录操作者、选项、备注。
- 身份和操作结果不得由 GUI 自报为审计事实。

## 9. 分阶段交付门禁

**每阶段通过验收后才进入下一阶段。无法实测时记录“待验收”，不得当作通过。**

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| 0（目标 1 天内） | NSH 可行性、最小 Linux 桌面应用 | 列表/表格、对话框、表单、横幅、后台线程或 socket 更新 signal、Wayland/Hyprland 窗口；记录 `nsh-spike.md`，阻塞先报告 |
| 1 | core + 配置 | 非法 baud、重复 id、未知键、协议编解码单元测试 |
| 2 | daemon + udev | list/show/save/apply、规则/权限/ModemManager/journal；`udevadm test`、真机插拔后链接权限正确 |
| 3 | termios + check + run | pty 测试 baud/parity/流控，设置与回读、偏差检测 |
| 4 | 事件 + polkit + systemd | 插拔 10 次事件无遗漏/重复；未授权调用拒绝且有审计 |
| 5 | 策略 + 两种暂停 + 服务控制 | 第二套输入设备环境下 enforce；到期/手动恢复、重启清除暂停；全部停止/启动/自启验收 |
| 6 | 正式 GUI | 第 7 节页面逐个接通真实 daemon，按页面验收 |
| 7 | 打包与文档 | Arch PKGBUILD、polkit、systemd、README、PX4/RadioLink 示例、排错表、实测设计记录 |

## 10. 总验收清单

- [ ] PX4 含 bootloader PID 登记后，QGC / MAVProxy 以普通用户连接 `/dev/devguard/px4-main`。
- [ ] SiK 类电台 57600 的 check 一致；stty 制造偏差后 GUI 显示，应用可修复。
- [ ] 插拔 20 次端口名不变，ModemManager 不抢占。
- [ ] bootloader 重新枚举不被拦截。
- [ ] enforce 未登记适配器被拦截并提示；暂停期间接入放行，到期提供登记选择。
- [ ] enforce 下“暂停服务 5 分钟”不改 sysfs/termios，到期恢复并提示登记。
- [ ] 停止默认放行可恢复已拦截设备；选择保持拦截则维持。
- [ ] 停止后链接与权限有效，GUI 红色“未受保护”，不自动拉起服务。
- [ ] 仅本次停止，机器重启恢复；禁用自启后机器重启不启动。
- [ ] `kill -9` 后 systemd 自动拉起，GUI 退避重连，记录 service_started。
- [ ] 进程/机器重启后不处于暂停状态。
- [ ] 停机期间设备增减出现在 startup_reconcile。
- [ ] 日志可查配置前后值、暂停、停止/启动、自启变更的操作者与备注。

## 11. 明确禁止

- 永久暂停，无授权修改，GUI root 运行。
- 拦截非串口类设备或间接使共享 USB 父设备的输入设备失效。
- 运行时联网、云同步。
- 凭记忆编造 NSH API；用桌面/headless 结果冒充 Linux 实测通过。

## 12. 交付物

代码、必要测试、systemd 单元、polkit 策略及默认禁用的可选规则、Arch PKGBUILD、README、`docs/nsh-spike.md`、`docs/design.md`，以及本需求和阶段进度记录。
