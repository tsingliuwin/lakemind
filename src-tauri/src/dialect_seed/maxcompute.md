## MaxCompute (ODPS) 方言要点

下推到 MaxCompute 执行的 SQL（`maxcompute_pushdown_query` 的 `sql` 参数）必须遵循 ODPS 方言，常见陷阱如下：

### 1. 表名必须用远程全限定名 `project.table`
- FROM 必须写该表在 MaxCompute 远程的 `project.table` 全限定名，**不能**用本地表名（如 `dim_user_uc`）或错误前缀（如 `mc.dim_user_uc`）——本地表名在远程不存在。
- 正确全限定名通过 `describe_table`（查看「MaxCompute 远程全限定名」）或 `list_tables` 获取；sample_guard 拦截消息也会给出。
- 典型报错 `ODPS-0130131:[n,m] Table not found - table xxx cannot be resolved` → 表名未用正确 `project.table`。

### 2. ORDER BY 必须配 LIMIT
- MaxCompute 默认 `odps.sql.validate.orderby.limit=true`，`ORDER BY` 不带 `LIMIT` 会报错。
- 典型报错 `ODPS-0130071:[n,1] Semantic analysis exception - ORDER BY must be used with a LIMIT clause`。
- 规避：给 `ORDER BY` 配 `LIMIT n`（推荐；聚合下推通常只需 TopN）；确需全量排序时在 SQL 最前加 `set odps.sql.validate.orderby.limit=false;`。

### 3. 聚合 / 全量统计必须下推
- 本地采样 / 部分物化的 maxcompute 表聚合会被 sample_guard 拦截（数据不全致指标失真）。
- 任何 SUM / COUNT / AVG / GROUP BY 等全量聚合，改用 `maxcompute_pushdown_query` 下推，FROM 用 `project.table`。

### 4. 大结果或需复用 → 用 target_table 落盘
- 下推结果需被后续本地 SQL / 视图 / 图表复用时，传 `target_table`（建议 `t_` 前缀）落盘为本地表。
- 落盘列均为 VARCHAR；需数值计算时在本地用 `CAST`，并遵守数据纪律（取整用 `ROUND`，禁 `CAST(... AS BIGINT)` 截断小数）- 不要用 `create_table` 从远程表名建表（远程表名在本地不存在）。
