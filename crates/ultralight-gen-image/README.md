# ultralightGenImage

一个插件搞定 HTML 出图：注册 `image.render` 服务，用 Ultralight 2.0 把 HTML 截成图片。

插件里**没有任何业务字段**——模板、数据、尺寸全由调用方给。动漫卡、报表、周报、订单都调它。
以前拆成三个 crate（渲染器 / 客户端 / 插件），现在合成一个：一个目录、一份请求类型、两个产物。

## 产物

| 产物 | 是什么 |
| --- | --- |
| `ultralight_gen_image.dll` | 插件本体（cdylib）。只发请求，不碰 Ultralight |
| `image-shot.exe` | 渲染器（同包 bin）。真正加载 Ultralight、跑 Vue、截图 |

```bash
cargo build -p ultralight-gen-image          # 一次编出两个产物
```

**为什么还是两个文件**：Ultralight 要求 Renderer 在**主线程**创建并驱动，而宿主主线程归
tokio，插件抢不到。渲染器必须以独立进程跑——但源码是同一个 crate，请求类型只有
[`RenderRequest`](src/request.rs) 一份，不会再出现两边字段对不上。

好处是渲染器崩了只崩一张图，机器人不受影响；升级渲染器也不用动插件。

## 调用约定

服务 `image.render`，两个方法：

| 方法 | 入参 | 出参 |
| --- | --- | --- |
| `render` | `{"template","template_dir","data","out","width","height","scale","format","quality","ready_expr"}`，或 `{"html","out",...}` | `{"ok":true,"path","width","height","bytes","ms"}` |
| `ping` | 任意 | `{"ok":true,"backend":"ultralight","plugin":"ultralightGenImage","info":{...}}` |

`out` 必填：TDLib 只接受本地路径，所以图片必须落盘。

业务插件侧不用手写 vtable 调用，用公共外壳的 `call_service`：

```rust
use fuyubot_service::call_service;

let result = call_service(&self.host, "image.render", "render", &serde_json::json!({
    "template": "bangumi-card",
    "template_dir": "plugins/anime/templates",
    "data": payload,
    "out": "D:/tmp/card.jpg",
    "width": 1000,
    "height": 550,
    "scale": 2.0
}))?;
// result["path"] 直接交给 TDLib inputFileLocal
```

**服务是同步的**：`ServiceMethodFn` 只有同步形态。调用方要放进 `spawn_blocking` 或专用线程，
别塞进 TDLib 的更新投递线程——一次渲染 0.2~1s，会堵住整个事件流。

## 模板契约

模板是**纯 HTML**，三条约定：

1. `<!--__VUE_RUNTIME__-->` 会被替换成 Vue 3 运行时（内联，无外部请求）；
2. `__IMAGE_DATA__` 会被替换成调用方给的 JSON（`<` 已转义，数据里写 `</script>` 也炸不掉页面）；
3. 数据渲染完置 `window.__READY__ = true`，渲染器等「页面 settled + 该表达式为真」两帧后截图。

模板跟着**业务**走，不放本插件里。本插件的 `templates/demo.html` 只是接口示例。

## 启动配置（可省略）

```json
{ "binary": "D:/.../image-shot.exe", "templates_dir": "D:/.../templates" }
```

`templates_dir` 是「请求里没给 `template_dir` 时」的默认值，方便单业务场景少传一个字段。

## 部署

```
fuyubot.exe
plugins/
  ultralight_gen_image.dll      ← 本插件
  fuyubot_plugin_anime.dll
ultralight/
  image-shot.exe
  Ultralight.dll / UltralightCore.dll / WebCore.dll / AppCore.dll
  resources/{icudt67l.dat,cacert.pem}
```

`image-shot.exe` 查找顺序：`IMAGE_SHOT_BIN` → 插件同级 → 同级 `ultralight/` → 上级目录。
Ultralight DLL 查找：`ULTRALIGHT_DIR` → exe 同级 → 同级 `ultralight/` → 仓库 `runtime/ultralight`。

## 联调

```bash
# 渲染器自检（运行时有没有就位）
cargo run -p ultralight-gen-image --bin image-shot -- --check

# 端到端：拉起渲染器 → 渲染 demo 模板 → 落盘
cargo run -p ultralight-gen-image --example render_card

# 单次渲染（请求 JSON 见 examples/render_card.rs 的结构）
cargo run -p ultralight-gen-image --bin image-shot -- --once req.json
```

实测（本机，debug 构建）：demo 卡 2000×1100 / 173KB / 653ms；
动漫卡 2000×1100 / 190KB / 758ms。
