# 阶段 0：NSH 可行性验证

状态：**进行中；Linux/Wayland 验收未通过（尚无目标会话）**。

## 环境证据

- 当前工具执行环境：Windows，工作目录为 UNC 路径。
- Rust / Cargo：会话中已检测到 `1.99.0`。
- WSL：`wsl -l -v` 报告无已安装发行版。
- 已按 NSH 官方 Windows 安装流程安装 CLI：`nsh 0.4.0 (e626a8d)`。
- 安装位置：`%LOCALAPPDATA%\nsh\bin\nsh.exe`；当前工具进程 PATH 未自动刷新时使用绝对路径。
- 官方 Rust 模板已在临时目录生成并检查；当前 GUI 保留其纯 Rust root/module/native platform 入口形式。

## 文档与版本

- 官方仓库：<https://github.com/longbridge/nsh>
- 安装：<https://nsh.dev/docs/installation.md>
- Rust：<https://nsh.dev/docs/rust.md>
- 编码：<https://nsh.dev/docs/coding-guides.md>
- 设计：<https://nsh.dev/docs/design-guides.md>
- 组件：<https://nsh.dev/elements/list.md>、<https://nsh.dev/elements/dialog.md>、<https://nsh.dev/elements/input.md>
- 测试：<https://nsh.dev/docs/testing-and-trace.md>

已安装官方 `nsh` / `nsh-design-guides` skills 到 `.agents/skills/`，并读取 Rust、组件、测试、编码和设计指南。通过 `cargo metadata` 定位实际 `nsh-* 0.4.0` 源码，核对了按钮、输入、radio、tabs、dialog、virtual_list、后台任务、tokens 和平台入口签名。

## 本机已完成的基线验证

- `nsh check`：通过（编译与声明生成）；导出文档警告已补上 doc comment。
- `nsh test`（共享目录）：Rust crate 与 headless runner 构建成功；JS 测试入口打包失败：`UNRESOLVED_ENTRY //?/UNC/Artemis/.../.nsh/test/App.test.tsx.run.js`。这是 NSH 0.4.0 测试打包器的 UNC 路径限制，不能算测试通过。
- 采用 `scripts/verify-nsh.ps1` 复制源码到带所有权标记的本机临时目录校验。逐文件比较 SHA-256，并保存 `artifacts/source-sha256.json`，以保证验证的是当前工作区源码；不修改 NSH。
- 校验副本与主工作区（UNC 路径）不得共用 `CARGO_TARGET_DIR`：两套源码路径会互相污染构建指纹与链接参数（先后表现为 `quickjs-jit-sys` 的 `bindings.rs` 缺失和 `WebView2LoaderStatic` 找不到）。脚本已改为独立 target 目录 `devguard-target-mirror`。
- 本机 C 盘曾因两套完整 debug 构建缓存（合计 36 GB）耗尽空间，rustc 报 `IO failure on output stream: no space on device`。工作区 `[profile.dev] debug = 1`（仅行表）可显著缩小缓存；两套缓存均为可再生的临时产物，删除后重建。
- 界面为英文（用户要求）；`cargo test -p devguard-gui` 14 项通过（4 单元 + 10 交互），`cargo clippy -p devguard-gui --all-targets -- -D warnings` 通过。
- 新增 Devices 页签：只读本机 USB 串口清单（Windows 用 WMI 事件自动刷新；Linux 明确提示需要 devguardd）。`DEVGUARD_GUI_NO_HW_SCAN=1` 供 headless 测试禁用扫描。
- 本机实测设备：`V6X Rust CDC` / `COM26` / `VID:PID 1209:0001`，证据见 [`hardware/windows-2026-11-06.md`](hardware/windows-2026-11-06.md)。
- 用户已选择“先完成本机验证”；Linux/Wayland 的阶段门禁保持待验收。

## 验证矩阵

| 项目 | headless / 当前平台 | Linux GTK / Hyprland |
| --- | --- | --- |
| 列表、稳定 key、回收 | 通过（10 项交互测试，含 1000 行滚动与筛选） | 待验证 |
| 对话框、确认/取消 | 通过（可取消、不误改记录；Windows 原生 Escape 需 Cancel 兜底） | 待验证 |
| 表单输入 | 通过（空值拦截、草稿保留、多行备注） | 待验证（含中文输入） |
| 顶部状态横幅 | 通过（暂停倒计时、到期自动结束） | 待验证 |
| 后台线程/事件更新 signal | 通过（跨线程徽章与剪贴板证据，Rust + TSX 两侧） | 待验证 |
| 窗口、缩放、浅/深色、焦点 | 通过（headless 视口/字号/明暗切换；原生截图见 [`../docs/screenshots/`](../docs/screenshots/)） | 待验证 |
| 只读 USB 设备清单 | 通过（Windows 原生：COM26 / V6X Rust CDC / 1209:0001，复合设备提示与详情正确） | 待验证（Linux 由 devguardd 提供） |

## 原生 Windows 证据（2026-11-06）

- `nsh test nsh.json --target windows` 通过；7 张截图在 `docs/screenshots/`：明色设备列表、设备详情、明色记录页、暗色记录页、暂停对话框、暂停横幅、窄窗口详情。
- 原生发现：对话框内 input 获焦时 Escape 不关闭 modal（Cancel 可用），已记录于 [`design.md`](design.md)；WMI 事件订阅在该宿主不可用时按设计回退为"Refresh to rescan"，不静默失败。

## 门禁与下一步

先完成最小应用和必要的行为测试，再在实际 Arch/Omarchy 桌面运行。Windows/headless 通过只记录为相应结果，不能替代 GTK/Wayland 验收。

阶段 0 验收需要可用的 Linux 开发环境（本机终端或用户提供的远程运行方式）；真实 USB / systemd / polkit 项目在其对应阶段继续验收。
