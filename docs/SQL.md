# RowBridge 全部 SQL 清单

> 本文档列出工具**实际执行**的所有 SQL，按功能模块分类。
> 变量约定：`?` = 参数化占位符；`{schema}` `{table}` `{pk}` = 运行时拼接（均经反引号转义）；`{cols}` = 列名列表。
> 参数一律走预处理语句，不拼字符串（`{cols}` 等标识符除外，均来自 information_schema，且做了反引号转义）。

---

## 一、连接与内省（introspect.rs）

### 1.1 列出所有库

```sql
SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME
FROM information_schema.SCHEMATA
WHERE SCHEMA_NAME NOT IN ('information_schema','performance_schema','mysql','sys')
ORDER BY SCHEMA_NAME
```

用途：左侧库下拉列表。排除系统库。

### 1.2 判断库是否存在

```sql
SELECT 1 FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?
```

### 1.3 列出库下所有表（列表快路径 · 查询 1/2）

```sql
SELECT TABLE_NAME, ENGINE, TABLE_ROWS, TABLE_TYPE
FROM information_schema.TABLES
WHERE TABLE_SCHEMA = ?
ORDER BY TABLE_NAME
```

用途：表清单 + 行数估算（TABLE_ROWS 在 InnoDB 是估算值）+ 是否视图。

### 1.4 全库主键批量查询（列表快路径 · 查询 2/2）

```sql
SELECT TABLE_NAME, COLUMN_NAME, SEQ_IN_INDEX, NON_UNIQUE
FROM information_schema.STATISTICS
WHERE TABLE_SCHEMA = ? AND INDEX_NAME = 'PRIMARY'
ORDER BY TABLE_NAME, SEQ_IN_INDEX
```

用途：一次拿全库所有表的主键。按表聚合后：
- 主键列 0 个 → 「无主键」置灰
- 主键列 ≥2 个 → 「复合主键」置灰
- 恰好 1 个 → 可勾选（整型校验留到勾选时）

### 1.5 判断表是否存在

```sql
SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
```

### 1.6 读取表的完整结构（勾选时异步校验 · validate_tables）

```sql
-- ① 列定义
SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT,
       EXTRA, CHARACTER_SET_NAME, COLLATION_NAME
FROM information_schema.COLUMNS
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
ORDER BY ORDINAL_POSITION
```

```sql
-- ② 索引（含主键）
SELECT INDEX_NAME, COLUMN_NAME, SEQ_IN_INDEX, NON_UNIQUE
FROM information_schema.STATISTICS
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
ORDER BY INDEX_NAME, SEQ_IN_INDEX
```

```sql
-- ③ 触发器（用于预检警告「目标表有触发器会改变写入结果」）
SELECT TRIGGER_NAME FROM information_schema.TRIGGERS
WHERE EVENT_OBJECT_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?
```

### 1.7 表的主键最大值（勾选时异步校验 · validate_tables）

```sql
SELECT MAX({pk}) FROM `{schema}`.`{table}`
```

用途：列表行尾显示「max N」，逐表游标做区间行数估算。

### 1.8 读取建表语句

```sql
SHOW CREATE TABLE `{schema}`.`{table}`
```

用途：目标表不存在时，取源表 DDL 用于自动建表。

### 1.9 服务端版本

```sql
SELECT VERSION()
```

用途：判断 8.0/5.7/MariaDB，决定 UPSERT 新老语法、collation 降级策略。

### 1.10 max_allowed_packet

```sql
SHOW VARIABLES LIKE 'max_allowed_packet'
```

用途：算包大小预算（60%），超出则自动缩批/单行降级。

### 1.11 单表行数估算（预检用）

```sql
SELECT TABLE_ROWS FROM information_schema.TABLES
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
```

---

## 二、同步主循环（engine.rs）

### 2.1 RTT 探测

```sql
SELECT 1
```

用途：连接建立后测一次往返延迟，决定初始批大小（低延迟 2000 起，30ms+ 直接 10000 起；用户手动设过批大小则完全尊重）。

### 2.2 分批读取（核心）

```sql
-- 全量（起始批，无 WHERE）
SELECT {cols} FROM `{schema}`.`{table}` ORDER BY `{pk}` ASC LIMIT ?

-- 游标推进后
SELECT {cols} FROM `{schema}`.`{table}`
WHERE `{pk}` > ? AND `{pk}` <= ?      -- 结束游标下推到 WHERE，避免最后一批超额写入
ORDER BY `{pk}` ASC LIMIT ?
```

参数顺序：`cursor_value`, `end_value`(如有), `batch_size`。

**为什么必须有 `ORDER BY pk`**：游标取「本批最后一行」的 id 作为下一批起点。若返回顺序不按 pk 递增，最后一行就不是本批最大值，中间的行会被静默跳过。`WHERE pk > ? ORDER BY pk LIMIT ?` 走主键有序范围扫描，无 filesort，性能远好于 `LIMIT offset, n` 深分页。

### 2.3 分批写入（三种模式，一条 SQL 一批）

```sql
-- ① UPSERT（默认，幂等，可重跑）
INSERT INTO `{schema}`.`{table}` ({cols}) VALUES (?,?...), (?,?...), ...
AS new ON DUPLICATE KEY UPDATE `c1` = new.`c1`, `c2` = new.`c2`, ...
-- 8.0.19+ 用 `AS new` / `new.col` 语法；5.7/MariaDB 用 VALUES(col) 老语法
-- UPDATE 子句不含主键列；表只有主键一列时用 `pk=pk` 制造 no-op

-- ② INSERT IGNORE（只补缺失行）
INSERT IGNORE INTO `{schema}`.`{table}` ({cols}) VALUES (?,?...), ...

-- ③ 纯 INSERT（遇重复即报错）
INSERT INTO `{schema}`.`{table}` ({cols}) VALUES (?,?...), ...
```

每条 INSERT 包含 **batch_size 组 VALUES**（一批一行只有一条 SQL），占位符总数受 65535 上限约束（宽表自动 `batch = 65535/列数`）。

### 2.4 读取目标表 MAX(pk)（断点续跑）

```sql
SELECT MAX(`{pk}`) FROM `{schema}`.`{table}`
```

用途：续传模式起点 = 目标表已有最大主键。UPSERT 幂等使「重跑安全」，目标表 MAX 本身就是断点，无需进度表。

### 2.5 读源库字符集（建库前）

```sql
SELECT DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME
FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?
```

用途：目标库不存在时，沿用源库字符集建库。

---

## 三、自动建库建表（engine.rs / ddl.rs）

### 3.1 建库

```sql
CREATE DATABASE IF NOT EXISTS `{schema}`
DEFAULT CHARACTER SET {charset} COLLATE {collation}
```

charset/collation 来自 §2.5 读到的源库配置。

### 3.2 建表

执行的是 `SHOW CREATE TABLE` 结果经 DDL 改写流水线后的语句，等价形式：

```sql
CREATE TABLE `{target_schema}`.`{target_table}` (
  ... -- 源表列定义原样保留（含生成列）
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci
```

改写规则（每步有 diff 展示）：
1. 表名替换为 `目标库.目标表`
2. 删除 `AUTO_INCREMENT=12345`（避免目标主键从源表当前值起跳留空洞；列级 AUTO_INCREMENT 保留）
3. 按开关剥离 `CONSTRAINT ... FOREIGN KEY ...`（被引用表可能还没建）
4. 跨版本 collation 降级：`utf8mb4_0900_ai_ci` → `utf8mb4_unicode_ci`（5.7/MariaDB 目标）
5. 低版本不支持的语法（CHECK/函数索引/DEFAULT(expr)/INVISIBLE）只警告不改

---

## 四、会话级参数（engine.rs · apply_session_opts）

仅在**目标连接**上执行，源端保持只读默认：

```sql
-- 开关开启时：
SET SESSION foreign_key_checks = 0
SET SESSION sql_mode = ''

-- 始终执行（大字段传输防超时）：
SET SESSION net_write_timeout = 600
SET SESSION net_read_timeout = 600
```

> 零日期（`0000-00-00`，错误 1292）：目标为 5.7 时可通过 `sql_mode=''` 放行（会话级，不影响服务器全局）。该开关当前未暴露到新 UI，如需默认开启见开发记录。

---

## 五、执行时序（一次典型同步）

```
连接源、连接目标
  ├─ SELECT 1                        （RTT 探测）
  ├─ SELECT VERSION()                （版本判断）
  ├─ SHOW VARIABLES max_allowed_packet
  ├─ SET SESSION net_write/read_timeout （目标）
  │
预检（可跳过，UI 勾选时已异步校验过大部分）
  ├─ §1.3 §1.4                       （快路径已在列表加载时做过）
  ├─ §1.6 ①②③ + §1.7               （仅当没缓存）
  ├─ SHOW CREATE TABLE               （目标表不存在时）
  │
同步每张表（并行 ≤ N）
  ├─ [建库] §3.1 + §2.5
  ├─ [建表] §3.2（SHOW CREATE TABLE 改写后执行）
  ├─ [续传] §2.4 读目标 MAX(pk)
  └─ loop：
       §2.2 读一批 → §2.3 写一批（一条 SQL）
       直到读满批数 < batch 或到达结束游标
```

---

## 六、SQL 安全性说明

| 类别 | 处理方式 |
|---|---|
| 值参数（游标、批大小、schema 名查询参数） | 预处理语句 `?` 占位，二进制协议传输，零注入风险 |
| 标识符（表名、列名、库名） | 反引号转义（`` ` `` → ` `` ` ``），来源为 information_schema 或用户选择，不直接拼原始输入 |
| 用户输入的 id（起始/结束游标） | 先按主键类型解析成 i64/u64 数字，不是字符串拼接 |
| DDL | `SHOW CREATE TABLE` 取自服务器，改写后执行；改写 diff 全程展示给用户 |

---

## 附录：已修复的 SQL 相关问题

| 问题 | 修复 |
|---|---|
| 1390 占位符过多（宽表） | 初始批先按 `65535/列数` 收敛；1390 加入自动缩批白名单 |
| 1292 零日期（5.7 严格模式） | 会话级 `sql_mode=''` 开关（见 §四） |
| 结束游标超额写入 | `pk <= ?` 下推到 SELECT 的 WHERE，不再写后比较 |
| 批大小被 RTT 探测覆盖 | 用户设过则完全尊重，仅默认值用 RTT 推荐 |
