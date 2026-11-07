# Linux daemon / CLI 开发链路

当前是**阶段 2 的部分实现，不是可部署的设备拦截器**。Windows GUI 保持只读诊断；没有刷写固件，也没有接管 Windows 的 USB 设备。

## 当前范围

- Linux daemon 提供真实 Unix socket / JSON-lines 请求：`list`、`show`、`learn_candidate`、`save_device`、`delete_device`、`check`、`doctor`、日志查询、服务状态和停止。
- 配置使用现有严格校验、备份与原子写；返回成功前持久化审计。规则更新失败会明确返回 `rules_failed`，说明**配置变更已保存**，不会谎报已应用到设备。
- 只读枚举 `/sys/class/tty` 的 `ttyACM<数字>` / `ttyUSB<数字>`，读取 udev 属性，并核对 sysfs 中实际的物理 USB 父设备。tty 的 `DEVPATH` 不能直接当作 USB 授权路径。
- 请求身份来自 `SO_PEERCRED`、`/proc/<pid>` 和本机账户查询。system 模式修改需 polkit；`--root` 模式仅允许该私有目录的同 uid 工程师操作自己的测试配置，这**不是 polkit 验收**。
- 启动记录仅描述 daemon 进程，单独保存 `daemon_uid` / `daemon_user`；发起人未知时不填写操作者 uid/user，不冒充 GUI 操作者。systemd 启动/自启动作的可信审计仍待阶段 4/5。
- 订阅提供配置、检查偏差、停止及只读设备增减/更新事件；`udevadm monitor` 就绪后建立初始快照，事件触发 Inventory 对账。跨重启对账及重连仍待实现；CLI `watch` 不轮询。
- `apply`、暂停/恢复、策略切换、USB 授权写入明确返回 `unsupported`。没有后台 sysfs/termios 写操作；已有 enforce 策略文件会阻止此阶段的 daemon 启动，不会被偷偷改写。

## WSL 非特权运行

在 WSL Ubuntu 26.04 中运行。构建缓存放在 Linux 文件系统，不能与 Windows 的 `target` 混用：

```sh
cd /mnt/c/Users/leeli/Downloads/USBDeviceGuard
export CARGO_TARGET_DIR="$HOME/.cache/devguard-target-lmx"
cargo run -p devguardd -- --root "$HOME/.cache/devguard-sandbox"
```

另一终端使用同一环境：

```sh
cargo run -p devguardctl -- --socket "$HOME/.cache/devguard-sandbox/run/devguard/ctl.sock" list
cargo run -p devguardctl -- --socket "$HOME/.cache/devguard-sandbox/run/devguard/ctl.sock" doctor
cargo run -p devguardctl -- --socket "$HOME/.cache/devguard-sandbox/run/devguard/ctl.sock" watch
cargo run -p devguardctl -- --socket "$HOME/.cache/devguard-sandbox/run/devguard/ctl.sock" shutdown --note "development session ended"
```

`--root` 下的规则仅写入私有树，**不 reload 主机 udev、不创建真实设备链接、不打开串口**。根目录必须属于当前工程师，拒绝 `..`、根目录 symlink、混合系统/私有路径和解析到 `/` 的别名。socket 模式为 `0660`；不会自动删除已有文件或另一个 daemon 的 socket。普通关闭只清理自己创建的同 inode socket，强制杀进程后的遗留 socket需工程师确认后处理。

WSL 里的 USB 列表可能为空：Windows 的 COM 口不会自动映射为 Linux tty。真实 USB 转接和插拔验收需要另行确认，不包含在本轮软件测试中。

## 已确认的测试边界

### 只读热插拔迭代（软件验证通过）

- 沿用 udev 输入、Unix socket 和 CLI 事件流的测试接口，不转接硬件、不发出 `udevadm trigger`。
- 先建立 `udevadm monitor --udev --property --subsystem-match=tty`，收到监听就绪标记后再建立设备快照。
- 只接受 ttyACM/ttyUSB 的有效事件；事件触发只读 Inventory 对账，不使用设备轮询定时器。
- 按设备快照差异发布事件，重复提示不重复发布；端口改号先移除旧节点再添加新节点。
- 监视器输出异常或退出必须进入审计并结束事件服务，不能继续宣称清单正常更新；停止时清理自己启动的监视进程。
- 通过 5 项流解析/进程清理与 5 项 socket 热插拔测试；覆盖重复提示、移除、端口改号、旧移除提示、启动快照和监视器 EOF。它验证当前快照变化，不声称已完成真实高速插拔不漏事件的验收。

工程师确认覆盖三个公共接口：

1. Unix socket：请求编号、内核身份、权限拒绝、配置/日志查询、事件与退出。
2. Inventory：只读 udev/sysfs 数据、物理 USB 父设备、路径与设备范围。
3. Client / CLI：参数、协议版本、编号、错误、帧长度和订阅。

Linux 测试使用真实临时目录和 Unix socket，USB 总线使用隔离 fixture。结果记录于 [`progress.md`](progress.md)，不能代替真实 USB、桌面 polkit、Arch/Wayland 或 termios 验收。

### 本轮验证结果（2026-11-07）

- WSL：92 项测试通过；Windows：63 项通过；core/daemon/CLI 两端 clippy `-D warnings` 通过。
- 实际 Linux 二进制：9 次 CLI 操作及私有规则 `udevadm verify` 通过；正常停止后私有 socket 清理正确。
- Linux 只读 USB 串口枚举为 0；没有 USB 转接、串口打开或主机规则安装。
- 需求示例的大写 `N/E/O` 已兼容，保留原小写序列化格式。

可重跑验证（Linux 用户，不要 sudo）：

```sh
export CARGO_TARGET_DIR="$HOME/.cache/devguard-target-lmx"
python3 scripts/verify-linux-shell.py --build --bin-dir "$CARGO_TARGET_DIR/debug"
```

本机原始结果位于 `artifacts/linux-cli-2026-11-07.json` 和本轮实际监视器运行结果 `artifacts/linux-monitor-cli-2026-11-07.json`，不提交运行产物。

### 真机只读转接验收（2026-11-07，已通过）

WSL 运行后执行此前已确认的 `usbipd attach --wsl --busid 2-3`（1209:0001 / COM4，`Shared` 状态沿用此前普通 bind，未用 `--force`）：转接成功，`cdc_acm` 绑定 `/dev/ttyACM0`。重建二进制（含复合接口判定）后，在 `--root` 沙箱内对实机完成只读验收：

- `list`：**在线 1 台**（此前 Linux 只读枚举为 0）；`doctor` 无问题；`learn` 给出 serial 绑定草案，`usb_path` 为 USB 父设备 `usb1/1-1`。
- `save` 生成私有规则；真实 `udevadm verify` 通过；规则匹配键（KERNEL/tty 模式、idVendor、idProduct、serial）与实机 sysfs 只读核对一致。
- `check` 的 `access: ok` 核对的是真实 devnode 组/模式；symlink/ModemManager/termios 三项 warning 与"规则未激活、阶段 3 未做"一致。
- `shutdown` 正常退出并清理私有 socket。全程未打开串口、未装主机规则、未 trigger/test、未写 authorized。

原始记录见 `artifacts/linux-real-attach-2026-11-07.md` 和热插拔续验 `artifacts/linux-real-hotplug-2026-11-07.md`（均不提交）。下一步：真实桌面 Linux（WSL 外）的规则激活与热插拔验收，再补 termios/apply；在门禁通过前继续禁用硬件写入。

## 项目级 skills

### 当前硬件接入准备

- 工程师已确认并完成 `usbipd-win 5.3.0 x64` 交互安装；官方 SHA256 和数字签名核对通过，安装 exit 0，无需重启。
- 设备为 BUSID `2-3`、`1209:0001`、Windows `COM4`（`V6X Rust CDC` / `V6X-RUST-001`）。此前已完成普通共享（`Shared`），**2026-11-07 已在 WSL `Ubuntu-26.04` 中完成确认的只读转接与验收**（见上文"真机只读转接验收"），未使用 `bind --force`。
- 工具提示未知 USB filter `hhdusbh64` 可能不兼容；不得自动使用 `bind --force`。若普通共享失败，保留错误并另行决定。
- 转接期间 Windows 无法使用 COM4；`usbipd detach --busid 2-3` 可恢复。首次只读枚举与插拔，不刷固件、不修改串口参数或 USB authorized。
- 首次共享可能弹 Windows 管理员确认；设备插拔目前只更新事件，不自动弹 USBDeviceGuard 通知窗。

已在 WSL 的当前项目目录执行：

```sh
npx skills@latest add mattpocock/skills -a opencode -y
```

38 个可识别 skills 安装到 `.agents/skills/`，未使用 `-g`；NSH skills 保留。源仓库 `software-factory` 的 YAML 不合法，安装器跳过该项。WSL 原先只有 Windows 的 npm/npx 路径，本次使用官方 SHA256 校验后的临时 Linux Node 26.13.0，未安装系统级 Node，也未修改全局 PATH。
