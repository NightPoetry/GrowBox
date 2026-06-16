//! tree-sitter 结构大纲(二期 D3 M4,第2层结构兜底)。
//!
//! 分层降级第2层(`设计文档/二期项目/项目设计/03-LSP集成.md` M4 + `设计原理/00` 推论5):
//! 语义层(LSP)不可用(无服务器/未装)时,用 tree-sitter 对文件做**结构解析**列出顶层符号
//! (函数/类型/类…),比纯文本(code_search)更结构化、又不依赖任何语言服务器进程。
//! 仍无对应 grammar 的语言 → 再退到文本层(code_search),AI 感知当前在哪一层。
//!
//! 内置 grammar:Rust / Python / JavaScript / TypeScript(含 TSX)。grammar 是编译进二进制的 C 解析表,
//! 不起子进程、不依赖 node —— 离线即用。机制纯函数,可单测(不碰 GUI/网络)。

use tree_sitter::{Language, Node, Parser};

/// 一个顶层(或嵌套)结构符号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// 人类可读类别(fn/struct/class/def…)。
    pub kind: String,
    /// 符号名。
    pub name: String,
    /// 起始行(1-based,与编辑器/file_read 对齐)。
    pub line: usize,
}

/// kind 字符串 → `(可读类别, 取名字段名)`。返回 None = 该 AST 节点不是我们要列的声明。
type KindFn = fn(&str) -> Option<(&'static str, &'static str)>;

fn rust_kind(k: &str) -> Option<(&'static str, &'static str)> {
    Some(match k {
        "function_item" => ("fn", "name"),
        "struct_item" => ("struct", "name"),
        "enum_item" => ("enum", "name"),
        "trait_item" => ("trait", "name"),
        "mod_item" => ("mod", "name"),
        "type_item" => ("type", "name"),
        "const_item" => ("const", "name"),
        "static_item" => ("static", "name"),
        "macro_definition" => ("macro", "name"),
        "impl_item" => ("impl", "type"), // impl 用 type 字段(无 name)
        _ => return None,
    })
}

fn py_kind(k: &str) -> Option<(&'static str, &'static str)> {
    Some(match k {
        "function_definition" => ("def", "name"),
        "class_definition" => ("class", "name"),
        _ => return None,
    })
}

fn js_kind(k: &str) -> Option<(&'static str, &'static str)> {
    Some(match k {
        "function_declaration" | "generator_function_declaration" => ("function", "name"),
        "class_declaration" => ("class", "name"),
        "method_definition" => ("method", "name"),
        _ => return None,
    })
}

fn ts_kind(k: &str) -> Option<(&'static str, &'static str)> {
    if let Some(x) = js_kind(k) {
        return Some(x);
    }
    Some(match k {
        "interface_declaration" => ("interface", "name"),
        "type_alias_declaration" => ("type", "name"),
        "enum_declaration" => ("enum", "name"),
        "abstract_class_declaration" => ("class", "name"),
        _ => return None,
    })
}

/// 扩展名 → (grammar 语言, kind 映射)。None = 无内置 grammar(退文本层)。
fn grammar(ext: &str) -> Option<(Language, KindFn)> {
    let g: (Language, KindFn) = match ext {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), rust_kind),
        "py" | "pyi" => (tree_sitter_python::LANGUAGE.into(), py_kind),
        "js" | "jsx" | "mjs" | "cjs" => (tree_sitter_javascript::LANGUAGE.into(), js_kind),
        "ts" | "mts" | "cts" => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), ts_kind),
        "tsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), ts_kind),
        _ => return None,
    };
    Some(g)
}

/// 本扩展名是否有内置结构 grammar(给 code_search/降级提示用)。
pub fn supports(ext: &str) -> bool {
    grammar(ext).is_some()
}

/// 解析源码列出结构符号(深度优先,含嵌套如 impl/class 内的方法)。
/// 返回 None = 无对应 grammar(调用方据此退到文本层)。Some(空) = 有 grammar 但没找到声明。
pub fn outline(source: &str, ext: &str, max: usize) -> Option<Vec<Symbol>> {
    let (language, kind_fn) = grammar(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut out = Vec::new();
    collect(tree.root_node(), source.as_bytes(), kind_fn, &mut out, max);
    Some(out)
}

fn collect(node: Node, src: &[u8], kind_fn: KindFn, out: &mut Vec<Symbol>, max: usize) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if out.len() >= max {
            return;
        }
        if let Some((label, name_field)) = kind_fn(child.kind()) {
            if let Some(name_node) = child.child_by_field_name(name_field) {
                if let Ok(name) = name_node.utf8_text(src) {
                    out.push(Symbol {
                        kind: label.to_string(),
                        name: name.to_string(),
                        line: child.start_position().row + 1,
                    });
                }
            }
        }
        collect(child, src, kind_fn, out, max); // 递归:列出嵌套声明(impl/class 内方法等)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_outline_lists_items() {
        let src = "struct Foo { a: u32 }\nfn bar(x: u32) -> u32 { x }\nimpl Foo { fn m(&self) {} }\n";
        let syms = outline(src, "rs", 100).expect("rust grammar");
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo") && names.contains(&"bar"), "应列出 struct/fn: {syms:?}");
        // impl 内的方法 m 也被递归列出。
        assert!(names.contains(&"m"), "应递归列出 impl 内方法: {syms:?}");
        // 行号 1-based:struct Foo 在第 1 行。
        let foo = syms.iter().find(|s| s.name == "Foo").unwrap();
        assert_eq!(foo.line, 1);
        assert_eq!(foo.kind, "struct");
    }

    #[test]
    fn python_and_ts_outline() {
        let py = outline("def f():\n    pass\nclass C:\n    def m(self):\n        pass\n", "py", 100).unwrap();
        let pn: Vec<&str> = py.iter().map(|s| s.name.as_str()).collect();
        assert!(pn.contains(&"f") && pn.contains(&"C") && pn.contains(&"m"), "py 大纲: {py:?}");

        let ts = outline("export interface I { x: number }\nexport function g() {}\nclass K {}\n", "ts", 100).unwrap();
        let tn: Vec<&str> = ts.iter().map(|s| s.name.as_str()).collect();
        assert!(tn.contains(&"I") && tn.contains(&"g") && tn.contains(&"K"), "ts 大纲: {ts:?}");
    }

    #[test]
    fn unknown_ext_has_no_grammar() {
        assert!(outline("whatever", "xyz", 100).is_none(), "无 grammar 应返回 None(退文本层)");
        assert!(!supports("xyz") && supports("rs") && supports("tsx"));
    }

    #[test]
    fn respects_max_cap() {
        let src = "fn a(){}\nfn b(){}\nfn c(){}\n";
        let syms = outline(src, "rs", 2).unwrap();
        assert_eq!(syms.len(), 2, "应被 max 截断");
    }
}
