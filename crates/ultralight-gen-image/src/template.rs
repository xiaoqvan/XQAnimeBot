//! 极简 HTML 模板引擎（mustache 子集）——出图的「数据绑定」全在这里。
//!
//! 早期方案是「模板里跑 Vue 运行时」：注入 vue.global.prod.js + __IMAGE_DATA__，
//! 页面自己渲染完置 window.__READY__。等于把绑定搬进浏览器，代价是 100KB+ 的运行时、
//! 一整套 __READY__ 协议，以及「模板字段写错只在出图时白屏」。现在改成**纯 HTML 模板**：
//! 数据在 Rust 侧就绑好，进渲染器的就是最终页面，零 JS、零等待。
//!
//! 语法只有六种，认完就会写模板：
//!
//! ~~~text
//! {{ name }}                   输出并 HTML 转义（< 变 &lt;，放进属性里也安全）
//! {{& name }} / {{{ name }}}   原样输出（拼好的 HTML 片段才用）
//! {{#if name}} … {{else}} … {{/if}}   真值走前半段，假值/缺失走后半段
//! {{#each rows}} … {{/each}}   遍历数组
//! {{ this }} / {{ @index }}    循环里：当前元素 / 0 起下标
//! {{ a.b.c }}                  点路径取嵌套字段
//! ~~~
//!
//! 循环里还能用 @index（0 起）、@number（1 起）、@first、@last；字段先在当前层找，
//! 找不到往外层作用域找（标准 mustache 的上下文栈），最外层就是 data 根。
//!
//! **字段不存在直接报错**（#if 除外，它对缺失字段一律当假）：模板和数据对不上，
//! 宁可渲染失败并指出字段名，也不要出一张悄悄缺一块的图——「改一处忘一处只在运行时炸」
//! 是这套迁移里最贵的教训。
//!
//! 注意：双开花括号永远是标签开头，模板里要输出它得拆开写（比如中间加个空格）。

use serde_json::Value;

/// 渲染错误：模板语法不对，或数据接不上模板。
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// 模板本身写错了（标签没闭合、路径不合法……）。
    #[error("模板语法错误（第 {line} 行）：{message}")]
    Syntax { line: usize, message: String },
    /// 模板引用了数据里没有的字段。
    #[error("模板字段不存在：{0}")]
    MissingField(String),
    /// 想直接输出一个对象 / 数组。
    #[error("模板字段 {path} 是{kind}，不能直接输出（请用 #each 或取具体字段）")]
    NotScalar { path: String, kind: &'static str },
}

type Result<T> = std::result::Result<T, TemplateError>;

/// 渲染入口：把 data 绑进 template，返回最终 HTML。
pub fn render(template: &str, data: &Value) -> Result<String> {
    let nodes = parse(template)?;
    let mut out = String::with_capacity(template.len() + 256);
    let mut scopes = vec![Frame {
        value: data,
        meta: None,
    }];
    emit(&nodes, &mut scopes, &mut out)?;
    Ok(out)
}

// ─── 解析 ──────────────────────────────────────────────────────────────────

/// 一个解析后的模板节点。
enum Node {
    /// 原样文本。
    Text(String),
    /// 单花括号（转义）或 and 号 / 三花括号（原样）插值。
    Interp { path: String, raw: bool },
    /// if 块。
    If {
        path: String,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
    /// each 块。
    Each { path: String, body: Vec<Node> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    If,
    Each,
}

impl BlockKind {
    fn name(self) -> &'static str {
        match self {
            BlockKind::If => "if",
            BlockKind::Each => "each",
        }
    }
}

/// 一个标签。
enum Tag {
    Interp { path: String, raw: bool },
    Open { kind: BlockKind, path: String },
    Else,
    Close(BlockKind),
}

/// 块解析结束在哪种边界上。
enum Terminator {
    Else,
    Close(BlockKind),
    Eof,
}

fn parse(template: &str) -> Result<Vec<Node>> {
    let mut parser = Parser {
        chars: template.chars().collect(),
        pos: 0,
        line: 1,
    };
    let (nodes, terminator) = parser.parse_block(true)?;
    match terminator {
        // 顶层把 else 收进来就说明没有对应的 if。
        Terminator::Else => Err(parser.err("else 没有对应的 #if")),
        Terminator::Close(kind) => Err(parser.err(&format!(
            "多余的结束标签 /{}，没有对应的开始标签",
            kind.name()
        ))),
        Terminator::Eof => Ok(nodes),
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    line: usize,
}

impl Parser {
    fn err(&self, message: &str) -> TemplateError {
        TemplateError::Syntax {
            line: self.line,
            message: message.to_owned(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch == Some('\n') {
            self.line += 1;
        }
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    /// 解析一段块，遇到允许的 else 或结束标签就停。
    fn parse_block(&mut self, at_else: bool) -> Result<(Vec<Node>, Terminator)> {
        let mut nodes = Vec::new();
        let mut text = String::new();
        loop {
            // 文本段：一直读到双花括号或结尾。
            while self.peek().is_some() {
                if self.peek() == Some('{') && self.peek_at(1) == Some('{') {
                    break;
                }
                text.push(self.bump().expect("peek 到了就有"));
            }
            if !text.is_empty() {
                nodes.push(Node::Text(std::mem::take(&mut text)));
            }
            if self.peek().is_none() {
                return Ok((nodes, Terminator::Eof));
            }

            match self.tag()? {
                Tag::Interp { path, raw } => nodes.push(Node::Interp { path, raw }),
                Tag::Open { kind, path } => nodes.push(self.parse_open(kind, path)?),
                Tag::Else if at_else => return Ok((nodes, Terminator::Else)),
                Tag::Else => return Err(self.err("else 没有对应的 #if")),
                Tag::Close(close) => return Ok((nodes, Terminator::Close(close))),
            }
        }
    }

    /// 解析一个完整的 if / each 块（含子块）。
    fn parse_open(&mut self, kind: BlockKind, path: String) -> Result<Node> {
        let (then, terminator) = self.parse_block(kind == BlockKind::If)?;
        match (kind, terminator) {
            (BlockKind::If, Terminator::Else) => {
                let (otherwise, terminator) = self.parse_block(false)?;
                match terminator {
                    Terminator::Close(BlockKind::If) => Ok(Node::If {
                        path,
                        then,
                        otherwise,
                    }),
                    Terminator::Close(other) => Err(self.err(&format!(
                        "if 块里混进了 /{} 结束标签",
                        other.name()
                    ))),
                    Terminator::Else | Terminator::Eof => Err(self.err("#if 块没有以 /if 结束")),
                }
            }
            (BlockKind::If, Terminator::Close(BlockKind::If)) => Ok(Node::If {
                path,
                then,
                otherwise: Vec::new(),
            }),
            (BlockKind::Each, Terminator::Close(BlockKind::Each)) => {
                Ok(Node::Each { path, body: then })
            }
            (_, Terminator::Close(other)) => Err(self.err(&format!(
                "{} 块没有以 /{} 结束",
                kind.name(),
                other.name()
            ))),
            // parse_block 对 each 不收 else（at_else=false），走到这说明没闭合。
            (_, Terminator::Else) | (_, Terminator::Eof) => Err(self.err(&format!(
                "{} 块没有以 /{} 结束",
                kind.name(),
                kind.name()
            ))),
        }
    }

    /// 读一个标签的内部并分类。
    fn tag(&mut self) -> Result<Tag> {
        self.bump(); // {
        self.bump(); // {
        let triple = self.peek() == Some('{');
        if triple {
            self.bump();
        }

        let mut body = String::new();
        let mut closed = false;
        while self.peek().is_some() {
            if self.peek() == Some('}') {
                if triple && self.peek_at(1) == Some('}') && self.peek_at(2) == Some('}') {
                    self.bump();
                    self.bump();
                    self.bump();
                    closed = true;
                    break;
                }
                if !triple && self.peek_at(1) == Some('}') {
                    self.bump();
                    self.bump();
                    closed = true;
                    break;
                }
            }
            body.push(self.bump().expect("peek 到了就有"));
        }
        if !closed {
            return Err(self.err("标签没有闭合（缺右花括号）"));
        }

        let body = body.trim();
        if triple {
            return Ok(Tag::Interp {
                path: require_path(self, body)?,
                raw: true,
            });
        }
        if let Some(rest) = body.strip_prefix('#') {
            let rest = rest.trim();
            if let Some(path) = rest.strip_prefix("if ") {
                return Ok(Tag::Open {
                    kind: BlockKind::If,
                    path: require_path(self, path.trim())?,
                });
            }
            if let Some(path) = rest.strip_prefix("each ") {
                return Ok(Tag::Open {
                    kind: BlockKind::Each,
                    path: require_path(self, path.trim())?,
                });
            }
            return Err(self.err("只支持 #if 字段 和 #each 字段 两种开始标签"));
        }
        if let Some(rest) = body.strip_prefix('/') {
            return match rest.trim() {
                "if" => Ok(Tag::Close(BlockKind::If)),
                "each" => Ok(Tag::Close(BlockKind::Each)),
                other => Err(self.err(&format!("不支持的结束标签：/{other}"))),
            };
        }
        if body == "else" {
            return Ok(Tag::Else);
        }
        if let Some(path) = body.strip_prefix('&') {
            return Ok(Tag::Interp {
                path: require_path(self, path.trim())?,
                raw: true,
            });
        }
        Ok(Tag::Interp {
            path: require_path(self, body)?,
            raw: false,
        })
    }
}

/// 标签里得有一个能查的字段路径；空路径 / 断开的点路径在解析期就报错。
fn require_path(parser: &Parser, path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(parser.err("标签里没有字段名"));
    }
    if path.split('.').any(|segment| segment.is_empty()) {
        return Err(parser.err(&format!("字段路径不合法：{path}")));
    }
    Ok(path.to_owned())
}

// ─── 求值 ──────────────────────────────────────────────────────────────────

/// 一层作用域：each 的一层就是「当前元素 + 下标信息」。
struct Frame<'a> {
    value: &'a Value,
    meta: Option<Meta>,
}

/// each 里的下标信息。
#[derive(Clone, Copy)]
struct Meta {
    index: usize,
    len: usize,
}

/// 查路径查到的东西：值、循环元信息，或什么都没有。
enum Found<'a> {
    Value(&'a Value),
    Meta(MetaValue),
    Missing,
}

#[derive(Clone, Copy)]
enum MetaValue {
    Index(usize),
    Number(usize),
    First(bool),
    Last(bool),
}

/// 按 mustache 的上下文栈查路径：首段从最内层往外找，剩下的在找到的值里取。
fn lookup<'a>(scopes: &[Frame<'a>], path: &str) -> Found<'a> {
    if let Some(name) = path.strip_prefix('@') {
        let top = scopes.last().expect("至少有根作用域");
        let Some(meta) = top.meta else {
            return Found::Missing;
        };
        return match name {
            "index" => Found::Meta(MetaValue::Index(meta.index)),
            "number" => Found::Meta(MetaValue::Number(meta.index + 1)),
            "first" => Found::Meta(MetaValue::First(meta.index == 0)),
            "last" => Found::Meta(MetaValue::Last(meta.index + 1 == meta.len)),
            _ => Found::Missing,
        };
    }

    let mut segments = path.split('.');
    let head = segments.next().expect("路径非空");
    let rest: Vec<&str> = segments.collect();

    // this / 点开头：只看当前层。
    let mut value = if head == "this" || head == "." {
        Some(scopes.last().expect("至少有根作用域").value)
    } else {
        scopes.iter().rev().find_map(|frame| frame.value.get(head))
    };

    for segment in rest {
        match value {
            Some(Value::Object(map)) => value = map.get(segment),
            _ => return Found::Missing,
        }
    }
    match value {
        Some(value) => Found::Value(value),
        None => Found::Missing,
    }
}

fn emit(nodes: &[Node], scopes: &mut Vec<Frame<'_>>, out: &mut String) -> Result<()> {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Interp { path, raw } => {
                let text = scalar_text(scopes, path)?;
                if *raw {
                    out.push_str(&text);
                } else {
                    escape_into(&text, out);
                }
            }
            Node::If {
                path,
                then,
                otherwise,
            } => {
                let branch = if truthy(scopes, path) { then } else { otherwise };
                emit(branch, scopes, out)?;
            }
            Node::Each { path, body } => match lookup(scopes, path) {
                Found::Value(Value::Array(items)) => {
                    let len = items.len();
                    for (index, item) in items.iter().enumerate() {
                        scopes.push(Frame {
                            value: item,
                            meta: Some(Meta { index, len }),
                        });
                        emit(body, scopes, out)?;
                        scopes.pop();
                    }
                }
                // null 当成空列表：可选列表给 null 时什么都不画。
                Found::Value(Value::Null) => {}
                Found::Value(other) => {
                    return Err(TemplateError::NotScalar {
                        path: path.clone(),
                        kind: type_name(other),
                    });
                }
                Found::Meta(_) | Found::Missing => {
                    return Err(TemplateError::MissingField(path.clone()));
                }
            },
        }
    }
    Ok(())
}

/// 插值的文本形态：标量直接转字符串，null 是空，对象 / 数组直接报错。
fn scalar_text(scopes: &[Frame<'_>], path: &str) -> Result<String> {
    match lookup(scopes, path) {
        Found::Value(Value::Null) => Ok(String::new()),
        Found::Value(value @ (Value::Object(_) | Value::Array(_))) => Err(TemplateError::NotScalar {
            path: path.to_owned(),
            kind: type_name(value),
        }),
        Found::Value(Value::String(text)) => Ok(text.clone()),
        Found::Value(Value::Number(number)) => Ok(number.to_string()),
        Found::Value(Value::Bool(flag)) => Ok(flag.to_string()),
        Found::Meta(MetaValue::Index(index)) => Ok(index.to_string()),
        Found::Meta(MetaValue::Number(number)) => Ok(number.to_string()),
        Found::Meta(MetaValue::First(_) | MetaValue::Last(_)) => {
            Err(TemplateError::MissingField(path.to_owned()))
        }
        Found::Missing => Err(TemplateError::MissingField(path.to_owned())),
    }
}

/// if 的真值判定：缺失一律当假；空串 / 0 / false / 空数组 / 空对象也是假。
fn truthy(scopes: &[Frame<'_>], path: &str) -> bool {
    match lookup(scopes, path) {
        Found::Value(Value::Null) => false,
        Found::Value(Value::Bool(flag)) => *flag,
        Found::Value(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Found::Value(Value::String(text)) => !text.is_empty(),
        Found::Value(Value::Array(items)) => !items.is_empty(),
        Found::Value(Value::Object(map)) => !map.is_empty(),
        Found::Meta(MetaValue::Index(index)) => index != 0,
        Found::Meta(MetaValue::Number(number)) => number != 0,
        Found::Meta(MetaValue::First(flag) | MetaValue::Last(flag)) => flag,
        Found::Missing => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "对象",
        Value::Array(_) => "数组",
        _ => "非标量",
    }
}

/// HTML 转义：文本和属性两种上下文都安全。
fn escape_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interpolates_scalars_with_html_escaping() {
        let html = render(
            "Hi {{ name }} / {{ n }} / {{ ok }} / {{ empty }}",
            &json!({
                "name": "<b>\"x\"</b> & 'y'",
                "n": 8.9,
                "ok": true,
                "empty": null,
            }),
        )
        .unwrap();
        assert_eq!(
            html,
            "Hi &lt;b&gt;&quot;x&quot;&lt;/b&gt; &amp; &#39;y&#39; / 8.9 / true / "
        );
    }

    #[test]
    fn raw_interpolation_skips_escaping() {
        let html = render(
            "{{& a }}|{{{ b }}}",
            &json!({ "a": "<i>x</i>", "b": "<u>y</u>" }),
        )
        .unwrap();
        assert_eq!(html, "<i>x</i>|<u>y</u>");
    }

    #[test]
    fn dotted_paths_reach_nested_fields() {
        let html = render(
            "{{ rating.score }}-{{ collection.wish_fmt }}",
            &json!({
                "rating": { "score": "8.9" },
                "collection": { "wish_fmt": "1.2K" },
            }),
        )
        .unwrap();
        assert_eq!(html, "8.9-1.2K");
    }

    #[test]
    fn missing_field_is_an_error_naming_the_field() {
        let err = render("{{ title }}", &json!({ "name": "x" })).unwrap_err();
        assert!(matches!(&err, TemplateError::MissingField(path) if path == "title"), "{err}");
    }

    #[test]
    fn nested_missing_field_is_also_an_error() {
        let err = render("{{ rating.score }}", &json!({ "rating": {} })).unwrap_err();
        assert!(matches!(&err, TemplateError::MissingField(path) if path == "rating.score"), "{err}");
    }

    #[test]
    fn if_treats_missing_and_falsy_as_false() {
        let html = render(
            "{{#if platform}}P{{else}}N{{/if}}{{#if empty}}E{{else}}e{{/if}}{{#if zero}}Z{{else}}z{{/if}}",
            &json!({ "platform": "TV", "empty": [], "zero": 0 }),
        )
        .unwrap();
        assert_eq!(html, "Pez");
    }

    #[test]
    fn if_does_not_complain_about_missing_fields() {
        let html = render("{{#if nope}}Y{{else}}N{{/if}}", &json!({})).unwrap();
        assert_eq!(html, "N");
    }

    #[test]
    fn each_iterates_with_meta_and_this() {
        let html = render(
            "{{#each rows}}[{{ @index }}:{{ @number }}:{{ this }}{{#if @first}}!{{/if}}{{#if @last}}?{{/if}}]{{/each}}",
            &json!({ "rows": ["a", "b"] }),
        )
        .unwrap();
        assert_eq!(html, "[0:1:a!][1:2:b?]");
    }

    #[test]
    fn each_falls_back_to_outer_scope() {
        let html = render(
            "{{#each rows}}{{ title }}-{{ name }};{{/each}}",
            &json!({
                "title": "T",
                "rows": [{ "name": "a" }, { "name": "b" }],
            }),
        )
        .unwrap();
        assert_eq!(html, "T-a;T-b;");
    }

    #[test]
    fn each_nests() {
        let html = render(
            "{{#each groups}}({{ label }}:{{#each items}}{{ this }},{{/each}}){{/each}}",
            &json!({
                "groups": [
                    { "label": "SP", "items": [1, 2] },
                    { "label": "OP", "items": [3] },
                ],
            }),
        )
        .unwrap();
        assert_eq!(html, "(SP:1,2,)(OP:3,)");
    }

    #[test]
    fn each_over_null_renders_nothing() {
        let html = render("[{{#each maybe}}{{ this }}{{/each}}]", &json!({ "maybe": null })).unwrap();
        assert_eq!(html, "[]");
    }

    #[test]
    fn each_over_missing_or_non_array_is_an_error() {
        let err = render("{{#each rows}}{{ this }}{{/each}}", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::MissingField(path) if path == "rows"), "{err}");
        let err = render("{{#each rows}}{{ this }}{{/each}}", &json!({ "rows": 3 })).unwrap_err();
        assert!(matches!(&err, TemplateError::NotScalar { .. }), "{err}");
    }

    #[test]
    fn object_interpolation_is_an_error() {
        let err = render("{{ rating }}", &json!({ "rating": { "score": 1 } })).unwrap_err();
        assert!(matches!(&err, TemplateError::NotScalar { path, kind: "对象" } if path == "rating"), "{err}");
    }

    #[test]
    fn syntax_errors_are_reported_with_line_numbers() {
        let err = render("a\n{{#if x}}b", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { line: 2, .. }), "{err}");

        let err = render("{{/if}}", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { .. }), "{err}");

        let err = render("{{#each x}}{{/if}}", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { .. }), "{err}");

        let err = render("{{ else }}", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { .. }), "{err}");

        let err = render("{{ x.y. }}", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { .. }), "{err}");

        let err = render("{{ x", &json!({})).unwrap_err();
        assert!(matches!(&err, TemplateError::Syntax { .. }), "{err}");
    }

    #[test]
    fn plain_html_passes_through_untouched() {
        let html = render(
            "<div style=\"width: 100%\">{ not a tag }</div>",
            &json!({}),
        )
        .unwrap();
        assert_eq!(html, "<div style=\"width: 100%\">{ not a tag }</div>");
    }
}
