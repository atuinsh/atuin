# stats

Atuin 还可以基于你的历史记录计算统计数据——目前这个功能还比较基础，但后续会加入更多特性。

## 1 天统计

你提供一个起始时间点，Atuin 便会计算从该时间点起 24 小时内的统计数据。
日期解析由 `interim` 提供，它支持多种完整或相对日期格式，其中部分格式需要依赖
[配置](../configuration/config.md#dialect) 中的 dialect 选项来区分日期与月份。
关于支持的日期格式，更多详情请参阅 [该模块的文档](https://docs.rs/interim/latest/interim/#supported-formats)。

```console
$ atuin stats last friday

+---------------------+------------+
| Statistic           | Value      |
+---------------------+------------+
| Most used command   | git status |
+---------------------+------------+
| Commands ran        |        450 |
+---------------------+------------+
| Unique commands ran |        213 |
+---------------------+------------+

# A few more examples:
$ atuin stats 2018-04-01
$ atuin stats April 1
$ atuin stats 01/04/22
$ atuin stats last thursday 3pm  # between last thursday 3:00pm and the following friday 3:00pm
```

## 完整历史记录统计

```console
$ atuin stats
# or
$ atuin stats all

+---------------------+-------+
| Statistic           | Value |
+---------------------+-------+
| Most used command   |    ls |
+---------------------+-------+
| Commands ran        |  8190 |
+---------------------+-------+
| Unique commands ran |  2996 |
+---------------------+-------+
```
