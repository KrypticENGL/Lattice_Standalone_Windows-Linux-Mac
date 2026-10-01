//! Turns an [`Analysis`] into instrumented source text.
//!
//! Only *insertions*, never deletions or moves, so:
//! - the user's original tokens are all still there, in the same order;
//! - no newline is ever added, so every line number still matches the user's
//!   source (compiler diagnostics and runtime locations stay truthful);
//! - semantics are preserved by construction: each wrapper returns exactly the
//!   value (and value category) of the expression it wraps.
//!
//! ```text
//! new Node{10, nullptr}
//!   -> ::lattice::rt::constructed(new (::lattice::rt::tag<Node>(SITE)) Node{10, nullptr})
//! a->next = b            (also +=, -=, ..., and prefix ++/--)
//!   -> ::lattice::rt::written(&(a->next = b), SITE)
//! i++
//!   -> ::lattice::rt::post_written(i++, &(i), SITE)
//! delete b
//!   -> delete ::lattice::rt::del_hint( b, SITE)
//! struct Node {...};
//!   -> struct Node {...}; <descriptor function for Node>
//! int f(int a) {            ->   int f(int a) { Frame guard; param(a...);
//!     int x = 1;            ->       int x = 1; local(x...);
//!     { int y; }            ->       { Scope guard; int y; local_uninit(y...); }
//!     for (int i = 0; c; ..)->   { Scope guard; for (int i = 0; (local(i...), c); ..) }
//! }
//! ```

use super::analysis::{
    Analysis, AssignSite, BlockSite, DeleteSite, ForSite, FunctionSite, HookKind, LocalSite, NewSite,
    PostfixSite, RangeForSite, RecordSite, VarHook,
};

struct Insertion {
    pos: usize,
    /// At equal `pos`, closers (0) are emitted before openers (1).
    group: u8,
    /// Within a group: smaller first.
    key: i64,
    text: String,
}

// Structural insertions (guards, variable hooks) must come before any expression
// wrapper that happens to open at the same offset. Wrapper keys are -(span length),
// always far above these.
const KEY_FRAME: i64 = i64::MIN;
const KEY_SCOPE: i64 = i64::MIN + 1;
const KEY_BODY_HOOK: i64 = i64::MIN + 2;
const KEY_LOCAL: i64 = i64::MIN + 3;

fn cpp_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            _ => o.push(c),
        }
    }
    o.push('"');
    o
}

fn site(file: &str, line: u32, column: u32, function: &str) -> String {
    format!("::lattice::rt::Site{{{},{line},{column},{}}}", cpp_string(file), cpp_string(function))
}

fn descriptor(r: &RecordSite) -> String {
    let n = &r.name;
    let mut s = String::new();
    s.push_str(" LATTICE_DIAG_PUSH");
    s.push_str(&format!(" inline void lattice_rt_describe(::lattice::rt::TypeId __id, const {n}*) {{"));
    s.push_str(&format!(" ::lattice::rt::detail::set_name(__id, {}, {});", cpp_string(n), n.len()));
    if r.fields.is_empty() {
        s.push_str(&format!(" ::lattice::rt::detail::define_record(__id, sizeof({n}), nullptr, 0);"));
    } else {
        s.push_str(" const ::lattice::rt::FieldInfo __f[] = {");
        for (i, f) in r.fields.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "{{{}, ::lattice::rt::type_id<decltype({n}::{f})>(), offsetof({n}, {f})}}",
                cpp_string(f)
            ));
        }
        s.push_str(&format!(" }}; ::lattice::rt::detail::define_record(__id, sizeof({n}), __f, {});", r.fields.len()));
    }
    s.push_str(" } LATTICE_DIAG_POP");
    s
}

fn push_span(out: &mut Vec<Insertion>, start: usize, end: usize, open: String, close: String) {
    // Outer spans open first (longer first) and close last (shorter first).
    out.push(Insertion { pos: start, group: 1, key: -((end - start) as i64), text: open });
    out.push(Insertion { pos: end, group: 0, key: -(start as i64), text: close });
}

fn new_edits(out: &mut Vec<Insertion>, file: &str, n: &NewSite) {
    let s = site(file, n.line, n.column, &n.function);
    push_span(out, n.start, n.end, "::lattice::rt::constructed(".into(), ")".into());
    out.push(Insertion {
        pos: n.kw_end,
        group: 1,
        key: 0,
        text: format!(" (::lattice::rt::tag<{}>({s}))", n.ty),
    });
}

fn delete_edits(out: &mut Vec<Insertion>, file: &str, d: &DeleteSite) {
    let s = site(file, d.line, d.column, &d.function);
    out.push(Insertion { pos: d.kw_end, group: 1, key: 0, text: " ::lattice::rt::del_hint(".into() });
    out.push(Insertion { pos: d.end, group: 0, key: -(d.start as i64), text: format!(", {s})") });
}

fn assign_edits(out: &mut Vec<Insertion>, file: &str, a: &AssignSite) {
    let s = site(file, a.line, a.column, &a.function);
    push_span(out, a.start, a.end, "::lattice::rt::written(&(".into(), format!("), {s})"));
}

fn postfix_edits(out: &mut Vec<Insertion>, source: &str, file: &str, p: &PostfixSite) {
    let s = site(file, p.line, p.column, &p.function);
    let operand = &source[p.operand_start..p.operand_end];
    push_span(
        out,
        p.start,
        p.end,
        "::lattice::rt::post_written(".into(),
        format!(", &({operand}), {s})"),
    );
}

/// `::lattice::rt::local(x, "x", SITE)` (a call, no trailing `;`).
fn hook_call(v: &VarHook, site: &str, parameter: bool) -> String {
    let f = match (v.kind, parameter) {
        (HookKind::Value, false) => "local",
        (HookKind::Uninit, false) => "local_uninit",
        (HookKind::Ref, false) => "local_ref",
        (HookKind::Value | HookKind::Uninit, true) => "param",
        (HookKind::Ref, true) => "param_ref",
    };
    format!("::lattice::rt::{f}({0}, {1}, {site})", v.name, cpp_string(&v.name))
}

/// One hook statement per variable.
fn hooks(vars: &[VarHook], site: &str, parameter: bool) -> String {
    vars.iter().map(|v| format!(" {};", hook_call(v, site, parameter))).collect()
}

fn function_edits(out: &mut Vec<Insertion>, file: &str, f: &FunctionSite) {
    let s = site(file, f.line, f.column, &f.name);
    let mut text = format!(" ::lattice::rt::Frame __lattice_frame({s}, {});", cpp_string(&f.name));
    text.push_str(&hooks(&f.params, &s, true));
    out.push(Insertion { pos: f.open, group: 1, key: KEY_FRAME, text });
}

fn block_edits(out: &mut Vec<Insertion>, file: &str, b: &BlockSite) {
    let s = site(file, b.line, b.column, &b.function);
    out.push(Insertion {
        pos: b.open,
        group: 1,
        key: KEY_SCOPE,
        text: format!(" ::lattice::rt::Scope __lattice_scope({s});"),
    });
}

fn local_edits(out: &mut Vec<Insertion>, file: &str, l: &LocalSite) {
    let s = site(file, l.line, l.column, &l.function);
    out.push(Insertion { pos: l.insert_at, group: 1, key: KEY_LOCAL, text: hooks(&l.vars, &s, false) });
}

fn for_edits(out: &mut Vec<Insertion>, file: &str, f: &ForSite) {
    let s = site(file, f.line, f.column, &f.function);
    // The loop variables get a scope of their own: the whole `for` goes in a block.
    push_span(
        out,
        f.start,
        f.end,
        format!("{{ ::lattice::rt::Scope __lattice_scope({s}); "),
        " }".into(),
    );
    // Reported from the condition: `(void, cond)` has the condition's value.
    let calls: Vec<String> = f.vars.iter().map(|v| hook_call(v, &s, false)).collect();
    push_span(out, f.cond_start, f.cond_end, format!("({}, ", calls.join(", ")), ")".into());
}

fn range_for_edits(out: &mut Vec<Insertion>, file: &str, r: &RangeForSite) {
    let s = site(file, r.line, r.column, &r.function);
    out.push(Insertion {
        pos: r.body_open,
        group: 1,
        key: KEY_BODY_HOOK,
        text: hooks(std::slice::from_ref(&r.var), &s, false),
    });
}

/// Apply the instrumentation described by `analysis` to `source`.
/// `file` is the name recorded in event locations (the user's file name).
pub fn instrument(source: &str, file: &str, analysis: &Analysis) -> String {
    let mut ins: Vec<Insertion> = Vec::new();
    for r in &analysis.records {
        ins.push(Insertion { pos: r.insert_at, group: 1, key: 0, text: descriptor(r) });
    }
    for n in &analysis.news {
        new_edits(&mut ins, file, n);
    }
    for d in &analysis.deletes {
        delete_edits(&mut ins, file, d);
    }
    for a in &analysis.assigns {
        assign_edits(&mut ins, file, a);
    }
    for p in &analysis.postfixes {
        postfix_edits(&mut ins, source, file, p);
    }
    for f in &analysis.functions {
        function_edits(&mut ins, file, f);
    }
    for b in &analysis.blocks {
        block_edits(&mut ins, file, b);
    }
    for l in &analysis.locals {
        local_edits(&mut ins, file, l);
    }
    for f in &analysis.fors {
        for_edits(&mut ins, file, f);
    }
    for r in &analysis.range_fors {
        range_for_edits(&mut ins, file, r);
    }
    ins.retain(|i| i.pos <= source.len() && source.is_char_boundary(i.pos));
    ins.sort_by(|a, b| (a.pos, a.group, a.key).cmp(&(b.pos, b.group, b.key)));

    let mut out = String::with_capacity(source.len() + ins.len() * 48);
    let mut at = 0;
    for i in &ins {
        out.push_str(&source[at..i.pos]);
        out.push_str(&i.text);
        at = i.pos;
    }
    out.push_str(&source[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::analysis::*;

    fn a(start: usize, end: usize) -> AssignSite {
        AssignSite { start, end, line: 1, column: 1, function: "f".into() }
    }

    #[test]
    fn nested_wrappers_nest_correctly() {
        // x = (y = 1)   -> outer assignment 0..11, inner 5..10 (inside the parens)
        let src = "x = (y = 1);";
        let an = Analysis { assigns: vec![a(0, 11), a(5, 10)], ..Analysis::default() };
        let out = instrument(src, "m.cpp", &an);
        assert!(out.starts_with("::lattice::rt::written(&(x = (::lattice::rt::written(&(y = 1), "), "{out}");
        assert!(out.ends_with(");"), "{out}");
        assert_eq!(out.matches("written(").count(), 2);
        assert_eq!(out.matches('(').count(), out.matches(')').count());
    }

    #[test]
    fn rewriting_never_changes_the_line_count() {
        let src = "struct N { int v; };\nint main() {\n  N* p = new N{1};\n  p->v = 2;\n  delete p;\n}\n";
        let an = Analysis {
            records: vec![RecordSite { name: "N".into(), fields: vec!["v".into()], insert_at: 20 }],
            news: vec![NewSite {
                start: src.find("new N").unwrap(),
                end: src.find("new N").unwrap() + 8,
                kw_end: src.find("new N").unwrap() + 3,
                line: 3,
                column: 10,
                function: "main".into(),
                ty: "N".into(),
            }],
            deletes: vec![DeleteSite {
                start: src.find("delete").unwrap(),
                end: src.find("delete p").unwrap() + 8,
                kw_end: src.find("delete").unwrap() + 6,
                line: 5,
                column: 3,
                function: "main".into(),
            }],
            assigns: vec![a(src.find("p->v").unwrap(), src.find("p->v").unwrap() + 8)],
            ..Analysis::default()
        };
        let out = instrument(src, "main.cpp", &an);
        assert_eq!(out.lines().count(), src.lines().count());
        assert!(out.contains("constructed(new (::lattice::rt::tag<N>"));
        assert!(out.contains("delete ::lattice::rt::del_hint( p, "));
        assert!(out.contains("lattice_rt_describe(::lattice::rt::TypeId __id, const N*)"));
    }

    #[test]
    fn original_tokens_survive_untouched() {
        let src = "p->v = 2;";
        let an = Analysis { assigns: vec![a(0, 8)], ..Analysis::default() };
        let out = instrument(src, "m.cpp", &an);
        let stripped = out.replace("::lattice::rt::written(&(", "");
        assert!(stripped.starts_with("p->v = 2"));
    }

    #[test]
    fn guards_precede_expression_wrappers_at_the_same_offset() {
        // `{x = 1;}`: the scope guard goes right after `{`, before anything that
        // starts at the next token.
        let src = "{x = 1;}";
        let an = Analysis {
            assigns: vec![a(1, 6)],
            blocks: vec![BlockSite { open: 1, line: 1, column: 1, function: "f".into() }],
            ..Analysis::default()
        };
        let out = instrument(src, "m.cpp", &an);
        let guard = out.find("Scope __lattice_scope").unwrap();
        let write = out.find("written(").unwrap();
        assert!(guard < write, "{out}");
    }

    #[test]
    fn postfix_repeats_its_operand_and_keeps_the_original() {
        let src = "y = x++;";
        let an = Analysis {
            postfixes: vec![PostfixSite {
                start: 4,
                end: 7,
                operand_start: 4,
                operand_end: 5,
                line: 1,
                column: 5,
                function: "f".into(),
            }],
            ..Analysis::default()
        };
        let out = instrument(src, "m.cpp", &an);
        assert!(out.starts_with("y = ::lattice::rt::post_written(x++, &(x), "), "{out}");
    }

    #[test]
    fn a_for_loop_gets_a_block_and_a_condition_hook() {
        let src = "for (int i = 0; i < 3; i++) {}";
        let an = Analysis {
            fors: vec![ForSite {
                start: 0,
                end: src.len(),
                cond_start: 16,
                cond_end: 21,
                line: 1,
                column: 1,
                function: "f".into(),
                vars: vec![VarHook { name: "i".into(), kind: HookKind::Value }],
            }],
            ..Analysis::default()
        };
        let out = instrument(src, "m.cpp", &an);
        assert!(out.starts_with("{ ::lattice::rt::Scope __lattice_scope("), "{out}");
        assert!(out.contains("(::lattice::rt::local(i, \"i\", "), "{out}");
        assert!(out.contains(" i < 3)"), "{out}");
        assert!(out.ends_with(" }"), "{out}");
        assert_eq!(out.lines().count(), 1);
    }
}
