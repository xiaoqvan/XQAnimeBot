# 出图：ultralightGenImage 插件

原来 TS 版是 `bangumi.vue → satori（Flexbox 布局）→ SVG → resvg → sharp → JPEG`。
Rust 侧没有 satori 的等价物，硬重写一套 CSS 布局引擎不划算，于是改成
**真浏览器渲染**：Ultralight 2.0（WebKit 内核）跑真 HTML/CSS，然后离屏截图。

数据绑定也经历了两代：先是在页面里跑 Vue（注入运行时 + JSON，页面自己渲染完置
`__READY__`），现在改成**纯 HTML 模板**——数据在 Rust 侧就绑进模板（mustache 子集），
进渲染器的就是最终页面。没有 100KB 的 Vue 运行时，没有就绪协议，模板字段写错会在
出图前直接报错，而不是截出一张悄悄缺一块的图。

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
    │  Rust 侧绑定数据 → Ultralight 2.0：LoadHTML → 等页面稳定 → 截图
    ▼
   JPEG/PNG 路径 → 交给 TDLib 的 inputFileLocal
```

| 路径 | 角色 | 认不认识动漫 |
| --- | --- | --- |
| `crates/ultralight-gen-image` (cdylib) | 注册 `image.render`，管渲染进程生命周期 | 不认识 |
| 同 crate 的 `image-shot` bin | 渲染器：模板 + 数据 → 图片 | 不认识 |
| 同 crate 的 `src/template.rs` | 模板绑定（mustache 子集） | 不认识 |
| `crates/anime-core/card` | 把番剧数据整理成模板 payload | 认识 |

早先这里拆过三个 crate（`image-shot` / `image-shot-client` / `fuyubot-plugin-imagegen`）：
渲染器一个、客户端一个、插件一个。合并的原因很实在——**请求结构写了两份**，
字段靠手抄对齐，改一处忘一处只在运行时炸。现在 `RenderRequest` 只有一份，
插件写出去、渲染器读回来是同一个类型（有 round-trip 单测钉住）。

两个产物仍然是必须的：Ultralight 要求 Renderer 在**主线程**创建并驱动，而宿主进程
主线程归 tokio。插件里抢不到主线程，硬塞会直接崩。子进程自己拥有主线程，顺带把
「渲染器崩了 = 机器人崩了」这条风险切断了。

跨插件调用不再需要专门的客户端 crate：`fuyubot_service::call_service` 就是通用的。

## 模板契约：纯 HTML + 六种标签

模板就是 HTML，加六种标签（实现见 `src/template.rs`，那里有完整语法说明）：

| 写法 | 作用 |
| --- | --- |
| `{{ 字段 }}` | 输出并 HTML 转义（放进属性里也安全） |
| `{{& 字段 }}` / `{{{ 字段 }}}` | 原样输出（拼好的 HTML 片段才用） |
| `{{#if 字段}} … {{else}} … {{/if}}` | 真值走前半段，假值/缺失走后半段 |
| `{{#each 字段}} … {{/each}}` | 遍历数组（循环里有 `this`、`@index`、`@number`、`@first`、`@last`） |
| `{{ a.b.c }}` | 点路径取嵌套字段 |

三条规则，别的没有：

1. **字段不存在直接报错**（`#if` 除外，缺失一律当假）。模板和数据对不上宁可渲染失败
   并指出字段名——「改一处忘一处只在运行时炸」是这套迁移里最贵的教训；
2. `html` + `data` 会把 `html` 当模板绑一遍（插件内嵌模板文本走的就是这条）；
   不带 `data` 的 `html` 视为最终页面，原样渲染；
3. 页面没有 JS，没有就绪协议，页面 settled 即截图；真要等 JS 动作，显式传 `ready_expr`。

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
   只判 null 会让页面永远「不就绪」，等到超时后截到白图。（纯 HTML 模板用不到求值，
   但传了 `ready_expr` 就会走到这条路上。）
2. 求值结果字符串归 SDK 所有，**不能** `ulDestroyString`，否则双释放，进程直接消失。

## 实测

| 模板 | 尺寸 | 体积 | 耗时（debug，`image-shot --once`，含进程与渲染器初始化） |
| --- | --- | --- | --- |
| `demo.html`（通用） | 2000×1100 | 181 KB | 1.3 s |
| `bangumi-card.html`（动漫，template 路径） | 2000×1100 | 231 KB | 1.4 s |
| `bangumi-card.html`（动漫，html + data 路径） | 2000×1100 | 231 KB | 1.0 s |

两条绑定路径产出**逐字节一致**（MD5 相同）。首图有进程启动成本，插件 `start()` 会预热，
常驻模式下比上表快。

## 现状

- 渲染链路、服务注册、模板绑定：**已通**，有单测，示例可跑，真机出图验证过；
- `html` + `data` 的绑定已修：插件内嵌模板文本这条路以前是断的（占位符根本没被替换）；
  修复后两条路径输出逐字节一致；
- 未在真实 FuyuBot 宿主里联调（需要在宿主里装插件看日志）；
- 用户收藏卡模板（原 `generateUserCollectionImage.ts`）还没迁。
