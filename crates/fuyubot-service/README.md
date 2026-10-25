# fuyubot-service

插件服务的公共外壳：把「名字 + 方法表 + JSON 进出」这套 ABI 样板收成一处，插件只写业务函数。

```rust
impl Plugin for MyPlugin {
    fn init(host: HostApi) -> Result<Self, String> {
        Service::new("storage.mongo", vec![
            ("ping", svc_ping as Handler),
            ("find", svc_find as Handler),
        ])
        .register(&host)?;
        Ok(Self { host })
    }
}
```

调用方统一走 `call_service`，不需要为每个服务写客户端 crate。

## 三条约定（写新服务时别忘）

1. **回复字符串由提供方持有**，调用方读完即弃——所以放在线程局部缓冲里，宿主不 free 插件指针；
2. **`ServiceMethodFn` 是同步的**，慢操作自己开线程，别堵住宿主回调线程（一次渲染 0.2~1s 就够堵死事件流）；
3. **注册后 vtable 必须活到进程结束**，`Service::register` 会把它泄漏成 `'static`。

## 为什么服务名是「点号名」而不是插件名

服务是**契约**，插件是可替换实现。叫 `image.render` 而不是 `ultralightGenImage.render`，
换渲染器时所有调用方都不用改。
