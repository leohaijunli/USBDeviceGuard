# SerialGuard 发布计划

更新：2026-11-08。开发门禁见 [`progress.md`](progress.md)，需求基线见 [`requirements.md`](requirements.md)。

## 命名决定

- **产品名：SerialGuard**（crates.io/GitHub/AUR 统一使用）。
- 原因：`devguard` 在 crates.io 已被占用（仓库卫生 CLI）；`serialguard` 发布前核验未被占用，且与 USBGuard 形成清晰的姊妹语义（管串口类 USB 设备）。
- **crate/二进制改名节点**：阶段 7 打包时一次性执行机械改名（`devguard-core → serialguard-core`、`devguardd → serialguardd`、`devguardctl → serialguardctl`、`devguard-gui → serialguard-gui`，含全部 `use` 路径与 `nsh.json`）。在此之前的开发提交保持现有 crate 名，避免中途大面积噪声；协议与配置文件格式不受改名影响（JSON-lines 协议自带版本号）。
- 仓库名：发布时 GitHub 仓库由 `USBDeviceGuard` 改为 `serialguard`（redirect 自动保留）。

## 定位声明（写进 README 首屏）

面向**工程师**的开源工具：管理 PX4 飞控、数传电台、FTDI/CP210x/CH340 串口适配器——稳定端口名（`/dev/devguard/<name>` → 改名后 `/dev/serialguard/<name>`）、正确权限、可校验的串口参数、ModemManager/brltty 抢占处理、限时暂停的 enforce。**不是** USBGuard 的替代：全 USB 设备白名单请用 USBGuard；本项目管 USBGuard 不管的那一半（串口层）。两者可共存但 enforce 会争写 `authorized`，阶段 5 文档将写明互斥说明。

## 渠道矩阵

| 渠道 | 内容 | 时机 |
| --- | --- | --- |
| GitHub 公开仓库 | 源码 + docs + issues；开发过程公开（验收证据链是可信度资产） | 现在可公开（完成下方卫生清单后） |
| crates.io | `serialguard-core`（lib）、`serialguardctl`（bin，`cargo install`） | 第一个 alpha（阶段 5 enforce 完成后） |
| GitHub Releases | `x86_64-linux-gnu` + `musl` 静态二进制（daemon/ctl） | 同上 |
| AUR | `serialguard` / `serialguard-bin` —— **主渠道**（目标桌面 Arch/Omarchy） | enforce 在 Arch 验收通过后 |
| .deb + PPA | Ubuntu/Debian/WSL | 阶段 7 顺手做 |
| copr (RPM) | Fedora | 之后再补 |
| snap | 不做：daemon 需要写宿主 udev 规则 + udev 触发的 systemd 模板单元 + fork/exec，全部在 strict 沙箱之外；classic 等于无沙箱收益 | —— |
| 社群公告 | PX4 Discord、discuss.px4.io、ArduPilot 论坛 | 有 GIF 演示 + 一键安装后 |

## 公开前卫生清单

1. **LICENSE**：仓库当前没有许可证文件，公开前必须加 `MIT OR Apache-2.0`（Rust 生态惯例；未引用 USBGuard 的 GPL 代码，独立实现不受传染）。
2. **个人信息脱敏**：`docs/progress.md` 等含主机名（`LMX`/`Artemis`）、内网路径（`\\Artemis\...`）；提交者邮箱会随公开暴露。清理或接受，二选一。
3. **状态声明**：README 首屏加 `⚠ alpha — audit mode only, NOT a security boundary yet (enforce ships in phase 5)`。
4. 文档语言：README/docs 英文为主（README 已是英文）；`docs/requirements.md` 保留中文原文（用户规格），加一行说明。
5. 最小 CI：ubuntu（test + clippy `-D warnings`）+ windows（test）两个 job。

## 里程碑对齐

- **alpha（阶段 5 完成）**：enforce + 暂停/恢复 + 服务控制可用；GitHub 公开 + Releases + crates.io。
- **beta（阶段 6 完成）**：正式 GUI 接真实 daemon；AUR 上架。
- **1.0（阶段 7 完成）**：打包（PKGBUILD/.deb）、`setup` 一键安装、文档齐全、总验收清单（requirements §10）全过。

## 发布件差异点（宣传与 README 的"为什么"）

USBGuard 完全不管的、本项目的核心卖点：
1. 串口参数管理：apply 设置并回读校验、check 漂移检测与修复、`run` 以正确线路参数 exec；
2. 稳定端口名与权限（`/dev/serialguard/<name>`、dialout 0660）；
3. ModemManager/brltty 抢占诊断（doctor）；
4. PX4 app+bootloader 双身份登记；
5. 限时暂停（5-60 分钟，到期恢复，暂停期间插入设备单独登记）;
6. 审计带修改前后 TOML 与操作者身份。
