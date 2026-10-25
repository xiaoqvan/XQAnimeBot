# fuyubot-plugin-mongo

MongoDB 插件：注册 `storage.mongo`，给**任意**插件提供集合级 CRUD。不认识番剧，也不认识任何业务表。

```
业务插件 ──get_service("storage.mongo")──▶ 本插件 ──官方 mongodb 驱动──▶ MongoDB
```

## 启动配置

```json
{ "uri": "mongodb://127.0.0.1:27017", "db": "xqanime" }
```

`uri` 支持标准连接串（含副本集、SRV、账号密码）。`start()` 会真 ping 一次——配错了在启动日志里就能看到，不用等第一条业务查询。

## 方法

所有方法入参都是 JSON，出参统一 `{"ok":true, ...}`；失败是 `{"ok":false,"error":"..."}`。
`collection` 必填，`db` 可选（覆盖默认库）。

| 方法 | 入参 | 说明 |
| --- | --- | --- |
| `ping` | – | 连通性 + 集合数 |
| `find` | `collection, filter?, projection?, sort?, limit?, skip?` | 上限 10 000 条 |
| `find_one` | `collection, filter?, projection?` | |
| `insert` | `collection, docs`（对象或数组） | 返回 `inserted` 与 `ids` |
| `update` | `collection, filter, update?/patch?, upsert?, many?` | `patch` 会自动包 `$set` |
| `delete` | `collection, filter, many?` | 空过滤会被拒绝（要清空得传 `allow_empty:true`） |
| `count` | `collection, filter?` | |
| `aggregate` | `collection, pipeline` | 大集合用这个，服务端先筛 |
| `distinct` | `collection, field, filter?` | |
| `ensure_index` | `collection, keys, unique?, sparse?, name?, ttl_seconds?` | 幂等 |
| `collections` | `db?` | 列集合名 |

示例：

```json
{"collection":"anime","filter":{"id":400602},"projection":{"name":1,"name_cn":1}}
{"collection":"torrents","patch":{"status":"done"},"filter":{"hash":"abc"},"upsert":true}
```

## 联调

```bash
cargo run -p fuyubot-plugin-mongo --example live
# 自定义：MONGO_URI=mongodb://127.0.0.1:27017 MONGO_TEST_DB=xqanime_it
```

在**独立测试库**（默认 `xqanime_it`）里跑完整 CRUD：建唯一索引 → 插入（含重复 id 拦截）→
find/find_one/count/aggregate/distinct → patch 更新 → upsert → 删除 → 清空。
不会碰正式库。

## 两个约定

1. **BSON 走 relaxed Extended JSON**：`ObjectId` 回来是 `{"$oid":"..."}`，日期是 `{"$date":"..."}`，
   数字和字符串保持自然形态。回写时同样的形态会被识别成对应 BSON 类型，不是普通子文档。
2. **服务是同步的**：驱动是异步的，插件内部用专用 tokio 运行时 + `Handle::block_on` 摊平。
   调用方（业务插件）不需要 async 传染，但别在 TDLib 更新回调线程里跑慢查询。

## 为什么没有「按业务建表」

TS 版的 `database/query.ts`（804 行）是「番剧专用查询函数集合」。搬到 Rust 后拆成两层：
本插件只做**通用通道**，业务语义（该查哪些字段、怎么排序）留在 `anime-core` 里。
好处是换个业务不用改数据库插件，坏处是业务侧要自己写 filter——这是有意的取舍。
