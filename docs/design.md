# devguard 设计决策与实测记录

基线：[`requirements.md`](requirements.md)。当前处于阶段 0，本文中的待验证内容尚非真机结论。

## 开发顺序与环境

目标运行平台是 Linux / GTK 4.14+ / systemd / udev。当前编码工具运行于 Windows。Windows 上编译 Rust、headless 测试和窗口检查用于发现代码问题，Linux/Wayland 验收仍须在目标环境执行。

NSH 固定使用当前已安装的 `0.4.0`，精确 API 以该版本 crate 源码为准；官网 main 文档可能更新更快。GUI 为纯 Rust，保留官方模板的 `#[native_module]`、`#[component]`、`nsh.json` 和平台入口结构。

## 需求第 3 节的决策登记

| 决策 | 实现约束 | 实测状态 |
| --- | --- | --- |
| 身份键 | serial 存在时只以 serial 匹配；不能在 serial 不匹配时退回 path | core 初稿需复核；待阶段 1/2 |
| PX4 bootloader | 同一逻辑设备多 ids；未知 PID 不自动猜为可信身份 | 待真机运行态/bootloader 对照 |
| 波特率 | apply 设置并回读；run 保持 fd 至 exec 的行为需 Linux 验证；CDC-ACM baud 不作漂移校验 | 待 pty / CDC-ACM |
| ModemManager/brltty | udev 忽略标记；doctor 说明抢占证据与修复方式 | 待 udev 和服务实测 |
| 自动挂起 | 只操作已确认 USB 设备的 power 属性；service 暂停禁止写 | 待 Linux sysfs |
| 轻量 udev | 链接、权限、systemd oneshot 触发；停止时 oneshot 不能意外拉起 daemon | 待单元依赖与 udev 测试 |
| 串口范围 | ttyACM/ttyUSB 只是候选；阻断前还要核查 USB 父设备影响范围 | 待复合 USB 描述符与接口检查 |
| audit/enforce | audit 默认；服务端重新 check 后切 enforce，不能只信任 GUI 先前的检查结果 | 待阶段 5 |

## 必须在实现前解决的语义

### 复合 USB 设备

对整个 USB 设备写 `authorized=0` 可能影响多个接口。仅检查 tty 名称不能证明其他接口不会被断开。实现必须检查授权目标是否还包含 HID、hub、存储等禁止阻断的功能；无法确定时不执行阻断并在 check/doctor 报告。不能仅凭 tty 子接口来关闭 USB 父 hub。

### 暂停结束与“保持放行到拔出”

截止时间需要单调时钟控制，墙上时钟用于协议显示和审计。恢复策略、GUI 未在线/用户未选择时的默认处理、被拦截导致 tty 消失与真实拔出的区分，须在阶段 5 写成可测试状态机。不能用等待 GUI 回答的无期限例外形成永久暂停。

### 停止后的恢复与启动审计

设备被反授权后 tty 可能消失，必须保留可信的物理设备定位，才能按 shutdown 的选项重新授权；此定位不能只以可复用的 `/dev/ttyUSB0` 为键。

启动/自启由 systemd D-Bus 执行。daemon 不能把它自己的 root uid 当成 GUI 操作者；离线动作的可信身份、选项与备注如何进入本机日志，需要与 systemd/polkit 的实际可用证据验证后确定。不得接受未经验证的客户端身份补记。

### 自定义 polkit 动作与 systemd 动作

规格同时要求 `dev.devguard.service-*` 的领域动作名以及 systemd 的 `manage-units` / `manage-unit-files` 授权。实现须明确每条调用链；提供的可选规则默认禁用，并验证 systemd 版本是否向规则提供足够的 unit 细节，避免变成全局 unit-file 管理授权。

### 配置校验与规则转义

严格 schema、同一 id 的唯一性、名称路径安全以及 udev 元字符必须在服务端统一验证。不能将序列号原样插入 udev 规则造成通配符匹配或规则注入。备份和原子写需要避免路径穿越、符号链接及忽略损坏配置的问题。

## NSH UI 基础规范

- 目标：Linux 桌面，键鼠操作；阶段 0 可用当前主机验证公共 Rust/headless 行为。
- 页面布局：常驻顶部状态、设备/日志/策略导航、清晰的内容区；不以色彩单独表达状态。
- 颜色全部由语义 theme token 定义，包含背景、正文、边框、强调、warning、success、destructive；支持浅/深色。
- 间距使用 NSH 的 4pt 刻度；按钮/输入提供 hover、active、focus、disabled 样式。
- `nsh-base` 是无样式行为层；按钮、字段、对话框、状态横幅统一封装于 GUI 的 `ui/`。
- 列表用稳定 domain key 与虚拟化；回收单元读取绑定中的当前 index，不保存每行状态。
- NSH signals 只在 engine 线程更新；后台 socket / I/O 通过可取消异步通道送入 `spawn`，没有定时读取队列或状态轮询。
- spike 只验证组件和事件传递，清晰显示“阶段 0 验证”；不能显示为真实设备状态。

## NSH 能力缺口（待版本源码核验）

- 托盘/桌面通知：尚未完成 0.4.0 API 核验。若缺失，按需求使用 daemon 经 D-Bus 发桌面通知；需确定用户 session bus 的可信路由，不自造通知系统。
- 窗口关闭前确认：需核查原生关闭事件/拦截能力，覆盖窗口按钮与 Hyprland 关闭动作，不能只在自定义“退出”按钮上提示。
- Linux GTK / Hyprland、IME、焦点与原生弹层：当前环境未验证，见 `nsh-spike.md`。

## NSH 0.4.0 的实测 API / 工具差异

- Windows UNC 路径下 `nsh check` 可构建 Rust，`nsh test` 的 JS bundler 报 `UNRESOLVED_ENTRY //?/UNC/...`；改用本机临时校验副本后最小启动测试通过。复现与脚本见 `nsh-spike.md`。
- `ButtonElement` 等 `#[component]` 生成的 builder 在 0.4.0 不是 `Interactive` / `Accessible`，未转发 `test_id`；`RadioGroup` 也没有 `label` prop。测试通过组件的角色/可访问名称定位，host input/list/text 才使用 `test_id`。
- 组件 props 使用 `IntoProp`，host 元素绑定使用 `IntoReactive`。共享组件包装器传入有明确返回类型的闭包，避免混淆这两类接口。
- `outline` 宽度是静态 `Into<AbsoluteLength>`，要跟随主题在 `style_with` 中更新；颜色自身可绑定。采用真实源码签名，不改 NSH。
- headless driver 0.4.0 在 input 获焦时会提早返回、忽略 Escape（`nsh-testing/src/headless/keyboard.rs`）；modal 取消契约使用 `back()` 验证，原生脚本另验实际 Escape。不能据此假定原生平台也失败或已经通过。
- 原生 Windows 0.4.0 实测：对话框内 input 获焦时按 Escape 不关闭 modal（`app.keyboard.press("escape")` 后对话框仍在）；Cancel 按钮始终可用。原生脚本据此采用“先 Escape、失败则 Cancel”，Linux GTK 验收时重新核实 Escape 行为。
- `nsh_core::background()` 不能在 engine 线程调用：TSX headless runner 中该线程没有 Tokio reactor，`tokio::spawn` 直接 panic（`App.test.tsx` 三个用例复现）。改用与 spike worker 相同的模式：OS 线程跑子进程与读取，经有界 `tokio::sync::mpsc` 通道与 engine 侧 `Receiver::recv().await` 通信；通道关闭使线程退出并终止子进程。此约束适用于后续 daemon 事件流的 GUI 接入。
- 长列表滚动到底再筛选时，0.4.0 会保留超出新结果范围的滚动位置。显式改变搜索/筛选后，使用 `after_answer` 将列表滚动到起点；后台事件到达不触发滚动重置。对应回归测试保留。

## 阶段 0 事件与界面的边界

- 每次发送动作在真实 OS 线程中产生记录，界面只从容量为 8 批的 `tokio::sync::mpsc` 接收；每批最多 64 条，每次最多 1000 条，总记录上限 4096。
- receiver 在 NSH `spawn` 中等待事件，不使用定时器轮询。窗口 owner 销毁会取消接收任务、关闭通道，让背压等待中的 producer 退出。
- 页面和详情读取私有 store；搜索/选择以稳定 id 为依据，回收行仅持有 computed 绑定；编辑草稿属于对话框的 owner。
- `PausePreview` 只是阶段 0 的 UI 交互验证。它用 NSH 单调时钟与可取消定时任务，真实 daemon 暂停、授权和恢复仍待后续阶段。
