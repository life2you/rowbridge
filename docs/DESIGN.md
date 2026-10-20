# RowBridge — 设计方案 v0.1

> 一个 macOS 端的 MySQL 可视化同步工具（开源）。
> 首版目标：选中 A 库的若干表，按**单列整型主键分批**把数据搬到 B 库同名表；目标表不存在则自动建表。
>
> 名字是占位，备选：RowBridge / PoolSync / MyMove。改名字只需改目录和 bundle id。

---

## 1. 范围定义

### 1.1 首版做

| # | 能力 | 说明 |
|---|------|------|
| 1 | 连接管理 | 多组源/目标连接，密码存 macOS 钥匙串 |
| 2 | 库表浏览 | 列出库 → 表 → 列/索引/行数估算/主键 |
| 3 | 结构预检 | 主键是否合格、目标表是否存在、列差异、跨版本兼容性 |
| 4 | 自动建表 | `SHOW CREATE TABLE` → 改写 → 在目标库执行 |
| 5 | 分批同步 | `WHERE pk > ? ORDER BY pk LIMIT n`，逐批 UPSERT |
| 6 | 实时进度 | 行/秒、已同步行数、预估剩余、逐批日志 |
| 7 | 可控运行 | 暂停 / 取消 / 断点续跑（用目标表 MAX(pk) 推导） |
| 8 | 结果报告 | 每表成功/失败/行数/耗时，可导出日志 |

### 1.2 首版不做（明确写进 README，避免预期错位）

- **不做 DELETE 同步**：目标端多出来的行保留。
- **不做 DDL 变更同步**：目标表已存在时只补数据，不 `ALTER`。
- **不做无主键表 / 复合主键表 / 非整型主键表**（界面上置灰并给出原因）。
- **不做持续增量 / CDC**（不读 binlog），是一次性任务。
- **不做表名映射**（同名进同名出）；v1.1 再加。
- 不支持 MySQL 之外的库（Postgres 等）。

---

## 2. 技术选型

| 层 | 选择 | 理由 |
|----|------|------|
| 外壳 | **Tauri 2** | dmg ~15MB，内存占用低，Rust 侧直接持连接跑重活 |
| 后端 | **Rust + `mysql_async`** | 见 §2.1，能拿到原始 `Value` 无损搬运 |
| 前端 | **React 19 + Vite + Tailwind + shadcn/ui** | 生态成熟，组件不用自己写 |
| 状态 | TanStack Query（服务端状态）+ zustand（向导状态） | 进度靠事件推，不靠轮询 |
| 本地存储 | SQLite（Rust 侧 sqlx） | 存连接元数据、任务历史、日志 |
| 密钥 | `keyring` crate → macOS Keychain | 不把密码写进任何文件 |

### 2.1 为什么是 `mysql_async` 而不是 `sqlx`

这个工具的核心是**把任意列的值原样搬过去**，不做业务解码。两条路的差别很大：

- **sqlx**：`Row` 是强类型的，取列值得先知道类型 → 要么先查 `information_schema` 建类型映射表，要么 `try_get_raw()` 再 `MySqlValueRef` 重新绑定。中间多一层"类型推断"，遇到 `DECIMAL` 精度、`0000-00-00` 零日期、`BIT`、`GEOMETRY`、`JSON` 都容易踩坑。
- **mysql_async**：`Row::unwrap() -> Vec<Value>`，`Value` 是个具体枚举（`NULL` / `Bytes` / `Int` / `UInt` / `Float` / `Double` / `Date` / `Time`）。读出来直接 `Params::Positional(Vec<Value>)` 塞回目标端，**零解码、零转义、零时区转换**。

选 mysql_async。附带收益：`query_iter()` 支持流式读，大表不必一次性吃进内存。

> 注意：Rust 侧用 rustls feature，避免 macOS 打包 OpenSSL 依赖。

### 2.2 前端 ↔ 后端通信

Tauri 2 的 `Channel<T>`，长任务在 Rust 侧 `tokio::spawn`，进度用 `channel.send(Event)` 推给前端；取消用 `tokio_util::sync::CancellationToken`。**不轮询、不做 HTTP server。**

---

## 3. 核心同步算法

### 3.1 你给的原型，以及必须补的两个东西

```
select xxx,xxx,xxx from a where id > xxxx
```

原型方向对，但直接这么写会漏数据，必须补 `ORDER BY` 和 `LIMIT`：

```sql
SELECT `c1`,`c2`,`c3` FROM `db_a`.`a`
WHERE `id` > ?            -- 游标
ORDER BY `id` ASC         -- ① 必须
LIMIT ?                   -- ② 必须
```

**为什么 `ORDER BY` 是强制的（不是可选优化）**：
我们下一批的游标取的是「本批最后一行的 id」。如果 MySQL 返回顺序不是按 id 递增的（优化器换了索引、用了覆盖索引扫描、并行读），最后一行可能不是本批最大的 id —— 那就有 id 落在中间的行被永久跳过，而且**静默跳过，不报错**。加上 `ORDER BY id` 才成立。

**代价几乎为零**：`WHERE id > ? ORDER BY id LIMIT n` 走的是主键索引的有序范围扫描，没有 filesort。这正是它比 `LIMIT offset, n` 好的地方 —— 后者要扫 `offset` 行再丢掉，深分页 O(offset)，你这个方案是 O(1) 定位。

**为什么不保证「不重复」**：同步期间源表若有并发写入/更新，同一行理论上可能被读到两次（新插入的小 id 不会，但事务可见性边界会造成边界行重复）。所以**写入端必须幂等** → UPSERT。重复读到的代价是一次多余的 UPDATE，可接受。

### 3.2 主循环

```
输入：源表 S，目标表 T，列集合 C，批大小 batch（自适应）
      cursor = start_id            // 默认 MIN(pk)-1，或目标表 MAX(pk)，或用户自定义
      stop_at = end_id or +∞

loop:
    rows ← SELECT C FROM S WHERE pk > cursor ORDER BY pk ASC LIMIT batch
    if rows is empty: break

    // 目标端写：一条多行 INSERT，一个事务
    BEGIN
      INSERT INTO T (C) VALUES (…×len(rows))
        ON DUPLICATE KEY UPDATE c1=VALUES(c1), c2=VALUES(c2), …   // 不含主键列
    COMMIT

    cursor ← rows.last[pk]         // 是本批最大 id，因为 ORDER BY 保证了
    emit Progress(done += len(rows), cursor, elapsed)

    if cursor >= stop_at: break
```

**每条不变式都要有测试兜底**：
- `cursor` 单调递增（有空洞、有负数、有 0 都不能坏）
- 目标表最终行数 ≥ 源表行数（在无删除的前提下应相等）
- 中途 kill 掉任务、重跑一次，结果与一次跑完完全一致（幂等性测试）

### 3.3 起始游标的三种来源

| 模式 | cursor 初值 | 场景 |
|------|------------|------|
| 全量 | `SELECT MIN(pk) FROM S` 减 1（无行则跳过） | 首次同步 |
| 续传 | `SELECT MAX(pk) FROM T` | 上次中断，从目标已有的位置往后补 |
| 自定义 | 用户填 | 修数据、补窗口 |

**续传不落盘**：目标表的 `MAX(pk)` 本身就是断点，不需要额外存任务进度表。这是 UPSERT 语义送的红利 —— 重跑安全，所以断点可以随时丢弃。
（唯一坑：目标端若有比源更大的 id，会把那些行当成"已完成"跳过去，所以预检要显示 `MAX(T) vs MAX(S)` 并让用户确认。）

### 3.4 自适应批大小

初始 1000 行。每批计时：

- 耗时 < 200ms → `batch = min(batch*2, 10000)`
- 耗时 > 2s 或拿到包过大错误 → `batch = max(batch/2, 100)`
- 出错（`1153`/`2006`）→ 减半重试，最多 3 次

同时受两个硬上限约束：

1. **占位符上限 65535**：`行数 × 列数 ≤ 65535`，宽表（100 列）时一批最多 655 行。
2. **`max_allowed_packet`**：连接时读 `SHOW VARIABLES`，按 60% 预算估算字节；单行超限（比如 5MB 的 BLOB）时降级为**单行 INSERT**。

### 3.5 一致性语义（说清楚，别让用户误会）

默认**每批独立读、不开长事务**。含义是：这不是一致性快照，同步期间的并发写入会造成个别行是新版本、个别行是旧版本。对「低峰期搬家 / 离线迁移」完全够用，而且不会把源库 undo log 撑爆。

v1.1 可选加「单事务一致性快照」模式（`START TRANSACTION WITH CONSISTENT SNAPSHOT`），代价是长事务 + undo 膨胀，界面上要红字警告。

---

## 4. 结构预检（这是产品的差异化，不是配菜）

点「开始」之前先跑一遍只读预检，把 90% 的失败提前暴露。预检结果直接渲染成可读的检查清单，红项禁止启动、黄项提示确认。

### 4.1 检查项

| 检查 | 方法 | 不通过时 |
|------|------|----------|
| 源表主键唯一且为单列 | `information_schema.STATISTICS`，`INDEX_NAME='PRIMARY'` 且 `Seq_in_index` 只有 1 | 置灰，提示"该表为复合主键" |
| 主键类型为整型 | `DATA_TYPE ∈ {tinyint,smallint,mediumint,int,bigint}` | 置灰（v1.1 支持字符主键） |
| 主键无 `unsigned` 溢出风险 | 源 `bigint unsigned` vs 目标 `bigint` | 报警 |
| 目标库存在 | `information_schema.SCHEMATA` | 可勾选自动建库 |
| 目标表存在 | `information_schema.TABLES` | 走自动建表（§4.2） |
| 列兼容 | 源列 ∖ 目标列（丢弃）、目标列 ∖ 源列（看是否 `NOT NULL` 且无 default） | 后者为红项 |
| 类型兼容 | 归一化后比对（`int(11)` vs `int`、`varchar` 长度收窄、`text` vs `varchar`） | 黄项 |
| 字符集/排序规则 | 源表 collation vs 目标库默认 collation | 黄项 |
| 外键依赖 | 源表 `REFERENCED_TABLE_NAME` 是否也在本次任务中且排在前面 | 黄项，提供「关闭外键检查」开关 |
| 行数对比 | `MAX(pk)` / 估算行数 源 vs 目标 | 信息展示 |

### 4.2 自动建表：`SHOW CREATE TABLE` 的改写流水线

拿到源表的 DDL 后按顺序改写，每一步都可单独开关并展示 diff 给用户看：

1. **表名替换** → 目标表名，反引号转义（`` ` `` → ` `` `）。
2. **删掉 `AUTO_INCREMENT=12345`** —— 保留会让目标表从源表当前值起跳，产生大段空洞；主键值本来就是我们显式写入的。
3. **可选删掉 `CONSTRAINT ... FOREIGN KEY ...`** —— 被引用的表可能还没建，或建表顺序不对。默认开。
4. **collation 降级映射**：`utf8mb4_0900_ai_ci`（MySQL 8.0）→ 目标若是 5.7 / MariaDB 则映射到 `utf8mb4_unicode_ci` / `utf8mb4_general_ci`。跨大版本时这是最容易翻车的一步。
5. **低版本不支持的语法**：8.0 的 `DEFAULT (expr)`、`CHECK` 约束、函数索引、`INVISIBLE` 列。检测到就打红字，让用户选择"仍然尝试"或"跳过该表"。
6. **`ROW_FORMAT` / `ENGINE`** 原样保留（用户选了就是想要）。

建表语句在 UI 里以只读 SQL 展示，用户可以复制、也可以手动改（进阶）；默认走自动。

### 4.3 建库

目标 schema 不存在时，`CREATE DATABASE IF NOT EXISTS` 并沿用源库的 `DEFAULT CHARACTER SET` / `COLLATE`。

---

## 5. 写入策略

### 5.1 UPSERT 的语法选择（8.0.20+ 会踩的坑）

```sql
-- 老语法，MySQL 8.0.20 起 deprecated，9.x 可能移除
INSERT INTO t (a,b) VALUES (1,2) ON DUPLICATE KEY UPDATE b=VALUES(b);

-- 新语法，8.0.19+
INSERT INTO t (a,b) VALUES (1,2) AS new ON DUPLICATE KEY UPDATE b=new.b;
```

启动时探测目标端版本，`>= 8.0.19` 用新语法，否则用 `VALUES()`。MariaDB 一律用 `VALUES()`（它没跟这个 deprecation）。

**`UPDATE` 子句里绝不能包含主键列**，否则会试图改主键。

### 5.2 每批一个事务

`BEGIN; INSERT ...; COMMIT;`。好处：单批失败不会污染已完成的进度，cursor 不前进即可重试。

不做整表单事务 —— 大表会把目标端 undo 打爆。

### 5.3 会话级优化开关（默认关，标"进阶"）

| 语句 | 作用 | 风险 |
|------|------|------|
| `SET SESSION foreign_key_checks=0` | 跳过外键校验，提速且解决建表顺序问题 | 可能写入孤儿行 |
| `SET SESSION sql_mode=''` | 避免严格模式拒绝历史脏数据（如 `0000-00-00`） | 会静默截断 |
| `SET SESSION unique_checks=0` | 提速 | **与 UPSERT 冲突，禁用** |

改服务端 `innodb_flush_log_at_trx_commit` 这类，只提示不代为执行。

---

## 6. 数据正确性

- **不比对、只搬**：值走 `Value` 原样传递，不做 `String` 中转，不做时区换算（`DATETIME` 就按字面搬，不经过 `chrono::DateTime<Tz>`）。
- 完成后可选的**校验**（v1.1）：分段 `SELECT COUNT(*), SUM(CRC32(CONCAT_WS('#',c1,c2,...)))` 双端对比，5 个分段抽样，比 `CHECKSUM TABLE` 更可控（后者会被页分裂影响）。
- 首版只做**行数对比**（`SELECT COUNT(*)` 可选开关，InnoDB 上是全索引扫描，大表会慢，默认不跑）。

---

## 7. 状态机

```
Idle → Preflighting → Ready → Running ⇄ Paused
                                  ↓
                          Completed / Failed / Cancelled
```

任务粒度：**一个任务 = N 张表串行**。每张表有自己的 cursor 和统计；一张表失败默认**继续下一张**（可切换为"遇错停止"），最后汇总。

Rust 侧数据结构：

```rust
struct TableTask { src: TableRef, dst: TableRef, columns: Vec<String>, pk: String, stats: TableStats }
struct TableStats { rows: u64, cursor: i128, started: Instant, bytes_est: u64, retries: u32 }
enum Event { TableStarted(..), BatchDone{ table, rows, cursor, rps }, TableDone(..), Failed{..}, AllDone(..) }
```

---

## 8. UI 信息架构

```
┌────────────────────────────────────────────────────────────────────────┐
│  RowBridge                                    [ 连接管理 ] [ 历史 ]    │
├──────────────┬─────────────────────────────────────────────────────────┤
│ 源连接        │  步骤 1/3  选择表                                        │
│ ▾ 127.0.0.1  │  ┌───────────────────────────────────────────────────┐  │
│   ▾ db_a     │  │ ☑ a   1,204,553 行  PK:id  ✓ 可同步                │  │
│     ☑ a      │  │ ☑ b      88,201 行  PK:id  ✓ 可同步                │  │
│     ☑ b      │  │ ☐ c         120 行  复合主键 ✗ 不支持               │  │
│     ☐ c      │  │ ☐ d      12,004 行  无主键  ✗ 不支持                │  │
│   ▸ db_b     │  └───────────────────────────────────────────────────┘  │
│              │                                                         │
│ 目标连接      │  步骤 2/3  目标                                        │
│ ▾ 10.0.0.8   │  库: [ db_a ▾ ]   ☑ 库不存在时自动建                   │
│   ▾ db_a     │  ☑ 目标表不存在时按源表结构自动建表                     │
│              │  ☑ 建表时去掉外键约束                                   │
│              │                                                         │
│              │  步骤 3/3  参数                                         │
│              │  写入方式 [ UPSERT（推荐）▾ ]                           │
│              │  起始游标 ( 全量 | 从目标 MAX(id) 续传 | 自定义 )       │
│              │  批大小   [ 自适应 ▾ ]   单批上限 [ 1000 ]              │
│              │  结束 id  [ 不限 ]                                      │
│              │                                                         │
│              │              [ 预检 ]   [ 开始同步 ]                    │
├──────────────┴─────────────────────────────────────────────────────────┤
│ 预检结果                                                                 │
│  ✓ db_a.a 主键 id (bigint unsigned) 合格                                │
│  ✗ db_a.c 复合主键 (id, ts)，首版不支持 → 已排除                         │
│  ! db_a.d 目标表存在但缺少列 note (NOT NULL, 无默认值) → 写入会失败      │
├────────────────────────────────────────────────────────────────────────┤
│ 运行中   db_a.a  ████████░░░░░░  82%  1,204,553 行  3,410 行/s  ETA 1m  │
│  10:02:11  batch 1204  cursor=842133  rows=1000  412ms                 │
│  10:02:11  batch 1205  cursor=843133  rows=1000  388ms                 │
│                                                  [ 暂停 ] [ 取消 ]      │
└────────────────────────────────────────────────────────────────────────┘
```

设计取舍：
- **三步向导**而不是一屏塞满 —— 源/目标/参数依赖关系强，分步能提前拦住非法组合。
- **预检是独立可点按钮**，不和"开始"绑死 —— 用户可以反复预检调参，不用真的跑。
- 日志区默认只显示每批一行，可展开成完整 SQL（脱敏后）。
- 大表列表用虚拟滚动，避免 2000 张表卡死。

---

## 9. 本地数据模型（SQLite）

```sql
CREATE TABLE connections (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, host TEXT, port INTEGER,
  username TEXT, use_tls INTEGER DEFAULT 0, keychain_ref TEXT,  -- 密码不落库
  created_at INTEGER
);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY, name TEXT, src_conn TEXT, dst_conn TEXT,
  src_schema TEXT, dst_schema TEXT, options_json TEXT,
  status TEXT, started_at INTEGER, finished_at INTEGER
);

CREATE TABLE task_tables (
  task_id TEXT, table_name TEXT, status TEXT,
  rows_synced INTEGER, last_cursor TEXT, error TEXT,
  PRIMARY KEY (task_id, table_name)
);

CREATE TABLE task_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT, ts INTEGER, level TEXT, table_name TEXT, message TEXT
);
```

`task_tables` 只做**历史记录和展示**用；真正的续传断点来自目标表 `MAX(pk)`。

---

## 10. 错误处理矩阵

| MySQL errno | 含义 | 处理 |
|---|---|---|
| 2006 / 2013 | 连接断开 | 重建连接，重试当前批（最多 3 次） |
| 1153 | 包超过 `max_allowed_packet` | 批大小减半重试 |
| 1205 | 锁等待超时 | 退避重试 |
| 1213 | 死锁 | 退避重试（目标端有并发写时） |
| 1049 | 目标库不存在 | 触发建库流程 |
| 1146 | 表不存在 | 触发建表流程 |
| 1054 | 列不存在 | 停止该表，报列不兼容 |
| 1366 / 1406 | 字符集不兼容 / 数据过长 | 停止该表，提示加 `sql_mode=''` 或改 charset |
| 1452 | 外键约束失败 | 提示开启 `foreign_key_checks=0` |
| 1044 / 1142 | 权限不足 | 直接失败，提示所需最小权限 |

错误信息要**翻译成人话**并给出下一步动作，不能只把 errno 甩给用户。这是开源工具口碑的关键。

所需最小权限，写进 README：

```
源端：SELECT
目标端：SELECT, INSERT, UPDATE, CREATE   -- 需要自动建表时
        （+ ALTER 仅在 v1.1 结构同步时需要）
```

---

## 11. 项目结构

```
rowbridge/
├── src/                      # React
│   ├── routes/
│   │   ├── connections/
│   │   └── sync/             # 三步向导
│   ├── components/ui/        # shadcn
│   ├── lib/ipc.ts            # invoke / Channel 封装
│   └── stores/wizard.ts
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   ├── commands/         # test_connection / list_tables / preflight / run_task ...
│   │   ├── engine/
│   │   │   ├── copy.rs       # 主循环
│   │   │   ├── batch.rs      # 批大小自适应 + 多行 INSERT 拼装
│   │   │   └── ddl.rs        # SHOW CREATE TABLE 改写
│   │   ├── introspect.rs     # information_schema 查询
│   │   ├── state.rs          # AppState + CancellationToken
│   │   └── store.rs          # SQLite
│   ├── capabilities/
│   └── tauri.conf.json
├── docs/DESIGN.md
├── tests/e2e/                # docker compose 双实例 + 真跑
├── .github/workflows/release.yml
├── LICENSE                   # MIT
└── README.md
```

---

## 12. 验证计划（本机就能跑）

已有 `mysql:8.0` 在 3306。再起一个当"目标库"：

```yaml
# tests/e2e/docker-compose.yml
services:
  mysql-src: { image: mysql:8.0, ports: ["3307:3306"], environment: [...] }
  mysql-dst: { image: mysql:8.0, ports: ["3308:3306"], environment: [...] }
```

测试用例（必须覆盖的边界）：

1. 空表 → 目标建表成功，0 行
2. 主键有大量空洞（1, 100, 100000）→ 不漏行
3. 主键含 0 和负数 → 游标正确
4. `bigint unsigned` 接近 `u64::MAX` → 不溢出
5. 宽表 100 列 × 批 2000 → 触发占位符上限降级
6. 单行 10MB BLOB → 降级为单行 INSERT
7. 目标已存在部分数据 → 续传模式只补差额
8. 目标表已存在但少一列（NOT NULL 无默认）→ 预检报红，不启动
9. 跨版本：源 8.0 → 目标 5.7（`utf8mb4_0900_ai_ci` 降级）
10. 运行中 kill 进程 → 重跑结果一致（幂等）
11. 同步中断开目标连接 → 自动重连重试

---

## 13. 里程碑

| 阶段 | 内容 | 估时 |
|------|------|------|
| M0 | Tauri 2 骨架 + React/Tailwind + IPC 打通 | 1d |
| M1 | 连接管理（Keychain）+ 库表树 + 列信息 | 1.5d |
| M2 | 预检引擎：主键判定、DDL 改写、列兼容报告 | 1.5d |
| M3 | 同步引擎：主循环、自适应批、UPSERT、取消、事件流 | 2d |
| M4 | 三步向导 UI + 实时进度 + 日志 + 报告 | 2d |
| M5 | e2e 测试 11 例 + dmg 打包 + README + CI | 1.5d |
| | **合计** | **~9.5 人日** |

M3 是风险所在，建议先用 CLI 形态把引擎跑通（`cargo run --bin sync -- --src ... --dst ...`），再套 UI。这样即使 UI 进度慢，引擎的正确性也有测试保障，也方便开源社区贡献。

---

## 14. 风险清单

| 风险 | 影响 | 缓解 |
|------|------|------|
| 跨版本 DDL 不兼容 | 建表失败 | 预检阶段检测并给降级建议，不静默 |
| 字符集/排序规则不一致 | 写入报 1366 | 预检比对 + 可选 `sql_mode=''` |
| 目标端有触发器 | 行数翻倍或报错 | 预检读 `information_schema.TRIGGERS` 并警告 |
| 大字段超包 | 任务中断 | 单行降级 + 减半重试 |
| 用户误把生产库当目标 | 数据被覆盖 | 目标连接首次使用时二次确认；非空表要求输入表名确认 |
| 未签名 dmg 被 Gatekeeper 拦 | 无法安装 | README 写明「右键 → 打开」，或做 ad-hoc 签名 |

---

## 15. 待你拍板的三件小事

1. **项目名**：`RowBridge` / 其它。
2. **多表任务是串行还是可并行**：串行简单、对源库友好；并行快但复杂度陡增。建议首版串行。
3. **是否需要「同实例跨库」同时支持**：技术上天然支持，只是 UI 上允许目标和源选同一个连接。建议支持（成本近零）。
