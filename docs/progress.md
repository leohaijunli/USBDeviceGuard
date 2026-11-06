# devguard 开发与验收进度

更新：2026-11-06。

## 当前门禁：阶段 0（进行中）

需求基线已写入 [`requirements.md`](requirements.md)。按照用户的阶段门禁，Linux NSH spike 验收通过后再进入阶段 1。

会话环境实测：

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
| 1 core + 配置 | 完成（本机可测部分） | 33 项测试通过（26 单元 + 7 集成），clippy 干净 |
| 2 daemon + udev | 决策核心完成；Linux 壳待写/待编译 | `devguardd` 纯核心 16 项测试通过（规则生成 4 + 决策 12），clippy 干净；`io.rs` 定义 DeviceIo 接缝，Linux socket/udev/termios/polkit 实现进行中，全部待 Linux 编译与真机验收 |
| 2 daemon + udev | 未开始 | 当前 devguardd 仅占位入口 |
| 3 termios/check/run | 未开始 | 需要 Linux pty 与真机 |
| 4 事件/polkit/systemd | 未开始 | 需要 Linux system bus、polkit agent、systemd |
| 5 策略/暂停/服务控制 | 未开始 | 需要可恢复的真实 USB 测试环境 |
| 6 正式 GUI | 未开始 | 当前 GUI 仅最小 NSH 骨架 |
| 7 打包/文档 | 未开始 | 文档随开发更新，系统安装文件按已验收功能交付 |

## 记录约定

- 将“已实现”“编译通过”“headless 测试通过”“Linux 真机验收通过”分开记录。
- 每个实测记录命令、环境、结果；失败保留原因和下一步。
- 不把待办项、空入口或模拟设备计为完成的产品功能。
- 新结果更新本文件与 [`nsh-spike.md`](nsh-spike.md)，设计冲突更新 [`design.md`](design.md)。

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

## 待验收

- Linux（Arch/Omarchy，GTK 4.14+）的 `--headless` / `--native` 验收仍未执行；脚本为 `scripts/verify-nsh-linux.sh`。
- 阶段 0 的正式验收（Wayland 窗口、IME、关闭行为）以 Linux 运行为准；Windows 结果只作开发证据。
- 用户于 2026-11-06 明示"先不用测 linux""继续后面的阶段"：阶段 0 的 Linux 门禁由用户决定暂缓，阶段 1 继续；此偏离记录在案，Linux 验收不因推进而免除。
