//! Source analysis: which constructs in the user's file the instrumenter will
//! touch, found through libclang. Produces facts (positions, names); contains no
//! text generation (see `rewrite`).
//!
//! Only the main file is walked: header subtrees are skipped wholesale, which is
//! what makes libclang practical where a textual AST dump is not (a program that
//! merely includes `<iostream>` produces a ~230 MB JSON dump).
//!
//! Anything ambiguous is skipped rather than guessed: an un-instrumented construct
//! loses observations; a wrongly instrumented one could change the program.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::libclang::{type_kind, Clang, Cursor, CHILD_CONTINUE};

/// A class/struct to describe to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSite {
    /// Qualified name as written in C++ (`ns::Node`).
    pub name: String,
    /// Public, non-bit-field, non-reference data members in declaration order.
    pub fields: Vec<String>,
    /// Byte offset just after the `;` that ends the definition.
    pub insert_at: usize,
}

/// A non-placement, non-array `new` expression of a nameable type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSite {
    pub start: usize,
    pub end: usize,
    /// Offset just after the `new` keyword.
    pub kw_end: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
    /// The allocated type as C++ spells it (`Node`, `int`).
    pub ty: String,
}

/// A non-array `delete` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteSite {
    pub start: usize,
    pub end: usize,
    /// Offset just after the `delete` keyword.
    pub kw_end: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
}

/// A built-in assignment (`=`, `+=`, ...) or prefix `++`/`--` whose operand is a
/// scalar, enum or pointer lvalue: the expression is itself an lvalue, so it can
/// be wrapped without changing its type or value category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignSite {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
}

/// A postfix `x++` / `x--` on a scalar or pointer. Its value is the *old* value, so
/// it cannot be wrapped as an lvalue; the operand (side-effect free, so safe to
/// mention twice) is repeated to observe the variable afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostfixSite {
    pub start: usize,
    pub end: usize,
    pub operand_start: usize,
    pub operand_end: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
}

/// How a variable is reported to the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Initialized: its current value is read.
    Value,
    /// Declared without an initializer: storage exists, content is indeterminate.
    Uninit,
    /// A reference: its value is the thing it is bound to.
    Ref,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarHook {
    pub name: String,
    pub kind: HookKind,
}

/// A function definition with a body we can instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSite {
    /// Name as shown (`main`, `Node::Node`).
    pub name: String,
    /// Offset just after the `{` that opens the body.
    pub open: usize,
    pub line: u32,
    pub column: u32,
    /// Named parameters, in order.
    pub params: Vec<VarHook>,
}

/// A `{` block that declares variables and therefore needs a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSite {
    /// Offset just after the `{`.
    pub open: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
}

/// A declaration statement of variables to report once they exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSite {
    /// Offset just after the statement's `;`.
    pub insert_at: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
    pub vars: Vec<VarHook>,
}

/// `for (int i = 0; cond; inc) body`: the loop variables live in a scope of their
/// own and are reported from the condition (evaluated once the first iteration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForSite {
    pub start: usize,
    pub end: usize,
    pub cond_start: usize,
    pub cond_end: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
    pub vars: Vec<VarHook>,
}

/// `for (T x : range) { body }`: `x` is a fresh variable on every iteration, so it
/// is reported at the top of the body (which gets its own scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeForSite {
    /// Offset just after the body's `{`.
    pub body_open: usize,
    pub line: u32,
    pub column: u32,
    pub function: String,
    pub var: VarHook,
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// Compile errors libclang found in the user's own code. If non-empty, nothing
    /// should be rewritten: the real compiler will report them in its own words.
    pub errors: Vec<String>,
    /// Errors inside system headers that libclang could not digest and that were
    /// set aside (the user's code is still analysed).
    pub ignored_system_errors: usize,
    pub records: Vec<RecordSite>,
    pub news: Vec<NewSite>,
    pub deletes: Vec<DeleteSite>,
    pub assigns: Vec<AssignSite>,
    pub postfixes: Vec<PostfixSite>,
    pub functions: Vec<FunctionSite>,
    pub blocks: Vec<BlockSite>,
    pub locals: Vec<LocalSite>,
    pub fors: Vec<ForSite>,
    pub range_fors: Vec<RangeForSite>,
}

#[derive(Default)]
struct Ctx {
    namespaces: Vec<String>,
    function: String,
    /// Types with a class-specific `operator new`: our placement form would be
    /// hidden by it, so `new` of these is left alone.
    custom_alloc: HashSet<String>,
    /// Inside a lambda body: no frames or variable hooks there (yet).
    in_lambda: u32,
    /// The innermost enclosing function gets a frame, so hooks that need one are fine.
    frame_ok: bool,
    /// Blocks that need a scope, by the offset after their `{` (deduplicated).
    block_opens: BTreeMap<usize, BlockSite>,
    out: Analysis,
}

pub fn analyze(clang: &Clang, file: &Path, source: &str, args: &[String]) -> Result<Analysis, String> {
    let tu = clang.parse(file, args)?;
    let mut ctx = Ctx::default();
    (ctx.out.errors, ctx.out.ignored_system_errors) = tu.errors();
    if !ctx.out.errors.is_empty() {
        return Ok(ctx.out);
    }
    tu.root().visit_children(|c| {
        if c.is_from_main_file() {
            walk(c, "TranslationUnit", source, &mut ctx);
        }
        CHILD_CONTINUE
    });
    ctx.out.blocks = std::mem::take(&mut ctx.block_opens).into_values().collect();
    Ok(ctx.out)
}

const FUNCTION_KINDS: [&str; 5] = ["FunctionDecl", "CXXMethod", "CXXConstructor", "CXXDestructor", "CXXConversion"];

fn walk(c: Cursor<'_>, parent: &str, src: &str, ctx: &mut Ctx) {
    if !c.is_from_main_file() {
        return;
    }
    let kind = c.kind_name();
    match kind.as_str() {
        // Templates: bodies depend on template parameters; not modelled yet.
        "FunctionTemplate" | "ClassTemplate" | "ClassTemplatePartialSpecialization" => return,
        "Namespace" => {
            ctx.namespaces.push(c.spelling());
            recurse(c, &kind, src, ctx);
            ctx.namespaces.pop();
            return;
        }
        "StructDecl" | "ClassDecl" => record(c, src, ctx),
        k if FUNCTION_KINDS.contains(&k) => {
            function(c, &kind, src, ctx);
            return;
        }
        "LambdaExpr" => {
            ctx.in_lambda += 1;
            recurse(c, &kind, src, ctx);
            ctx.in_lambda -= 1;
            return;
        }
        "CompoundStmt" => block(c, parent, src, ctx),
        "ForStmt" => for_stmt(c, src, ctx),
        "CXXForRangeStmt" => range_for(c, src, ctx),
        "CXXNewExpr" => new_expr(c, src, ctx),
        "CXXDeleteExpr" => delete_expr(c, src, ctx),
        "BinaryOperator" | "CompoundAssignOperator" => assignment(c, ctx),
        "UnaryOperator" => unary(c, src, ctx),
        _ => {}
    }
    recurse(c, &kind, src, ctx);
}

fn recurse(c: Cursor<'_>, kind: &str, src: &str, ctx: &mut Ctx) {
    c.visit_children(|child| {
        walk(child, kind, src, ctx);
        CHILD_CONTINUE
    });
}

/// `constexpr`/`consteval` functions may run at compile time, where our calls
/// into the runtime are not allowed.
fn is_compile_time_function(c: Cursor<'_>, src: &str) -> bool {
    let Some((s, e)) = c.plain_range() else { return false };
    let Some(text) = src.get(s.offset..e.offset) else { return false };
    let head = text.split('{').next().unwrap_or(text);
    head.contains("constexpr") || head.contains("consteval")
}

fn qualified(ctx: &Ctx, name: &str) -> String {
    let mut q = ctx.namespaces.join("::");
    if !q.is_empty() {
        q.push_str("::");
    }
    q.push_str(name);
    q
}

fn is_scalar_kind(k: i32) -> bool {
    (type_kind::BOOL..=type_kind::LONG_DOUBLE).contains(&k) || k == type_kind::POINTER || k == type_kind::ENUM
}

// ---- functions, blocks, variables ----------------------------------------------

/// How (or whether) a variable declaration is reported.
fn hook_for_var(var: Cursor<'_>, src: &str) -> Option<VarHook> {
    if var.kind_name() != "VarDecl" {
        return None; // structured bindings, typedefs, local classes...
    }
    // Only automatic storage: not `static`/`extern`, not `thread_local`.
    if matches!(var.storage_class(), 2 | 3 | 4) || var.tls_kind() != 0 {
        return None;
    }
    let name = var.spelling();
    if name.is_empty() {
        return None;
    }
    let ty = var.ty().canonical();
    let k = ty.kind();
    if matches!(k, 0 | 1 | type_kind::DEPENDENT | type_kind::VARIABLE_ARRAY | type_kind::INCOMPLETE_ARRAY) {
        return None;
    }
    if k == type_kind::LVALUE_REF || k == type_kind::RVALUE_REF {
        return Some(VarHook { name, kind: HookKind::Ref });
    }
    // `int x;` / `int a[3];` have storage but no value yet. Class types are
    // constructed, so they are always read.
    let scalar_like = is_scalar_kind(k)
        || (k == type_kind::CONSTANT_ARRAY && is_scalar_kind(ty.array_element().canonical().kind()));
    let uninit = scalar_like && !has_initializer(var, &name, src);
    Some(VarHook { name, kind: if uninit { HookKind::Uninit } else { HookKind::Value } })
}

/// Is there anything after the declarator name (other than array bounds)?
fn has_initializer(var: Cursor<'_>, name: &str, src: &str) -> bool {
    let Some((s, e)) = var.plain_range() else { return true };
    let Some(text) = src.get(s.offset..e.offset) else { return true };
    let Some(pos) = whole_word(text, name) else { return true };
    let mut rest = text[pos + name.len()..].trim_start();
    while rest.starts_with('[') {
        let mut depth = 0;
        let mut end = None;
        for (i, ch) in rest.char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(i) => rest = rest[i + 1..].trim_start(),
            None => return true,
        }
    }
    !rest.is_empty()
}

fn whole_word(text: &str, word: &str) -> Option<usize> {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = start == 0 || !ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !ident(bytes[end]);
        if before_ok && after_ok {
            return Some(start);
        }
        from = start + 1;
    }
    None
}

fn function(c: Cursor<'_>, kind: &str, src: &str, ctx: &mut Ctx) {
    let name = c.spelling();
    let display = if kind == "FunctionDecl" {
        name.clone()
    } else {
        let parent = c.semantic_parent();
        let pn = parent.spelling();
        if pn.is_empty() { name.clone() } else { format!("{pn}::{name}") }
    };
    let saved_fn = std::mem::replace(&mut ctx.function, display.clone());
    let saved_lambda = std::mem::replace(&mut ctx.in_lambda, 0);
    let saved_ok = ctx.frame_ok;
    ctx.frame_ok = false;

    if !is_compile_time_function(c, src) {
        // The body is the last child, and must be a plain `{ ... }` (not a
        // function-try-block).
        let kids = c.children();
        let body = kids.iter().rev().find(|k| k.kind_name() == "CompoundStmt");
        let site = body.and_then(|b| {
            let (bs, _) = b.plain_range()?;
            let (fs, _) = c.plain_range()?;
            if src.as_bytes().get(bs.offset) != Some(&b'{') {
                return None;
            }
            // A try-block body is preceded by `try` before the brace.
            let head = src.get(fs.offset..bs.offset)?.trim_end();
            if head.ends_with("try") {
                return None;
            }
            let params = kids
                .iter()
                .filter(|k| k.kind_name() == "ParmDecl")
                .filter_map(|p| {
                    let n = p.spelling();
                    let k = p.ty().canonical().kind();
                    if n.is_empty() || matches!(k, 0 | 1 | type_kind::DEPENDENT) {
                        return None;
                    }
                    let is_ref = k == type_kind::LVALUE_REF || k == type_kind::RVALUE_REF;
                    Some(VarHook { name: n, kind: if is_ref { HookKind::Ref } else { HookKind::Value } })
                })
                .collect();
            Some(FunctionSite { name: display.clone(), open: bs.offset + 1, line: fs.line, column: fs.column, params })
        });
        if let Some(site) = site {
            ctx.out.functions.push(site);
            ctx.frame_ok = true;
        }
        recurse(c, kind, src, ctx);
    }

    ctx.frame_ok = saved_ok;
    ctx.in_lambda = saved_lambda;
    ctx.function = saved_fn;
}

fn local_site(decl: Cursor<'_>, src: &str, ctx: &Ctx) -> Option<LocalSite> {
    let (s, e) = decl.plain_range()?;
    if !src.get(..e.offset)?.ends_with(';') {
        return None;
    }
    let vars: Vec<VarHook> = decl.children().into_iter().filter_map(|v| hook_for_var(v, src)).collect();
    if vars.is_empty() {
        return None;
    }
    Some(LocalSite { insert_at: e.offset, line: s.line, column: s.column, function: ctx.function.clone(), vars })
}

fn block(c: Cursor<'_>, parent: &str, src: &str, ctx: &mut Ctx) {
    if !ctx.frame_ok || ctx.in_lambda > 0 {
        return;
    }
    let kids = c.children();
    let mut hooked = false;
    for k in &kids {
        if k.kind_name() == "DeclStmt" {
            if let Some(site) = local_site(*k, src, ctx) {
                ctx.out.locals.push(site);
                hooked = true;
            }
        }
    }
    let is_function_body = FUNCTION_KINDS.contains(&parent); // the frame is its scope
    // A scope guard before a `case`/label would be jumped over: ill-formed.
    let jump_targets = kids
        .iter()
        .any(|k| matches!(k.kind_name().as_str(), "CaseStmt" | "DefaultStmt" | "LabelStmt"));
    if hooked && !is_function_body && parent != "SwitchStmt" && !jump_targets {
        add_block(c, src, ctx);
    }
}

fn add_block(body: Cursor<'_>, src: &str, ctx: &mut Ctx) -> Option<usize> {
    let (s, _) = body.plain_range()?;
    if src.as_bytes().get(s.offset) != Some(&b'{') {
        return None;
    }
    let open = s.offset + 1;
    ctx.block_opens.entry(open).or_insert_with(|| BlockSite {
        open,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
    });
    Some(open)
}

fn for_stmt(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    if !ctx.frame_ok || ctx.in_lambda > 0 {
        return;
    }
    // Only the complete form `for (decl; cond; inc) body`: with a part missing,
    // libclang's child list no longer says which is which.
    let kids = c.children();
    let [init, cond, inc, _body] = kids.as_slice() else { return };
    if init.kind_name() != "DeclStmt" || !cond.is_expression() || !inc.is_expression() {
        return;
    }
    let vars: Vec<VarHook> = init.children().into_iter().filter_map(|v| hook_for_var(v, src)).collect();
    if vars.is_empty() {
        return;
    }
    let (Some((s, e)), Some((cs, ce))) = (c.plain_range(), cond.plain_range()) else { return };
    ctx.out.fors.push(ForSite {
        start: s.offset,
        end: e.offset,
        cond_start: cs.offset,
        cond_end: ce.offset,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
        vars,
    });
}

fn range_for(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    if !ctx.frame_ok || ctx.in_lambda > 0 {
        return;
    }
    let kids = c.children();
    let [var, _range, body] = kids.as_slice() else { return };
    if var.kind_name() != "VarDecl" || body.kind_name() != "CompoundStmt" {
        return;
    }
    let Some(hook) = hook_for_var(*var, src) else { return };
    let Some(open) = add_block(*body, src, ctx) else { return };
    let Some((s, _)) = c.plain_range() else { return };
    ctx.out.range_fors.push(RangeForSite {
        body_open: open,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
        var: hook,
    });
}

// ---- records, allocation, writes -------------------------------------------------

fn record(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    let name = c.spelling();
    // Custom allocation functions are noted even if the record is not described.
    for m in c.children() {
        if m.kind_name() == "CXXMethod" && m.spelling() == "operator new" {
            ctx.custom_alloc.insert(name.clone());
            ctx.custom_alloc.insert(qualified(ctx, &name));
        }
    }
    if !c.is_definition() || name.is_empty() || name.contains(['<', '(', ' ']) {
        return;
    }
    // Namespace-scope only: a descriptor is a free function next to the type.
    let parent = c.semantic_parent().kind_name();
    if parent != "TranslationUnit" && parent != "Namespace" {
        return;
    }
    let Some((_, end)) = c.plain_range() else { return };
    // `struct X {...};` only. With declarators (`struct X {...} x;`) skip.
    let after = &src[end.offset.min(src.len())..];
    let trimmed = after.trim_start();
    if !trimmed.starts_with(';') {
        return;
    }
    let insert_at = end.offset + (after.len() - trimmed.len()) + 1;

    let mut fields = Vec::new();
    for m in c.children() {
        if m.kind_name() != "FieldDecl" || m.access() != 1 || m.is_bitfield() {
            continue;
        }
        let fname = m.spelling();
        let canon = m.ty().canonical().kind();
        if fname.is_empty()
            || canon == type_kind::LVALUE_REF
            || canon == type_kind::RVALUE_REF
            || canon == type_kind::INCOMPLETE_ARRAY
        {
            continue;
        }
        fields.push(fname);
    }
    ctx.out.records.push(RecordSite { name: qualified(ctx, &name), fields, insert_at });
}

fn new_expr(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    let Some((s, e)) = c.plain_range() else { return };
    let Some(text) = src.get(s.offset..e.offset) else { return };
    let Some(rest) = text.strip_prefix("new") else { return };
    let rest = rest.trim_start();
    // Placement / nothrow / parenthesized type: leave alone.
    if rest.starts_with('(') {
        return;
    }
    // `new T[n]`: arrays are a later milestone. Ambiguity resolves to "skip".
    if matches!(rest.find(['(', '{', '[']).map(|i| rest.as_bytes()[i]), Some(b'[')) {
        return;
    }
    let pointee = c.ty().pointee();
    let ty = pointee.spelling();
    if ty.is_empty() || ty.contains(['(', '\'']) || ty.contains("unnamed") || ty.contains("anonymous") {
        return;
    }
    let bare = ty.trim_start_matches("const ").trim_start_matches("struct ").trim_start_matches("class ");
    if ctx.custom_alloc.contains(bare) {
        return;
    }
    ctx.out.news.push(NewSite {
        start: s.offset,
        end: e.offset,
        kw_end: s.offset + 3,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
        ty,
    });
}

fn delete_expr(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    let Some((s, e)) = c.plain_range() else { return };
    let Some(text) = src.get(s.offset..e.offset) else { return };
    let Some(rest) = text.strip_prefix("delete") else { return };
    if rest.trim_start().starts_with('[') {
        return; // delete[]
    }
    ctx.out.deletes.push(DeleteSite {
        start: s.offset,
        end: e.offset,
        kw_end: s.offset + 6,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
    });
}

/// Writes to `volatile` objects are left alone: they are hardware or concurrency
/// flags, meaningless to visualize, and the C++20 rules deprecate using their value.
fn is_volatile(operand: &Cursor<'_>) -> bool {
    operand.ty().spelling().contains("volatile")
}

/// `&(x = y)` is ill-formed for a bit-field.
fn is_bitfield_lvalue(operand: &Cursor<'_>) -> bool {
    let target = operand.referenced();
    target.kind_name() == "FieldDecl" && target.is_bitfield()
}

fn assignment(c: Cursor<'_>, ctx: &mut Ctx) {
    // `=` and every compound assignment (`+=`, `<<=`, ...): not `==`, `<=`, ...
    let Some(op) = c.binary_operator() else { return };
    if !op.ends_with('=') || matches!(op.as_str(), "==" | "!=" | "<=" | ">=") {
        return;
    }
    let kids = c.children();
    let [lhs, _rhs] = kids.as_slice() else { return };
    if !is_scalar_kind(lhs.ty().canonical().kind()) || is_bitfield_lvalue(lhs) || is_volatile(lhs) {
        return;
    }
    let Some((s, e)) = c.plain_range() else { return };
    ctx.out.assigns.push(AssignSite {
        start: s.offset,
        end: e.offset,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
    });
}

/// Evaluating the expression again would do something beyond reading.
fn has_side_effects(c: Cursor<'_>) -> bool {
    let kind = c.kind_name();
    let impure = match kind.as_str() {
        "CallExpr" | "CompoundAssignOperator" | "CXXNewExpr" | "CXXDeleteExpr" | "LambdaExpr" => true,
        "UnaryOperator" => matches!(c.unary_operator(), 1..=4),
        "BinaryOperator" => c.binary_operator().as_deref() == Some("="),
        _ => false,
    };
    impure || c.children().into_iter().any(has_side_effects)
}

fn unary(c: Cursor<'_>, src: &str, ctx: &mut Ctx) {
    let kind = c.unary_operator();
    if !(1..=4).contains(&kind) {
        return;
    }
    let kids = c.children();
    let [operand] = kids.as_slice() else { return };
    let k = operand.ty().canonical().kind();
    // `bool++` is not valid C++; enums have no `++`.
    if !(((type_kind::BOOL + 1)..=type_kind::LONG_DOUBLE).contains(&k) || k == type_kind::POINTER)
        || is_bitfield_lvalue(operand)
        || is_volatile(operand)
    {
        return;
    }
    let Some((s, e)) = c.plain_range() else { return };
    if kind >= 3 {
        // Prefix: the result is the operand lvalue itself.
        ctx.out.assigns.push(AssignSite {
            start: s.offset,
            end: e.offset,
            line: s.line,
            column: s.column,
            function: ctx.function.clone(),
        });
        return;
    }
    if has_side_effects(*operand) {
        return; // `a[i++]++`: repeating the operand would repeat `i++`
    }
    let Some((os, oe)) = operand.plain_range() else { return };
    if src.get(os.offset..oe.offset).is_none() {
        return;
    }
    ctx.out.postfixes.push(PostfixSite {
        start: s.offset,
        end: e.offset,
        operand_start: os.offset,
        operand_end: oe.offset,
        line: s.line,
        column: s.column,
        function: ctx.function.clone(),
    });
}
