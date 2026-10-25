# 出图：ultralightGenImage 插件

原来 TS 版是 `bangumi.vue → satori（Flexbox 布局）→ SVG → resvg → sharp → JPEG`。
Rust 侧没有 satori 的等价物，硬重写一套 CSS 布局引擎不划算，于是改成
**真浏览器渲染**：Ultralight 2.0（WebKit 内核）跑真 HTML/CSS/Vue，然后离屏截图。

同时定了另一条边界：**出图不属于动漫业务**。它是一个独立插件，谁都能调。

## 一个 crate，两个产物

```
任意业务插件（动漫、报表、周报……）
    │  call_service(host, "image.render", "render", {...})
    ▼
ultralight_gen_image.dll     插件本体（cdylib，同进程，**零业务知识**）
    │  拉子进程 + JSON 行协议（stdin/stdout）
    ▼
image-shot.exe               同一 crate 的 bin，独立进程，独占主线程
    │  Ultralight 2.0：LoadHTML → 跑 Vue → 等 __READY__ → 截图
    ▼
   JPEG/PNG 路径 → 交给 TDLib 的 inputFileLocal
```

| 路径 | 角色 | 认不认识动漫 |
| --- | --- | --- |
| `crates/ultralight-gen-image` (cdylib) | 注册 `image.render`，管渲染进程生命周期 | 不认识 |
| 同 crate 的 `image-shot` bin | 渲染器：HTML/模板 → 图片 | 不认识 |
| `crates/anime-core/card` | 把番剧数据整理成模板 payload | 认识 |

早先这里拆过三个 crate（`image-shot` / `image-shot-client` / `fuyubot-plugin-imagegen`）：
渲染器一个、客户端一个、插件一个。合并的原因很实在——**请求结构写了两份**，
字段靠手抄对齐，改一处忘一处只在运行时炸。现在 `RenderRequest` 只有一份，
插件写出去、渲染器读回来是同一个类型（有 round-trip 单测钉住）。

两个产物仍然是必须的：Ultralight 要求 Renderer 在**主线程**创建并驱动，而宿主进程
主线程归 tokio。插件里抢不到主线程，硬塞会直接崩。子进程自己拥有主线程，顺带把
「渲染器崩了 = 机器人崩了」这条风险切断了。

跨插件调用不再需要专门的客户端 crate：`fuyubot_service::call_service` 就是通用的。

## 模板契约（三条，别的一条都没有）

1. `<!--__VUE_RUNTIME__-->` 会被替换成 Vue 3 全局构建（内联，页面零外部请求）；
2. `__IMAGE_DATA__` 会被替换成调用方给的 JSON（`<` 已转义，数据里写 `</script>` 也闭合不了）；
3. 页面渲染完成后把 `window.__READY__ = true`，渲染器才截图。

模板放哪由调用方决定：传 `template_dir`，渲染器只按 `template_dir/<name>.html` 找文件。
动漫的 `bangumi-card.html` 在 `crates/fuyubot-plugin-anime/assets/templates/`，
出图插件里一行动漫代码都没有。

参考实现：`crates/ultralight-gen-image/templates/demo.html`（通用演示卡）。

## 调用方怎么写

```rust
use fuyubot_service::call_service;

let result = call_service(&self.host, "image.render", "render", &serde_json::json!({
    "template": "bangumi-card",
    "template_dir": "crates/fuyubot-plugin-anime/assets/templates",
    "data": card.to_html_payload(),
    "out": "D:/tmp/bangumi-card.jpg",
    "width": 1000,
    "height": 550,
    "scale": 2.0
}))?;
// result["path"] 交给 TDLib
```

服务是**同步**的（`ServiceMethodFn` 只有同步形态），调用要放在 `spawn_blocking`
或专用线程里，不要塞进 TDLib 的更新回调线程。

## 部署

1. `scripts/fetch-ultralight.ps1` 下载 Ultralight 2.0 SDK（约 56 MB，已 gitignore）；
2. DLL 与 `resources/` 放到 `runtime/ultralight/`；
3. `image-shot.exe` 与插件放在一起（查找链：`IMAGE_SHOT_BIN` → exe 同级 →
   `ultralight/` 子目录 → `target/debug|release`）。

插件启动配置（可省略）：

```json
{ "binary": "D:/.../image-shot.exe", "templates_dir": "D:/.../templates" }
```

## 两个踩过的坑（别改回去）

1. `ulViewEvaluateScript` 的 exception 出参在**没有异常时是空 ULString，不是空指针**。
   只判 null 会让页面永远「不就绪」，等到超时后截到白图。
2. 求值结果字符串归 SDK 所有，**不能** `ulDestroyString`，否则双释放，进程直接消失。

## 实测

| 模板 | 尺寸 | 体积 | 耗时（debug，含进程通信） |
| --- | --- | --- | --- |
| `demo.html`（通用） | 2000×1100 | 173 KB | 653 ms |
| `bangumi-card.html`（动漫） | 2000×1100 | 190 KB | 758 ms |

Release 下再快一截；首图有进程启动成本，所以插件 `start()` 会预热。

## 现状

- 渲染链路、服务注册、模板注入：**已通**，有单测，示例可跑；
- 未在真实 FuyuBot 宿主里联调（需要在宿主里装插件看日志）；
- 用户收藏卡模板（原 `generateUserCollectionImage.ts`）还没迁。
