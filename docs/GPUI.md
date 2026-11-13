# GPUI 版界面（RowBridge Native）

`crates/rowbridge-gpui` 是 RowBridge 的 **原生界面** 参考实现：用 Rust + [GPUI](https://gpui.rs)（Zed 的 UI 框架）
配合 [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)（原 `gpui-component`）重写界面，
不再依赖 Node / Vite / WebView。

Tauri 版（`src/` + `src-tauri/`）源码保留，但**已经不再随发布构建**——`release.yml` / `ci.yml`
现在只出原生版的 `.app` 和 dmg。两者共用同一个引擎 `crates/rowbridge-core`。

## 为什么可以并存

```
crates/rowbridge-core   引擎 + 内省 + 预检 + 数据源配置（0 依赖 UI）
        ├── src-tauri          Tauri 外壳（React 界面，原有版本）
        ├── crates/rowbridge-core/src/bin/rowbridge_cli.rs   CLI
        └── crates/rowbridge-gpui    GPUI 原生界面（本目录）
```

三套界面调用的是同一份 `engine::run_task` / `preflight::run` / `engine::sync_schema`，
所以同步语义、断点续跑、批大小自适应、清空目标表、表结构同步行为完全一致。

## 运行

```bash
# 调试运行
cargo run -p rowbridge-gpui

# 发布版（更接近真实体验）
cargo run -p rowbridge-gpui --release
```

首次启动会读取 `~/.RowBridge/connections.json`，也就是 Tauri 版保存的同一批数据源，
两个界面可以共用，互不影响。

### 打成 .app（Dock 图标 / 名称正常）

`cargo run` 直接跑二进制时，Dock 里显示的是可执行文件的默认图标。要么在主线程里用
`NSApplication.setApplicationIconImage` 现设一张（`src/main.rs` 的 `install_dock_icon`，已验证），
要么直接打包成标准 .app：

```bash
./scripts/bundle-gpui.sh            # release 构建 → dist-gpui/RowBridge.app
./scripts/bundle-gpui.sh --debug    # 用现有 debug 二进制，秒出
./scripts/bundle-gpui.sh --install  # 顺带装到 /Applications/RowBridge.app
```

`.app` 与 Tauri 版共用 `src-tauri/icons/icon.icns`，`CFBundleExecutable` 就叫 `RowBridge`，
所以 Dock、Cmd-Tab、菜单栏显示的都是 RowBridge。

## 功能对齐

| 能力 | 状态 |
|---|---|
| 数据源列表 / 一键套用 / 保存 / 删除 | ✅ |
| 连接测试、库列表、表清单（主键 / 行数估算 / 不支持原因） | ✅ |
| 勾选表、全选 / 清空、表级「区间」覆盖（起止 ID）/ 目标表名 / 清空目标表 | ✅ |
| 全局游标（全量 / 续跑 / 自定义）、批大小、上限、并发、自适应 | ✅ |
| 高级选项（自动建表 / 建库、外键与自增剥离、collation 映射、sql_mode、失败继续） | ✅ |
| 预检（逐表可同步状态、预计行数、源/目标 MAX(pk)、阻塞原因） | ✅ |
| 表结构同步（建库 / 建表 / 补列） | ✅ |
| 实时进度（逐表进度条、行/秒、游标）+ 日志面板 + 取消 | ✅ |
| 历史任务记录 | ✅ SQLite（core::store，与旧 Tauri 表结构兼容，重启后恢复展示） |

## 代码结构

| 文件 | 职责 |
|---|---|
| `src/main.rs` | 初始化 GPUI、深色主题、Dock 图标、打开主窗口 |
| `src/model.rs` | 纯 UI 数据：连接面板、选项面板、表行、进度、`DbSide` / `CursorMode` / `Tri` |
| `src/app.rs` | `RowBridgeApp` 状态机与动作：连库、拉表、预检、结构同步、开始/取消同步、事件归并 |
| `src/ops.rs` | Tokio 运行时桥接：所有 MySQL 调用都在这里，结果用 oneshot 送回 UI 任务 |
| `src/views.rs` | 顶栏、状态栏、数据源页、日志页 |
| `src/views_pane.rs` | 三栏主体：连接编辑、库列表、表列表、批次选项、表级覆盖编辑、进度与预检 |

线程模型：GPUI 主线程只做渲染；所有 IO 在 `ops::rt()` 的 Tokio 多线程运行时里执行，
引擎事件通过 `mpsc` 流回 UI 任务，再 `cx.notify()` 重绘。

## 自检（可选）

带 `RB_GPUI_SMOKE=1` 启动会自动走一遍只读链路并打印结果，用于在没有 GUI 自动化时验证接线：

```bash
RB_GPUI_SMOKE=1 cargo run -p rowbridge-gpui
```

它会：套用第一个已保存数据源 → 加载库列表 → 选库（优先 `fbb`）→ 加载表清单 → 勾选第一张可同步表
→ 写入一次表级自定义区间 → 预检，并把 `planned_rows` / `source_max_pk` 等结果打到 stdout。
**全程只读，不会写入任何数据。**

## 后续可做

- 历史任务（把 `src-tauri/src/store.rs` 的 SQLite 记录迁到 core）
- 表详情面板（列 / 索引 / 触发器）
- 键盘快捷键：`⌘Enter` 预检 / 启动，`⌘.` 取消
- 表清单虚拟化（`VirtualList`），万级表更顺滑

### 自检：目标库跟随源库同名

`RB_GPUI_SMOKE_DST=1` 会跑一遍「目标库自动对齐」的只读自检并打印结果：

```bash
RB_GPUI_SMOKE_DST=1 cargo run -p rowbridge-gpui
```

依次验证：① 选好两侧数据源后目标库是否自动等于源库名；② 源库在目标端不存在时目标库
是否留空并记为待创建；③ 手动选不同名目标库时是否只告警不阻断；④ 再切源库后是否重新对齐。

`RB_GPUI_SMOKE_DST=3` 会停在第②步（目标库待创建、已勾选一张小表、区间 0..20），
方便人工点「加入任务并开始」验证创建确认框。
