---
type: Playbook
title: 转化分析准则
description: 转化漏斗、归因窗口、口径一致性等转化分析主题的通用陷阱。跨行业适用。
tags: [topic:conversion, error-type:attribution, error-type:truncation, error-type:fabrication]
timestamp: 2026-07-10T00:00:00Z
resource: lakemind://tenets/topic/conversion
---

# 转化分析准则

转化分析（漏斗、转化率、归因）是数据出错的重灾区，因为每一步都涉及"口径"——稍有偏差，数字就张冠李戴。

## 漏斗各环节口径必须一致

漏斗的每一环（如"曝光→点击→注册→付费"）必须用**同一种定义**统计分子和分母。常见错误：用"曝光人数"做分母，却用"点击次数"做分子，次数/人数混用导致转化率失真。

**怎么做**：定义漏斗前，先把每一环是"人数"还是"次数"、是否去重、时间窗口多长，全部白纸黑字写下来。

## 归因窗口

一个用户今天付费，可能是上周的广告触达带来的。转化分析必须明确"归因窗口"（如 7 天、30 天），否则转化率无法比较。

## 重复转化去重

一个用户可能多次进入同一环节（如多次访问落地页）。统计"转化人数"时通常按用户去重，统计"转化次数"时则不去重——两者数字可以差几倍，结论前先说清是哪个。

## 整数截断制造假差异

计算转化率、折扣率时，用 `CAST AS BIGINT` 或 `::INTEGER` 会把小数截断，制造"前后对不上"的假差异。取整用 `ROUND()`。详见 `core/data-discipline.md` 第 4 条。

# Citations

- 相关：转化归因的具体行业案例见 `industry/education/index.md`（体验课订单归因错误）。
- 核心准则：`core/data-discipline.md`。
