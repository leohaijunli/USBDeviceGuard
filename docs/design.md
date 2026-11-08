# devguard 设计决策与实测记录

基线：[`requirements.md`](requirements.md)。阶段 4 门禁尚未收口；用户现要求提前推进 GUI，只使用已通过软件门禁的读取能力。本文中的软件验证不能替代 Linux 真机或桌面验收。

## 开发顺序与环境

目标运行平台是 Linux / GTK 4.14+ / systemd / udev。当前编码工具运行于 Windows。Windows 上编译 Rust、headless 测试和窗口检查用于发现代码问题，Linux/Wayland 验收仍须在目标环境执行。

**命名（2026-11-08）**：产品对外发布名定为 **SerialGuard**（`devguard` 在 crates.io 已被占用），crate/二进制在阶段 7 打包时机械改名为 `serialguard-*`；在此之前开发保持现有命名。详见 [`release.md`](release.md)。

NSH 固定使用当前已安装的 `0.4.0`，精确 API 以该版本 crate 源码为准；官网 main 文档可能更新更快。GUI 为纯 Rust，保留官方模板的 `#[native_module]`、`#[component]`、`nsh.json` 和平台入口结构。

### Linux 壳的当前安全边界

- WSL2 Ubuntu 26.04 已可用于 Linux 编译和真实 Unix socket 测试；WSL 中的 USB、polkit 桌面提示和 Arch/Wayland 仍须分别验收。
- 在现有 `Daemon` / `DeviceIo` 接缝上接入 Linux 库存和 RPC，单一 worker 串行执行决策、持久化审计和后续 effect。枚举失败必须显式暴露（list 使用 problems），不能伪装成健康的空设备清单。
- 此版本只运行 audit；termios apply 可用，授权/power 写入、策略切换和暂停仍拒绝。不会把尚未通过复合设备/策略验证的状态机暴露为真实拦截功能。
- system 模式使用内核 peer uid/gid/pid、`/proc` 启动时间/组和本地账户；polkit 的 process subject 包含 `pid,start_time,uid`，不接受客户端身份字段。
- `--root` 是同 uid、私有文件树的开发能力，不是绕过生产 polkit 的开关。生成规则不 reload 主机 udev，但显式 apply 可打开串口并写 termios；sandbox 结果不得计为 polkit 或权限安装验收。
- 配置保存/删除与规则激活不是跨文件事务：若规则更新失败，明确返回已保存配置的 `rules_failed`，保留真实配置事件与审计；不隐式恢复旧配置或宣称设备已应用。
- 客户端与服务端共同限制帧长；响应编号必须匹配，事件序号递增。慢订阅者落后时明确断开并要求新快照，不能静默丢事件。

### systemd 就绪与 socket 生命周期（2026-11-08）

- 单元使用 `Type=notify`：初始快照、审计和 RPC worker 初始化完毕后才报告 READY。手动启动没有 NOTIFY_SOCKET 时不依赖 systemd；错误的通知地址导致启动失败，不谎报 ready。
- socket 接管先持有私有 `daemon.lock` 的非阻塞 flock。锁文件不可删除以免生成两个锁 inode；关闭/崩溃会释放内核锁，子进程不会继承 CLOEXEC fd。SocketGuard 先清理自身 inode，再释放实例锁。
- 遗留 socket 只在归当前 uid、无额外硬链接、非阻塞探测明确返回 ECONNREFUSED、再次核对 inode 后删除。满队列/EAGAIN 不等于死 socket。非 socket、符号链接和活监听者保留不动。
- apply@ 只带 After 排序和运行状态 ExecCondition，不通过 Wants/Requires 拉起 daemon。单元契约已有离线测试，真实停止期间插拔仍待验收。
- WSL 自动 apply 当前仅 3 个完整周期通过；中断续跑保持旧证据，序号连续性只对同一订阅会话作断言，不能假装服务重启后的事件序号仍连续。

## 需求第 3 节的决策登记

| 决策 | 实现约束 | 实测状态 |
| --- | --- | --- |
| 身份键 | serial 存在时只以 serial 匹配；不能在 serial 不匹配时退回 path | core 初稿需复核；待阶段 1/2 |
| PX4 bootloader | 同一逻辑设备多 ids；未知 PID 不自动猜为可信身份 | 待真机运行态/bootloader 对照 |
| 波特率 | apply 设置并回读；`termios::apply` 已实现"设置+回读+比对"一体化（`TCSANOW`，`BaudRate::NotApplicable` 不写速度不比对）；run 把设备 fd `dup2` 到 0/1/2 后 exec，pty 端到端已验证；CDC-ACM baud 不作漂移校验 | pty 门禁已过；CDC-ACM 实机回读待真机确认 |
| ModemManager/brltty | udev 忽略标记；doctor 说明抢占证据与修复方式 | 待 udev 和服务实测 |
| 自动挂起 | 只操作已确认 USB 设备的 power 属性；service 暂停禁止写 | 待 Linux sysfs |
| 轻量 udev | 链接、权限、systemd oneshot 触发；停止时 oneshot 不能意外拉起 daemon | 待单元依赖与 udev 测试 |
| 串口范围 | ttyACM/ttyUSB 只是候选；阻断前还要核查 USB 父设备影响范围 | 已实现只读复合接口判定（`ID_USB_INTERFACES`）；enforce 前的 I/O 拒阻仍待真机验证 |
| audit/enforce | audit 默认；服务端重新 check 后切 enforce，不能只信任 GUI 先前的检查结果 | 待阶段 5 |

## 必须在实现前解决的语义

### termios 的读取副作用与 pty 局限（2026-11-08 实测）

- 读 termios 必须打开 tty（`stty` 同样的副作用）：部分驱动把 open 当作 DTR 置位。`check` 用 `O_RDONLY|O_NOCTTY|O_NONBLOCK` 最小化影响，不向线路写任何字节；这是检查真实线路参数的固有代价，不是可消除的副作用。
- pty 驱动强制 CSIZE=CS8 并可能对标志组合返回 EINVAL：数据位 5-7 的回读验证只能在真机驱动（cdc_acm/ftdi 等）上完成；baud/parity/stop_bits/flow/raw 在 pty 上可完整往返（Ubuntu 26.04 / 6.6 内核实测）。
- 真机实测（2026-11-08，cdc_acm 经 vhci 链路）：`cs7` 写入后读回仍 CS8 —— 该驱动/固件组合不保持数据位 5-7，`apply` 的回读契约会诚实失败而非静默错误配置；8N1 完全可用。
- `termios::apply` 的契约是"设置 → 回读 → 比对"，回读不一致返回错误并附逐字段描述；调用方（`LinuxIo::apply`、`devguardctl run`）不得自行声称成功。
- env 文件（`/run/devguard/<name>.env`，沙箱在私有树内）是 daemon 与 CLI 的稳定契约：`run` 不联系 daemon，env 缺失或设备消失都明确报错，不静默换设备。

### 复合 USB 设备

对整个 USB 设备写 `authorized=0` 可能影响多个接口。仅检查 tty 名称不能证明其他接口不会被断开。实现必须检查授权目标是否还包含 HID、hub、存储等禁止阻断的功能；无法确定时不执行阻断并在 check/doctor 报告。不能仅凭 tty 子接口来关闭 USB 父 hub。已把 `ID_USB_INTERFACES` 解析为只读 `interface_unknown` / `forbidden_interface_class`，doctor 逐设备报告；enforce 的 I/O 拒阻（`set_authorized` 返回）仍待真机验证。

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

### GUI 先行的读取边界与快照同步（2026-11-08）

- 默认 Linux App 使用真实 daemon 工作区；Windows 保持只读诊断。ValidationApp 是显式测试工具，不作为离线/权限错误的回退。
- 有界 OS 线程运行自己的 Tokio runtime，两条 Unix socket 分离订阅与查询；只有读取操作的枚举可进入 GUI bridge。不得在 UI 中自报操作者或直接打开/写设备。
- list 增加可选 `event_sequence`：worker 返回快照时的事件边界，先订阅后快照的客户端跳过被包含的排队事件。编号间断需要重新同步，不能对 devnode 复用的旧 remove 盲目重放。旧 CLI 解码仍兼容；GUI 对无游标 daemon 拒绝猜测。
- 选择以登记名、VID:PID+serial 或无 serial 时的 ID_PATH 为键。重复身份的在线端口归在同一读取对象；保存/应用工作流必须进一步明确目标端口，不在本次读取入口中执行。
- 只在新的选择/快照/事件到达时加载详情，旧票据和旧连接回复不覆盖新状态。日志草稿与已提交过滤器分离，打字不发送查询；执行中有明确禁用状态。
- 确切 service_stopping 使 GUI 停止自动重连；未知掉线仅标未知并退避重连，从不启动 systemd。原生停止/自启控制和可信离线动作审计仍属后续门禁。
- 原生 GTK 不可用时采用显式 headless 构建选项，不把依赖缺失伪装成成功桌面应用。完整范围及验证见 [`gui-daemon.md`](gui-daemon.md)。

## NSH 能力缺口（待版本源码核验）

### Windows 后台窗口设计修订（2026-11-07）

- 工程师要求 `X` 只隐藏后台，托盘右键退出才结束 GUI，详见 `ui-design.md`。
- 已核查项目解析的 NSH 0.4.0：`AppOptions` 不提供关闭策略/托盘回调，但公开 `AppView::new(Some(parent), ...)`、`hwnd`、`load`、`stop` 供原生宿主嵌入。
- 实现方向是应用自己的 Win32 顶层窗口和原生托盘菜单，NSH 作为子视图；不修改依赖源码、不臆造 NSH 关闭/托盘 API。原生窗口和托盘行为属于应用宿主，设备数据属于现有 HardwareStore。
- Windows 的 `X` 隐藏、`Show` 恢复及 `Exit` 退出已验收；Linux 托盘/Wayland 仍待单独实现与验收。

### 非全屏窗口尺寸与滚动（2026-11-07 修订）

- 宿主窗口尺寸按逻辑点换算 DPI（`GetDpiForWindow` + `AdjustWindowRectExForDpi`），不再用物理像素创建 1120×760：150% 屏下后者会变成 747×507 逻辑点，导致非全屏内容被截断。
- 默认窗口 1120×760 逻辑点，居中并限制在工作区内；`WM_GETMINMAXINFO` 给最小 900×700 逻辑点，`WM_DPICHANGED` 采用建议矩形并重排内嵌 `AppView`。
- 全局 `set_scrollbar_theme(ScrollbarMode::Always)` 让有滚动范围的视图常显可拖动条；NSH 的 `shown()` 在无范围时不绘制，因此不会到处出现假滚动条。
- 页面区外层 `scroll()` 的 padding 移到内容子视图。实测：padding 放在 scroll 自身时，每个尺寸都会多出几十点的假滚动（内置条以绝对定位贴在滚动视口上，其溢出矩形会再叠加 scroll 的 padding）；放在内容上则滚动范围只在内容真正超高时出现。
- 列表 `content_padding_x` 为滚动条预留右缘空间，避免滑块压住行尾内容。
- headless 回归新增：最小 900×700 视图下页面整体可见，指南在其自己的滚动视口内可达底部。

### 新设备通知与紧凑详情（2026-11-07 修订）

- `hardware/arrivals.rs` 是快照差分与去重的唯一实现：首个成功快照为基线，USB 串口实例 ID 大小写不敏感，使用 NSH 单调时钟作五分钟冷却。过期记录只在快照到达时清理，不轮询设备。
- `HardwareStore` 将新增事实交给 `ui/toast.rs`；`ToastManager`/`ToastStack`/`Toast` 使用 NSH 0.4.0 公开 API，状态在绑定中读取，定时器仅在有提示且窗口可见时推进生命周期。
- `desktop_notifications.rs` 是应用宿主的原生边界：有界队列（8 批通知、32 条可见性/选择事件）和无指针 Win32 消息连接两个线程。隐藏时调用系统 `Shell_NotifyIconW`，点击后恢复同一窗口；不改 NSH、不另造通知窗口或查询设备。
- 设备页消费一次性定位请求，核对设备仍在线并打开详情；当前模态保留。设备拔出后旧提示不可打开错误设备。
- PowerShell 的 `Register-WmiEvent` 无 `-Action` 时可能没有返回值；成功与否以 `Get-EventSubscriber` 为准，仍先订阅后建立快照。GUI owner 销毁会取消并回收自己的监视进程，不等待下一次设备广播。
- `ui::metadata` 统一紧凑标签—值列；页头、面板头、列表与详情采用同一 4pt 刻度，典型行估计同步到 56pt，长备注不挤入窄值列。
- 测试快照和注入入口仅在 `cfg(test)` 下编译；物理插拔和原生后台通知的可见效果仍需真机验收。

- Linux 托盘/桌面通知：仍待实现与核验；若 NSH 缺系统接口，daemon 经 D-Bus 发桌面通知，并验证用户 session bus 路由。
- Linux 窗口关闭动作（含 Hyprland）需符合 `X` 隐藏语义，不再采用旧的关闭确认提示。
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
