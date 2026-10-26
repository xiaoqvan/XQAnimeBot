# TDLib 封装缺口（fuyubot-simplify → 动漫插件）

> 生成时间：2026-10-26 01:26。对照对象：
> - TS 侧：`F:/Node.JS/XQAnimeBot/src/TDLib/function/*.ts`（message/get/chat/files/text/markdown/inlineAdapter/permissions）
> - Rust 侧：`F:/Rust/FuyuBot/crates/fuyubot-simplify`（lib.rs + rich.rs）
> - 消费方：`crates/fuyubot-plugin-anime`

## 1. 结论

simplify 已经覆盖「发消息」这一整条（文本/图片/视频/相册/编辑/删除/回调查询/富文本），
缺的主要是「**读**」和「**inline**」：

| 能力 | TS 封装 | simplify 现状 | 插件是否需要 |
| --- | --- | --- | --- |
| sendMessage / sendPhoto / sendVideo | `message.ts` | ✅ `Simplify::send_message` + `Media` | 需要（发卡片、发视频） |
| sendAlbum | `message.ts` | ✅ `send_message_album` | 需要（多集/多图） |
| editText/Caption/Media | `message.ts` | ✅ 三个 `edit_message_*` | 需要（导航消息更新） |
| deleteMessage | `message.ts` | ✅ `delete_message` | 需要 |
| answerCallbackQuery | `text.ts` | ✅ `answer_callback_query` | 需要（审核按钮） |
| parseTextEntities | `text.ts` | ✅ `resolve_entities` | 需要（Markdown 文案） |
| downloadFile | `files.ts` | ✅ `download_file` | 需要（取封面/视频） |
| getMessage | `get.ts` | ❌ | 需要（回调里取原消息） |
| getChat / getSupergroup / getSupergroupFullInfo | `get.ts` | ❌ | 需要（频道标题、成员、权限） |
| getChatMember | `get.ts` | ❌ | 需要（管理员校验） |
| getUser / getMe | `get.ts` | ❌ | 需要（回调发起人、机器人自身） |
| getMessageLink / getMessageLinkInfo | `get.ts` | ❌ | 需要（资源链接、导航跳转） |
| answerInlineQuery | `inlineAdapter.ts` | ❌ | 需要（inline 搜索番剧/按 EPID 发视频） |
| pinChatMessage / forwardMessages | —（TS 用 client 直调） | ❌ | 可选（置顶导航） |
| restrictUser / isUserAdmin | `permissions.ts` | ❌ | 可选（命令权限；也可用 config.owner_id 自己判断） |

## 2. 处理方式

上游 `F:/Rust/FuyuBot` 是独立仓库，且当前沙箱不允许写工作区外路径，所以先在**本仓**落一个
过渡 crate：`crates/fuyubot-tdlib-ext`（只依赖 `fuyubot-abi` + `serde_json` + `thiserror`）。
它按 simplify 的风格直接组 TDLib JSON，接口稳定后可以整文件搬进上游。

约定：
- 每个方法一个 `*_params()` 纯函数（可单测）+ 一个发送方法；
- 字段名严格按 TDLib `td_api.tl`（驼峰），不做二次抽象；
- 错误只分两类：`Host`（宿主调用失败）与 `Payload`（返回体缺字段/类型不对）。

## 3. 待办

- [x] `crates/fuyubot-tdlib-ext` 落地读接口（上表里 `getMessage` / `getChat` /
      `getSupergroup` / `getChatMember` / `getUser` / `getMe` / `getMessageLink` /
      `answerInlineQuery` / `pinChatMessage` 都已在，`restrictUser` 未做，权限改走
      `getChatMember` + 配置白名单，见 `perm.rs`）
- [x] inline query 订阅：**事件名是 `updateNewInlineQuery`**（不是 TS 那层的
      `updateInlineQuery`），订阅方式就是 `host.on("updateNewInlineQuery", ...)`；
      回包 `TdExt::answer_inline_query`。落地见
      [inline.rs](../crates/fuyubot-plugin-anime/src/inline.rs) 与
      [plan-parity-newdbtest.md](plan-parity-newdbtest.md) §8
- [ ] 上游发版后把该 crate 合并回 `fuyubot-simplify`，本仓改回版本依赖

## 4. inline 结果类型的坑（2026-10-27 补）

`answerInlineQuery` 收的是 `InputInlineQueryResult`，与查询返回的
`InlineQueryResult` 不是同一套类型。`inputInlineQueryResultVideo` 的 `video_url` /
`thumbnail_url` 必须是 **HTTP URL**（客户端拿它做预览），**不能**填 Telegram 的文件 id；
要发一个已存在的文件，正确做法是把 `inputMessageVideo{inputFileRemote}` 放进
`input_message_content`，卡片本身用 `inputInlineQueryResultArticle`。
