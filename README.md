# 周期规则裁决器（Periodic Rule Arbiter）

纯本地运行的周期触发规则裁决服务。运维团队登记时区、起止闭区间、月内日期、周内
日期、节假日集合、排除窗与最小间隔后，系统在给定时间窗内生成**带解释**的完整触发
序列：每个 UTC 瞬间标明由哪条包含规则产生（可多来源合并）、通过了哪些检查、为何被
接受或拒绝。

## 安装与运行

```bash
cargo build --locked
cargo test --all-targets
cargo run --locked -- --addr 127.0.0.1:5217
# 浏览器打开 http://127.0.0.1:5217 ，页面标题为“周期规则裁决器”
```

- 不访问网络、不依赖机器当前时区；时区数据库固定为 chrono-tz 0.9 自带的 IANA
  tzdata **2024a**（见 `src/tzutil.rs` 中 `tzdb_version()`，测试与游标均断言此版本）。
- 数据默认落盘到 `./.periodic-arbiter-data/`，可用 `--data-dir` 或环境变量
  `PERIODIC_ARBITER_DATA_DIR` 覆盖。

## 时间语义（DST）

包含规则只写本地墙钟（时:分:秒），解析为 UTC 瞬间时按规则版本携带的策略处理：

- 跳空（gap，本地时间不存在）
  - `skip`：该候选直接消失；
  - `shift_forward`：平移到跳空后的**下一合法瞬间**（如纽约 2024-03-10 02:30 → 03:00 EDT）。
- 回拨（fold，本地时间出现两次）
  - `early`：只取早侧（较大偏移，DST 仍生效的第一次）；
  - `late`：只取晚侧（较小偏移，回拨后的第二次）；
  - `both`：两次都作为独立瞬间进入裁决序列。

边界（起止）与排除窗在界面上用本地墙钟录入，提交前按同一策略解析为 UTC 瞬间；之后
**所有比较都按瞬间**完成：排除窗是闭区间 `[start_epoch, end_epoch]`（端点包含），最小
间隔比较相邻已接受瞬间的 epoch 秒差，不比较任何格式化字符串。

## 裁决与镜像

- 一个瞬间可由多条包含规则同时产生：只输出**一次**，但解释保留全部来源（稳定排序后
  去重）。
- 最小间隔采用固定起点语义：从闭区间起点升序贪心接受；因此**正向翻页与反向翻页在同
  一闭区间上互为镜像**（`tests/arbiter.rs::forward_backward_are_mirrors_over_closed_interval`
  以多种页大小断言）。反向查询内部对同一序列取后缀并逆序，不存在“反向贪心”偏差。

## 游标与会话

- 规则指纹 = SHA-256(规范 JSON(规则))，JSON 对象键排序后哈希，字段书写顺序不影响指纹。
- 游标是不透明 base64url，载荷绑定：游标版本、规则指纹、固定 tzdb 标识、**查询方向**、
  最后返回的瞬间 epoch。规则任意字段变更、方向不符、tzdb 不同或内容被篡改都会被拒绝
  （HTTP 409/400）。
- 会话落盘（规则快照 + 每次查询的方向、锚点、返回 epoch、下一页游标）。导出文件是带
  SHA-256 摘要的自包含信封；另一实例导入后校验摘要，重建得到**相同的 UTC 瞬间序列、
  相同裁决种类与解释顺序**，旧游标仍可继续翻页。摘要不匹配的导入直接拒绝。

## HTTP API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/` | 单页界面 |
| GET | `/api/health` | 健康检查与 tzdb 版本 |
| GET | `/api/timezones` | 全部 IANA 时区 |
| GET/POST | `/api/rules` | 列出 / 校验并保存规则 |
| POST | `/api/resolve-local` | 按 DST 策略解析本地墙钟为瞬间（供边界/排除窗） |
| POST | `/api/evaluate` | 计算完整闭区间裁决序列（不落盘） |
| POST | `/api/query` | 带游标的正/反向分页查询，并写入会话 |
| GET | `/api/sessions` | 已存会话 id |
| POST | `/api/export` | 导出会话信封（JSON 下载） |
| POST | `/api/import` | 校验并导入会话信封 |

## 测试覆盖

`cargo test --all-targets` 共 20 个集成测试，覆盖：春令跳空（北美/南半球）、秋令回拨
三种策略、回拨合并、月末负数日期、闰日、闭区间端点、排除窗端点包含、最小间隔按瞬间、
多来源合并、指纹稳定性/敏感性、游标规则变更与方向失效、导入摘要篡改拒绝、正反向镜像
（多页大小）、跨实例导出导入逐字一致、以及在 `TZ=Pacific/Kiritimati` 下结果不变。

## 目录结构

- `src/model.rs`：规则版本、DST 策略、包含规则、排除窗等可序列化模型
- `src/tzutil.rs`：时区解析与 gap/fold 本地时间解析
- `src/engine.rs`：候选生成、来源合并、排除窗与最小间隔裁决、正反向分页
- `src/fingerprint.rs`：规则指纹 SHA-256 与绑定方向的游标编解码
- `src/storage.rs`：本地落盘、会话导出信封与导入校验
- `src/api.rs`、`src/server.rs`、`src/main.rs`：HTTP API、tiny_http 服务与 CLI
- `src/static/`：内嵌进二进制的单页界面
- `tests/arbiter.rs`：集成测试
