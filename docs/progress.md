# devguard 开发与验收进度

更新：2026-11-07。

## 当前门禁：阶段 2（进行中；Linux 真机验收未通过）

需求基线见 [`requirements.md`](requirements.md)。工程师先前暂缓阶段 0 的 Linux 桌面验收并要求继续开发；该偏离保留，不能以 WSL 软件测试代替 Arch/Omarchy/Wayland 验收。

2026-11-06 的历史环境实测：

- 工作目录：`\\Artemis\Users\HLi\Downloads\devguard`，初始为空目录，非 Git 仓库。
- 当前执行环境为 Windows；Rust / Cargo 已安装。
- `wsl -l -v` 报告没有已安装发行版。目标 Arch/Omarchy 桌面尚未接入此会话。
- 已安装 NSH CLI `0.4.0 (e626a8d)`，以及官方 `nsh` / `nsh-design-guides` 项目 skills。
- 先前已生成四 crate 骨架和部分 core 初稿；这是尚未完成验收的工作，不代表阶段 1 完成。
- 先前 `cargo check -p devguard-core` 的 Cargo 输出显示编译完成；当时 PowerShell 包装产生非零退出值，须以可复现的直接命令重新检查。

## 阶段清单

| 阶段 | 状态 | 证据 / 下一步 |
| --- | --- | --- |
| 0 NSH spike | Windows 侧完成；Linux 验收被用户暂缓 | 17 项测试 + 7 张原生截图；`scripts/verify-nsh-linux.sh` 待 Arch/Omarchy 执行 |
| 1 core + 配置 | 完成（软件可测部分） | Windows 28 单元 + 9 集成通过；Linux 另含 1 项 symlink 集成和 5 项 Unix 客户端测试 |
| 2 daemon + udev | **完成（部署级验收通过）**：system 模式规则激活 + 真实插拔后链接权限正确；物理插拔的桌面层（Arch/Omarchy 特有 Wayland 部分）归阶段 0/4 的桌面验收 | 纯核心 22 项、Inventory 4、socket RPC 9、CLI 9 操作、`udevadm verify` 通过；WSL 实机枚举 0→1、热插拔事件闭环、system 模式下 daemon 生成并 reload 真实 `/etc/udev/rules.d/90-devguard.rules`、`udevadm test` 命中、真实插拔后 symlink/dialout/0660/MM 标记全部正确、check 四项 ok（`artifacts/linux-real-{attach,hotplug}-2026-11-07.md`、`linux-system-mode-2026-11-08.md`） |
| 3 termios/check/run | 完成（pty 门禁 + 真机 apply/check/run 验收通过） | WSL 116 项测试（含 pty 9 + run 端到端 2）；真机 apply 写入 115200 8N1 raw 并回读验证、偏差注入/检出/修复闭环、run 的 fd 继承在实机验证（`artifacts/linux-real-termios-2026-11-08.md`）；实测 cdc_acm/vhci 不保持 CS7，5-7 数据位申请将得到明确回读错误 |
| 4 事件/polkit/systemd | 未开始 | 需要 Linux system bus、polkit agent、systemd |
| 5 策略/暂停/服务控制 | 未开始 | 需要可恢复的真实 USB 测试环境 |
| 6 正式 GUI | 未开始 | 当前 GUI 仅最小 NSH 骨架 |
| 7 打包/文档 | 未开始 | 文档随开发更新，系统安装文件按已验收功能交付 |

## 记录约定

- 将“已实现”“编译通过”“headless 测试通过”“Linux 真机验收通过”分开记录。
- 每个实测记录命令、环境、结果；失败保留原因和下一步。
- 不把待办项、空入口或模拟设备计为完成的产品功能。
- 新结果更新本文件与 [`nsh-spike.md`](nsh-spike.md)，设计冲突更新 [`design.md`](design.md)。

## 2026-11-07：本机 WSL 与阶段 2 续开发

### 只读 udev 监视续开发结果

### Windows GUI 设备显示与后台生命周期修订

- 工程师要求本应用不提供固件烧录入口，并修订为 `X` 隐藏后台、托盘右键 `Exit` 才正常结束 GUI；需求和 `ui-design.md` 已同步。
- GUI 空设备问题复现：旧脚本能找到 COM4，但耗时 33.78 秒，超过 25 秒启动看门狗；另将 Bluetooth COM5/COM6 误归为 USB。已改为只处理 USB 来源的串口、按设备缓存批量 PnP 属性，保留 90 秒有界冷启动预算。
- 同机同只读扫描复测：9.86 秒，1 个 USB 串口 `COM4 / V6X Rust CDC / 1209:0001 / V6X-RUST-001`，Bluetooth COM5/COM6 正确隐藏。没有打开串口或改变设备参数。
- Windows 使用应用自己的 Win32 顶层宿主和托盘，嵌入 NSH 0.4.0 的公开 `AppView`，不修改 NSH。`X` 隐藏且窗口/进程保留的原生回归通过。
- GUI `cargo test -p devguard-gui --locked -- --test-threads=1`：34 项通过（20 单元/通知引擎、11 交互、1 原生生命周期、2 Windows 扫描）；GUI clippy `--all-targets --locked -- -D warnings` 通过。
- 修复非全屏内容不全：宿主窗口改为按 DPI 缩放逻辑点（默认 1120×760、最小 900×700、`WM_DPICHANGED` 建议矩形），外层页面滚动视图的内边距移到内容子视图以消除假滚动。实测默认/最小/普通三种窗口内容完整，设备/记录/指南各自滚动条常显且可拖动（2026-11-07）。
- 新版窗口已启动；Windows UI Automation 确认窗口可见且 COM4 可访问。工程师随后确认 COM4 显示、`X` 隐藏、托盘 `Show` 恢复和 `Exit` 退出全部正常（2026-11-07）。Linux 托盘行为未验收，不计为完成。
- 本轮紧凑布局收紧头部/内容区/列表，详情改为共享标签—值行；保留字号、长值复制、单栏回退和区域滚动。
- 本机 150% DPI 截图核对：默认 1702×1128、普通 1500×1106、最小 1372×1106 物理像素，COM4 可见；实际 UI 已显示 `Watching device changes`。这是只读扫描/布局验收，不是物理插拔或后台通知验收。
- 新增只读 USB 串口 Toast 与 `View` 定位，首快照不弹、同实例五分钟去重、自动关闭、断开后禁用查看；模态草稿不受通知影响。隐藏/最小化时有 Windows 托盘通知入口，点击恢复同一窗口，不抢焦点。
- 修复自动更新误判：无返回值的 WMI 订阅不再被当成失败；新增订阅与取消子进程回归。真实插拔及后台通知可见效果仍待验收，未因此提交代码。

- 工程师确认继续只读监听与事件测试；新增 `devguardd/src/linux/monitor.rs`，启动固定 `udevadm monitor --udev --property --subsystem-match=tty`，先完成就绪握手再建立快照。
- 事件触发只读 Inventory 对账，不定时轮询；差异发布增减/更新事件，改号先删旧节点再加新节点。监视异常写 `monitor_failed` 并退出，停止时终止/回收自己启动的监视进程。
- 最近 core/daemon/CLI 软件验收：**WSL 92 项通过、Windows 63 项通过，两端 clippy 干净**；GUI 测试另行统计。新增 Monitor 5 项、Hotplug socket 5 项均使用隔离输入/USB fixture，不是实际硬件插拔结论。
- 实际 WSL `udevadm monitor` 启停及 CLI/私有规则语法验证通过，产物 `artifacts/linux-monitor-cli-2026-11-07.json`。仍未转接 USB，WSL 只读枚举为 0。
- 工程师确认后已下载并交互安装 `usbipd-win 5.3.0 x64`，SHA256/签名有效、exit 0、无需重启。当前设备为 `V6X Rust CDC` / COM4 / `1209:0001` / `V6X-RUST-001` / BUSID `2-3`；普通 bind 成功，列表显示 `Shared`。
- 工具提示 `hhdusbh64` USB filter 可能不兼容，未使用 `bind --force`。工程师已确认转接，attach 因 WSL2 发行版未运行而失败，尚未成功转接。随后优先处理 GUI 密度与新增提示；正式 GUI 仍未接 Linux daemon。

### 真机只读转接与清单/规则验收（2026-11-07 续）

- WSL `Ubuntu-26.04` 运行后重试此前已确认的转接：`usbipd attach --wsl --busid 2-3` 成功，Windows 侧 `Attached`（转接期间 COM4 暂不可用），未弹管理员确认。WSL 内核 `6.6.114.1` 加载 `vhci_hcd`/`usbip_core`，`cdc_acm` 绑定出 `/dev/ttyACM0`（`root:dialout 0660`，166:0）。
- 二进制在复合接口判定提交（`cb1c4e4`）之后重建。实机 udev 只读属性：`1209:0001`、`ID_SERIAL_SHORT=V6X-RUST-001`、`ID_USB_INTERFACES=:020200:0a0000:`（两个接口均为 CDC-ACM，无 HID/hub/存储/视频/音频）。
- **里程碑：Linux 只读枚举 0 → 1**。daemon `--root` 沙箱内 `list` 报告 1 台在线 USB 串口；`doctor` 对实机无问题（`forbidden_interface_class=false`、`interface_unknown=false`）；`learn` 正确给出 serial 绑定草案并取 USB 父路径 `usb1/1-1`（非 tty 节点），带 CDC-ACM baud 不校验告警。
- `save` 后生成私有规则 `90-devguard.rules`（ttyACM/ttyUSB + VID/PID/serial 匹配、`devguard/` symlink、`dialout 0660`、`ID_MM_DEVICE_IGNORE`、systemd apply 钩子）。真实 `udevadm verify` 通过（Success: 1 / Fail: 0）；规则四个匹配键与实机 sysfs 只读逐一核对一致。
- 保存后 `list` 显示 registered: 1、状态 Drift；`check` 的 `access: ok` 是对真实 devnode 组/模式的核对，其余 warning（symlink、ModemManager 标记、termios/autosuspend）均为"规则未激活/阶段 3 未做"的正确门禁表现。`show`/`log` 完整，`shutdown` 正常退出并清理私有 socket。
- 全程未打开串口、未安装主机规则、未 `udevadm trigger`/`test`、未写 `authorized`/power、无固件操作。原始记录：`artifacts/linux-real-attach-2026-11-07.md`（不提交）。
- 仍待验收：真实桌面 Linux（WSL 外）的规则激活与热插拔、阶段 3 termios、enforce I/O 拒阻真机验证。`usbipd detach --busid 2-3` 可把 COM4 还给 Windows。

### 真机热插拔事件流验收（2026-11-07 续）

- WSL 重启后复测：沙箱配置跨重启保留，重启后的 daemon 对遗留 socket（上轮 daemon 被 WSL 硬杀）按设计拒绝启动；双重确认无监听后手动清理，`list` 显示设备经 **serial 绑定自动重新登记**（registered: 1），未依赖物理路径。
- Windows 侧 BUSID 因重新枚举从 `2-3` 变为 `1-3`，串行号绑定不受影响——与"绑定用序列号而非端口"的设计一致。
- 真实 udev 事件闭环：`usbipd detach` → watch 收到 `device_removed`（seq 1，按登记设备名报告）；`usbipd attach` → `device_added`（seq 2，完整 facts 含复合接口标志、`managed_by`、`binding=serial`、三条预期 note）；复插后 `list` 正常。事件连续、版本化，journal 带 `source=udev_monitor` 与 udev_seq。
- `shutdown` 正常退出，journal `service_stopping` 记录 `blocked_devices: []`。全程只读。原始记录：`artifacts/linux-real-hotplug-2026-11-07.md`（不提交）。
- 说明：这是 WSL 内 usbipd detach/attach 产生的**真实 udev 事件**，但仍不等同桌面 Linux 物理插拔验收。

### 阶段 3 termios/check/run 软件实现（2026-11-08）

- 新增 `devguard-core/src/termios.rs`（仅 Linux）：`apply` 打开设备（`O_RDWR|O_NOCTTY|O_NONBLOCK`）→ `TCSANOW` 写线路参数 → 回读 → **与配置比对，不一致即失败**，调用方拿不到未验证的成功；`read` 只读打开做检查。`BaudRate::NotApplicable`（CDC-ACM）不写也不比对线路速度。
- 新增 `devguard-core/src/runtime_env.rs`：`/run/devguard/<name>.env`（沙箱在私有树内）的共享模型，daemon 写、CLI 读；未知键拒绝，避免 daemon/CLI 静默不一致。
- `LinuxIo::apply` 启用（阶段 2 是明确拒绝的桩）：termios 设置与回读校验 → 写 runtime env 文件 → `AppliedSetting` 逐字段报告；autosuspend 与 symlink/权限仍归各自阶段门禁，apply 的 warnings 如实说明。
- `LinuxIo::check` 的 `live_apply` 硬编码告警替换为真实 termios 比对项 + 独立的 `autosuspend` 待验收项；打不开设备（权限/非 tty）报 warning 而非伪通过。
- `devguardctl run <name> -- <cmd>`（仅 Linux）：读 env → 设置并校验 termios → 清 `O_NONBLOCK` → 设备 fd `dup2` 到 0/1/2 → `exec`；进程被替换，子进程以设备为 stdio。错误路径（env 缺失、设备消失、回读不一致）都在 exec 前中止。
- server 派发表移除 `Apply`（Pause/Resume/PolicySet/Block/Allow 仍留阶段 5）；RPC 回归测试确认 apply 走决策核心而 pause 仍 `unsupported`。
- **实测 pty 限制（Ubuntu 26.04 / 6.6 内核）**：pty 驱动强制 CSIZE=CS8，数据位 5-7 无法在 pty 上回读验证；baud、parity（PARENB）、stop_bits、flow（含 CRTSCTS）、raw 均可往返。数据位验证归入真机验收。测试 `a_pty_never_claims_character_sizes_it_cannot_hold` 锁定该诚实行为。
- 测试结果：WSL **116 项通过**（core 34+5+10、CLI 7+2 pty、daemon 8+16+5+4+5+11 RPC+9 pty termios），两端 `clippy --all-targets -- -D warnings` 通过；Windows 侧 core+CLI 全绿。
- **真机 termios apply 门禁待工程师确认**：对 `/dev/ttyACM0` 执行 apply 会打开设备（DTR 可能置位）并写 CDC 线路参数（不碰固件、不写 authorized）。确认前实机 apply 不执行。

### 阶段 3 真机 termios 验收（2026-11-08，已通过）

- 工程师确认后对转接设备（`V6X-RUST-001`，WSL 重启后重新 attach）执行完整闭环：
  - **apply**：115200 8N1 raw 写入并回读验证，CDC-ACM 接受 SET_LINE_CODING；env 文件生成、`AppliedSetting` 逐字段报告、warnings 如实。
  - **偏差闭环**（需求 §10）：外部 `stty` 注入 7O2+cooked → `check` 检出 `parity=o stop_bits=2 flow=software raw=0` → `apply` 修复 → `check` 恢复 ok。
  - **run 真机端到端**：子进程 stdin 为设备 fd，`stty -a` 读回 `speed 115200`；journal 两次 `applied` 审计完整。
- **实测**：`cs7` 写入后读回仍 CS8 —— cdc_acm（vhci 链路）不保持 CSIZE 5-7；`apply` 的回读契约会在此类配置上诚实失败。与 pty 的 CS8 强制一并记录于 design.md。
- 全程未写固件、未写 `authorized`/power、未装主机规则。原始记录：`artifacts/linux-real-termios-2026-11-08.md`（不提交）。

- 工程师取消固件刷写，要求继续 `USBDeviceGuard`；本轮没有串口打开、固件写入、USB 转接/反授权、系统规则安装或服务配置变更。
- 当前主机是 Windows `LMX`，仓库位于 `C:\Users\leeli\Downloads\USBDeviceGuard`。开始时 `main` 工作树干净，HEAD 为 `5d4dd01`。
- 已确认 WSL2：`Ubuntu-26.04`（默认）及 `Ubuntu-24.04`。本轮使用前者，用户 `dev` / uid 1000，PID 1 为 `systemd`；Rust 1.99.0，udevadm 259，cargo/rustc/udevadm/systemctl/pkcheck/python3 可用。
- Windows 只读枚举：`PX4 FMU v6X.x` / `COM3` / `3185:0035`，PnP 状态 OK、ProblemCode 0。它不是历史记录中的 `V6X Rust CDC` / COM26；没有因此修改固件或假定 bootloader 身份。
- 构建缓存分别使用 Windows `target` 和 Linux `/home/dev/.cache/devguard-target-lmx`，不混用。
- 原始基线 `cargo test -p devguard-core -p devguardd --locked`：Windows 49 项通过。本轮最终 Windows 的 core/daemon/CLI 测试 **63 项通过**，这三个 crate 的 `cargo clippy --all-targets -- -D warnings` 通过。
- Linux `cargo test -p devguard-core -p devguardd -p devguardctl --locked`：**82 项通过** = core 单元 28 + core 集成 10 + Unix client 5 + CLI 4 + daemon 规则 8 + 决策 14 + Inventory 4 + socket RPC 9。
- Linux 真实 socket 测试已覆盖保存/查询/删除、身份冲突、内核身份优先于客户端伪造、拒绝审计、事件顺序/停止 EOF、枚举/规则失败、沙箱目录保护、超大响应和重启后审计回读。启动日志明确区分 daemon 身份与未知的启动发起人。
- WSL 工具链原缺 clippy/rustfmt，已补充用户工具链组件；Linux clippy 的队列枚举尺寸和测试字面量告警已修正，**最终 `-D warnings` 通过**。
- 接入前修正：serial 匹配失败不得退回 path；只接受有 USB 证据及数字后缀的 tty；删除/查询名称拒绝路径穿越；配置 symlink 与损坏配置不能被静默读取或忽略；规则限定 ttyACM/ttyUSB、保留显式组并拒绝危险值；响应编号和 check 偏差事件正确返回。
- 新增复合 USB 安全判定：解析 `ID_USB_INTERFACES` 得到接口类，`UdevFacts` 携带 `interface_unknown` / `forbidden_interface_class`，doctor 逐设备报告 HID/hub/storage/video/audio 接口或缺失接口列表。只读、不越界；enforce 前的 I/O 拒阻仍待真机验证。
- 新增 `devguardd/src/linux/` 和 Unix 客户端/CLI。仅提供 audit 与配置链路；硬件操作明确拒绝，详细边界及运行方法见 [`linux-shell.md`](linux-shell.md)。
- 工程师确认三个测试边界：Unix socket 的请求/鉴权/审计/事件、只读 udev/sysfs Inventory、Client/CLI 协议与错误处理。
- 实际 CLI 验证发现需求示例 `parity = "N"` 被仅接受小写的枚举拒绝。已用 CLI 文件解析最小回归复现，并在共享 `Parity` 增加 `N/E/O` 反序列化别名；保留原小写输出与兼容性。六种大小写输入的回归、原始 CLI 验证及全套测试均通过。
- `scripts/verify-linux-shell.py --build`：在非 root 私有树启动实际 Linux daemon/CLI，list/save/show/log/doctor/delete/shutdown 等 9 次 CLI 操作及一次私有规则 `udevadm verify` 全部 exit 0，daemon 正常退出并清理自己的 socket。原始产物为本机 `artifacts/linux-cli-2026-11-07.json`（不提交）。
- 该验证只读枚举到 **0 个 Linux USB 串口**；没有打开串口、reload 主机规则或改写 USB authorized。`udevadm verify` 仅验证语法，**不是 `udevadm test`、真机插拔、桌面 polkit 或 termios 验收**。
- 在 WSL 项目目录安装/核对 38 个 Matt Pocock skills；`.agents/skills/` 文件齐全，原有 NSH 两项保留。`software-factory` 因上游 YAML 错误跳过；未做全局安装。

## 本次开发记录（阶段 2，进行中）

- 设计决策：`devguardd` 拆为两层——**纯决策核心**（`src/core.rs` + `src/rules.rs`，任何平台可编译可测试）与 **Linux I/O 壳**（socket、udevadm、sysfs、termios、pkcheck，只编译于 Linux）。核心通过 `src/io.rs` 的 `DeviceIo` trait 与设备 I/O 解耦，测试用 FakeIo 驱动完整状态机。
- 纯核心已实现并测试（16 项，`cargo test -p devguardd` 全绿，workspace clippy 干净）：
  - 授权门控：只读请求需 devguard 组或 root；修改/暂停需 polkit 正向决定（root 隐式通过）；拒绝写入 `request_denied` 审计。
  - udev 规则生成：白名单转义（`"`、`%`、`$`、`\` 不可注入）；app 与 bootloader 双 id 共享同一 SYMLINK；无安全绑定值的 id 记入 skipped 而非生成宽松规则。
  - enforce：未登记串口设备按 **USB 路径**（非 tty 编号）键控拦截；复合设备父层含非串口功能时 `set_authorized` 拒绝 → 保持放行并写 `block_refused` 审计，绝不盲目阻断。
  - 暂停：enforcement 暂停释放已拦截设备（键控与拦截一致，tty 改号不丢放行标记）；暂停期间策略切换 busy 拒绝；到期/手动恢复返回 GUI 需提示的设备清单；暂停上限由核心强制（0/61 拒绝）。
  - learn_candidate：按 VID/驱动/序列号猜测 kind 与 role（26ac→PX4，PID 0011→bootloader），无序列号自动落到 ID_PATH 并告警"绑定物理口"。
  - save/delete：ConfigChanged 事件 + 前后 TOML 入 journal + WriteRules 效果；service 暂停时 apply busy 拒绝。
  - check：偏差产生 DriftDetected 事件；多设备匹配同一配置记为 error（身份冲突）。
  - shutdown：ServiceStopping 事件 + 操作者审计 + Shutdown 效果（壳负责按选项释放或保持拦截后 exit 0）。
- 修正的语义（由测试逼出）：暂停放行的键必须是 USB 路径而非 devnode，否则 tty 改号后放行失效；`on_device_facts` 需维护在线快照供恢复清单使用。
- 待办（本阶段后续）：Linux 壳（Unix socket + SO_PEERCRED、udevadm monitor/info 解析、termios 设置与回读、sysfs authorized/power、pkcheck 调用、devguard-apply@.service），`udevadm test` 验证规则，真机插拔验收。

- 用户指示跳过阶段 0 的 Linux 验收，继续后续阶段（偏离已记录于"待验收"）。
- 新增 `devguard-core/tests/integration.rs`，7 项集成测试全部通过：
  - 存储：空库加载、保存→文件可读、编辑自动备份（旧内容可从备份恢复）、跨设备身份冲突在写盘前拒绝、同名保存为"编辑替换"语义、物理口绑定（ID_PATH）与序列号绑定共存、删除备份并移除文件、非法/穿越名称与缺绑定拒绝。
  - Journal：追加→按序查询、操作者与前后值完整、`pause_*` 前缀过滤、按设备过滤、limit 保最新、导出为合法 JSONL、损坏半行不丢失其余记录（`skipped` 计数）。
  - 策略：缺文件默认 audit、roundtrip 为 `mode = "enforce"`、暂停 0/超上限拒绝、5 分钟剩余秒数正确。
  - 协议：版本不符拒绝、缺 op 拒绝、shutdown 选项完整往返、只读/需 polkit 分类、响应 ok/error 分离且 next_step 可操作、事件版本化、操作者用户名控制字符净化。
- `cargo clippy -p devguard-core --all-targets -- -D warnings` 通过。
- 结论：阶段 1 在本机可验证范围内完成；与真实 udev/文件系统权限相关的验收（`udevadm test`、真机插拔）属阶段 2 门禁。

- 用户补充“界面要优化，便捷”，已转为 [`ui-design.md`](ui-design.md) 的具体布局、快捷键和交互验收项。
- 用户随后要求：英文界面、同时提供 Linux 与 Windows 版本、显示 USB 设备列表、弹出窗口演示。
- 界面已全部切换为英文文案（含按钮、提示、对话框、剪贴板导出、窗口标题），中文仅保留在需求文档中；新增 `devguard-gui/src/copy.rs` 承担共享文案与单复数，含单元测试。
- 新增 `devguard-gui/src/hardware/`：只读 USB 串口设备清单。Windows 上通过 `-EncodedCommand` 的 PowerShell 子进程读取 PnP/WMI 并以 JSON-lines 输出，WMI `Win32_DeviceChangeEvent` 驱动自动刷新（无轮询定时器），子进程随通道关闭一并终止；Linux 上明确提示“USB 管理需 devguardd”，不伪造设备。
- 修正 `nsh_core::background()` 在 engine 线程的 panic（TSX headless 无 Tokio reactor）：设备扫描全部改为 OS 线程 + 有界通道，engine 侧只 `recv()`（详见 [`design.md`](design.md)）。
- 新增 `devguard-gui/src/pages/devices.rs` 与顶部 `Devices / Records / Validation guide` 三个页签，Devices 为默认页：搜索、刷新、状态徽章、复合 USB 父设备提示、详情面板与复制导出。
- `DEVGUARD_GUI_NO_HW_SCAN=1` 供 headless 测试禁用真实设备扫描；原生运行时自动扫描。
- 本机实测设备：`V6X Rust CDC`，`COM26`，`VID:PID 1209:0001`，父设备 `USB\VID_1209&PID_0001\V6X-RUST-001`，证据与复现步骤见 [`hardware/windows-2026-11-06.md`](hardware/windows-2026-11-06.md) 与 `scripts/inspect-usb-windows.ps1`。
- 测试：`cargo test -p devguard-core` 26 通过；`nsh test`（校验副本）17 通过 = core/GUI 单元 6 + Rust 交互 10 + TSX headless 3（含真实 USB 扫描启动）；`cargo clippy -p devguard-gui --all-targets -- -D warnings` 通过。
- 校验副本改用独立 `CARGO_TARGET_DIR`，修复了两套源码路径共用 target 目录导致的构建缓存损坏。
- 英文界面下 `spike_flows` 全部重写并通过；原生窗口已在用户桌面打开，Devices 页签默认展示。

### 部署级验收：system 模式规则激活 + 真实插拔（2026-11-08，已通过）

- 工程师确认后，daemon 首次以 **system 模式**（root，`/etc`/`/run`/`/var/lib`）运行于 WSL：`devguard` 组创建、socket `root:devguard 0660`、`save` 触发 daemon 写真实 `/etc/udev/rules.d/90-devguard.rules` 并 reload udev。
- `udevadm test` 命中规则：GROUP=dialout、MODE=0660、SYMLINK 已添加、`ID_MM_DEVICE_IGNORE=1`、`SYSTEMD_WANTS=devguard-apply@ttyACM0.service`（unit 为阶段 4 交付）。
- **真实插拔后**：`/dev/devguard/rust-fmuv6x-ttyacm0` symlink 由 add 事件创建、devnode `root:dialout 0660`、MM 标记置位；daemon `check` 的 symlink/access/modem_manager 全 ok；termios 如实检出插拔重置（9600 cooked）并由 apply 修复。
- 插拔循环事件流正常（removed/added，notes 仅剩 termios/autosuspend）；journal 完整（service_started 无 uid、root 操作带 uid=0）；shutdown 清理 socket、保留 env 文件。
- 全程未写固件、未写 authorized/power。原始记录：`artifacts/linux-system-mode-2026-11-08.md`（不提交）。物理插拔的桌面特有部分（Wayland 桌面通知、polkit agent）归阶段 4/6 桌面验收。

## 待验收

- Linux（Arch/Omarchy，GTK 4.14+）的 `--headless` / `--native` 验收仍未执行；脚本为 `scripts/verify-nsh-linux.sh`。
- 阶段 0 的正式验收（Wayland 窗口、IME、关闭行为）以 Linux 运行为准；Windows 结果只作开发证据。
- 用户于 2026-11-06 明示"先不用测 linux""继续后面的阶段"：阶段 0 的 Linux 门禁由用户决定暂缓，阶段 1 继续；此偏离记录在案，Linux 验收不因推进而免除。
