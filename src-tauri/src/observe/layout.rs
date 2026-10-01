//! Layouts of types the runtime cannot describe by itself: library classes
//! (`std::vector<int>`, `std::string`, smart pointers...) and project templates.
//!
//! libclang knows the exact layout of every complete type the program uses, including
//! instantiated templates and private members. This module reads it and produces plain
//! data (a graph of nodes: primitives, pointers, arrays, records with named fields and
//! byte offsets). The rewriter emits that data next to the code that uses the type; the
//! runtime turns it into the same type descriptions it builds for user records.
//!
//! Nothing here knows a type by name. A `std::vector<int>`, a `std::list<Node>` and a
//! user's `Box<T>` all go through the same walk, and the walk gives up (an opaque node of
//! the right size) rather than guess whenever the layout is not plain: virtual functions,
//! virtual or several bases, bit-fields, incomplete types.

use std::collections::HashMap;

use super::libclang::{type_kind, Cursor, Type};

/// Nodes in one layout. Beyond this the remainder is described only by size.
const MAX_NODES: usize = 256;
/// Levels of nesting followed from the root.
const MAX_DEPTH: u32 = 10;
/// Same bound the runtime uses for one array's elements.
const MAX_ARRAY: i64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A built-in scalar; `prim` names the runtime's `Prim`.
    Prim,
    Enum,
    /// `reference` is the pointee node.
    Pointer,
    /// `reference` is the element node, `count` the length.
    Array,
    Record,
    /// Known only by name and size.
    Opaque,
    /// A project record that has its own description: `reference` indexes `externals`.
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutField {
    pub name: String,
    pub offset: u64,
    pub ty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutNode {
    pub kind: NodeKind,
    /// `Prim` variant name (`Int`, `Char`, ...) for `Prim`.
    pub prim: &'static str,
    pub signed: bool,
    pub size: u64,
    pub name: String,
    pub reference: u32,
    pub count: u64,
    pub fields: Vec<LayoutField>,
}

impl LayoutNode {
    fn new(kind: NodeKind, size: u64, name: impl Into<String>) -> Self {
        LayoutNode { kind, prim: "Int", signed: false, size, name: name.into(), reference: 0, count: 0, fields: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub nodes: Vec<LayoutNode>,
    pub root: u32,
    /// C++ spellings of the project types the layout refers to, to be named at the
    /// place the layout is used (where they are in scope).
    pub externals: Vec<String>,
}

/// Where a layout will be used: what is in scope there.
pub struct Site<'p> {
    /// Is this record described by the instrumenter itself (a project struct/class)?
    /// Such records are referenced, not copied.
    pub is_described: &'p dyn Fn(&Cursor<'_>) -> bool,
}

struct Builder<'p, 'a> {
    site: &'p Site<'p>,
    nodes: Vec<LayoutNode>,
    externals: Vec<String>,
    /// Canonical spelling -> node, so shared and self-referential types are one node.
    memo: HashMap<String, u32>,
    _p: std::marker::PhantomData<&'a ()>,
}

/// The scalar a libclang type kind stands for.
fn prim_of(kind: i32) -> Option<(&'static str, bool)> {
    Some(match kind {
        3 => ("Bool", false),
        4 => ("Char", false),
        5 => ("UnsignedChar", false),
        6 => ("Char16", false),
        7 => ("Char32", false),
        8 => ("UnsignedShort", false),
        9 => ("UnsignedInt", false),
        10 => ("UnsignedLong", false),
        11 => ("UnsignedLongLong", false),
        13 => ("Char", true),
        14 => ("SignedChar", true),
        15 => ("WChar", true),
        16 => ("Short", true),
        17 => ("Int", true),
        18 => ("Long", true),
        19 => ("LongLong", true),
        21 => ("Float", true),
        22 => ("Double", true),
        23 => ("LongDouble", true),
        _ => return None,
    })
}

/// Strip pointers, references and arrays: the type that decides whether a layout is needed.
fn innermost<'a>(mut t: Type<'a>) -> Type<'a> {
    loop {
        t = t.canonical();
        match t.kind() {
            type_kind::POINTER | type_kind::LVALUE_REF | type_kind::RVALUE_REF => t = t.pointee(),
            type_kind::CONSTANT_ARRAY | type_kind::INCOMPLETE_ARRAY => t = t.array_element(),
            _ => return t,
        }
    }
}

/// Does a variable of type `ty` need a layout: does it hold a record the instrumenter
/// does not already describe (a library class, or a template instantiation)?
pub fn needs_layout(ty: Type<'_>, site: &Site<'_>) -> bool {
    let t = innermost(ty);
    t.kind() == type_kind::RECORD && {
        let decl = t.declaration();
        decl.is_definition() && !(site.is_described)(&decl)
    }
}

/// The layout of `ty` (a variable's, field's or allocation's type). `root_name` is the
/// name to show for the root (the type as the user wrote it). `None` when nothing
/// useful can be said.
pub fn build(ty: Type<'_>, root_name: &str, site: &Site<'_>) -> Option<Layout> {
    let mut b = Builder { site, nodes: Vec::new(), externals: Vec::new(), memo: HashMap::new(), _p: std::marker::PhantomData };
    let root = b.node(ty.canonical(), Some(root_name), 0)?;
    // The runtime defines the root under the id the program's own type has, so it must be
    // node 0 (or at least is looked up by `root`); keep the index explicit.
    Some(Layout { nodes: b.nodes, root, externals: b.externals })
}

impl<'p, 'a> Builder<'p, 'a> {
    fn push(&mut self, n: LayoutNode) -> u32 {
        self.nodes.push(n);
        (self.nodes.len() - 1) as u32
    }

    fn opaque(&mut self, size: i64, name: &str) -> u32 {
        self.push(LayoutNode::new(NodeKind::Opaque, size.max(0) as u64, name))
    }

    fn node(&mut self, t: Type<'_>, root_name: Option<&str>, depth: u32) -> Option<u32> {
        let size = t.size_of();
        let spelling = t.spelling();
        let name = root_name.map(str::to_string).unwrap_or_else(|| display_name(&spelling));
        let kind = t.kind();

        if let Some((prim, signed)) = prim_of(kind) {
            // Scalars are shared, not duplicated per use.
            let key = format!("prim:{prim}");
            if let Some(i) = self.memo.get(&key) {
                return Some(*i);
            }
            if size <= 0 {
                return None;
            }
            let mut n = LayoutNode::new(NodeKind::Prim, size as u64, name);
            n.prim = prim;
            n.signed = signed;
            let i = self.push(n);
            self.memo.insert(key, i);
            return Some(i);
        }
        if kind == type_kind::VOID {
            return Some(self.opaque(0, "void"));
        }
        if size < 0 && kind != type_kind::INCOMPLETE_ARRAY {
            // Incomplete or dependent: nothing to read.
            return if depth == 0 { None } else { Some(self.opaque(0, &name)) };
        }
        if self.nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
            return Some(self.opaque(size, &name));
        }
        // Everything below is memoized on the spelling (which, canonical, is unique).
        if let Some(i) = self.memo.get(&spelling) {
            if root_name.is_none() {
                return Some(*i);
            }
        }

        match kind {
            type_kind::POINTER => {
                let i = self.push(LayoutNode::new(NodeKind::Pointer, size as u64, name));
                self.memo.insert(spelling, i);
                let pointee = t.pointee().canonical();
                let target = self.node(pointee, None, depth + 1).unwrap_or_else(|| self.opaque(0, "?"));
                self.nodes[i as usize].reference = target;
                Some(i)
            }
            type_kind::LVALUE_REF | type_kind::RVALUE_REF => Some(self.opaque(size, &name)),
            type_kind::CONSTANT_ARRAY => {
                let len = t.array_size();
                let elem = t.array_element().canonical();
                if len < 0 || len > MAX_ARRAY || elem.size_of() <= 0 {
                    return Some(self.opaque(size, &name));
                }
                let i = self.push(LayoutNode::new(NodeKind::Array, size as u64, name));
                self.memo.insert(spelling, i);
                let e = self.node(elem, None, depth + 1).unwrap_or_else(|| self.opaque(0, "?"));
                let n = &mut self.nodes[i as usize];
                n.reference = e;
                n.count = len as u64;
                Some(i)
            }
            type_kind::ENUM => {
                let decl = t.declaration();
                let int = decl.enum_int_type().canonical();
                let signed = prim_of(int.kind()).map(|p| p.1).unwrap_or(true);
                let mut n = LayoutNode::new(NodeKind::Enum, size as u64, name);
                n.signed = signed;
                let i = self.push(n);
                self.memo.insert(spelling, i);
                Some(i)
            }
            type_kind::RECORD => Some(self.record(t, name, spelling, size, depth)),
            _ => Some(self.opaque(size, &name)),
        }
    }

    fn record(&mut self, t: Type<'_>, name: String, spelling: String, size: i64, depth: u32) -> u32 {
        let decl = t.declaration();
        // A project record with its own description: referenced, not copied, so that both
        // routes (its descriptor and this layout) agree on one type.
        if (self.site.is_described)(&decl) {
            let slot = match self.externals.iter().position(|e| *e == spelling) {
                Some(i) => i,
                None => {
                    self.externals.push(spelling.clone());
                    self.externals.len() - 1
                }
            };
            let mut n = LayoutNode::new(NodeKind::External, size as u64, name);
            n.reference = slot as u32;
            let i = self.push(n);
            self.memo.insert(spelling, i);
            return i;
        }

        let i = self.push(LayoutNode::new(NodeKind::Opaque, size as u64, name.clone()));
        self.memo.insert(spelling, i);
        if !decl.is_definition() {
            return i;
        }

        // Only plain layouts are described: anything with virtual bases, several bases,
        // a vtable or bit-fields is left as a sized, named, opaque node. (The members are
        // read from the *type*, which also covers implicit template instantiations.)
        let bases = t.bases();
        if bases.len() > 1 || bases.iter().any(|b| b.is_virtual_base()) {
            return i;
        }
        let own = t.fields();
        let mut offsets: Vec<u64> = Vec::new();
        let mut members: Vec<(String, u64, Type<'_>)> = Vec::new();
        for c in &own {
            if c.is_bitfield() {
                return i;
            }
            let bits = c.field_offset_bits();
            if bits < 0 || bits % 8 != 0 {
                return i;
            }
            let mut fname = c.spelling();
            if fname.is_empty() {
                fname = format!("_anon{}", members.len());
            }
            offsets.push((bits / 8) as u64);
            members.push((fname, (bits / 8) as u64, c.ty().canonical()));
        }
        // An empty base (an allocator, a comparator) holds no data and, by the empty-base
        // optimization, may overlap the first member: it contributes nothing to describe.
        let data_base = bases.first().filter(|b| {
            let bty = b.ty().canonical();
            !(bty.fields().is_empty() && bty.bases().is_empty())
        });
        if let Some(base) = data_base {
            // The base is described at offset 0, which holds unless the class has a vtable
            // of its own in front of it: then the first member (or the end, with none) would
            // not lie after the whole base.
            let bty = base.ty().canonical();
            let bsize = bty.size_of();
            let first_own = offsets.iter().min().copied().unwrap_or(size as u64);
            if bsize < 0 || first_own < bsize as u64 {
                return i;
            }
            members.insert(0, (display_name(&bty.spelling()), 0, bty));
        } else if let Some(first) = offsets.iter().min() {
            // No data base: members start at 0 unless a vtable pointer comes first.
            if *first != 0 {
                return i;
            }
        }

        let mut fields = Vec::new();
        for (fname, offset, fty) in members {
            let child = self.node(fty, None, depth + 1).unwrap_or_else(|| self.opaque(0, "?"));
            fields.push(LayoutField { name: fname, offset, ty: child });
        }
        let n = &mut self.nodes[i as usize];
        n.kind = NodeKind::Record;
        n.fields = fields;
        i
    }
}

/// A readable name for a type spelling (libclang prints anonymous types with a source
/// position, which is noise).
fn display_name(spelling: &str) -> String {
    if spelling.starts_with('(') || spelling.contains("(unnamed") || spelling.contains("(anonymous") {
        "<anonymous>".to_string()
    } else {
        spelling.to_string()
    }
}
