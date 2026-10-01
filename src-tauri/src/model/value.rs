//! Structured runtime values. A value is a tree; the visualization layer walks it
//! and never parses text. Values carry no type: the owning [`Object`](super::Object)
//! has one `TypeId`, and aggregates are positional (names live in the type table),
//! so nothing is stringly duplicated per element.

use serde::{Deserialize, Serialize};

use super::ids::ObjectId;

/// One step inside an object: a field (by declaration index, bases first) or an
/// array element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Field(u32),
    Index(u64),
}

/// A location inside the model: a root object plus a path to a sub-object.
/// `&node` is `Place { node, [] }`, `&node.next` is `[Field(1)]`, `&arr[2]` is
/// `[Index(2)]`. This is how pointers to interior subobjects are expressed without
/// giving every field its own identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Place {
    pub object: ObjectId,
    pub path: Vec<Step>,
}

impl Place {
    pub fn root(object: ObjectId) -> Self {
        Place { object, path: Vec::new() }
    }

    pub fn field(mut self, index: u32) -> Self {
        self.path.push(Step::Field(index));
        self
    }

    pub fn index(mut self, index: u64) -> Self {
        self.path.push(Step::Index(index));
        self
    }
}

/// What a pointer or reference designates, as resolved by the observer.
///
/// Whether the target is still alive is *not* stored (see
/// [`RuntimeState::target_status`](super::RuntimeState::target_status)): a
/// dangling pointer is simply a `Place` whose root object has been destroyed, so
/// freeing an object never requires rewriting every pointer to it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum Target {
    Null,
    Place { place: Place },
    /// Non-null but not a tracked object: untracked memory, a function, a
    /// one-past-the-end address, an integer cast to a pointer.
    Unresolved,
}

impl Target {
    pub fn to(place: Place) -> Self {
        Target::Place { place }
    }

    pub fn object(object: ObjectId) -> Self {
        Target::Place { place: Place::root(object) }
    }

    pub fn place(&self) -> Option<&Place> {
        match self {
            Target::Place { place } => Some(place),
            _ => None,
        }
    }
}

/// A pointer value. `address` is optional metadata (e.g. to show `0x7ff...`);
/// nothing in the model is keyed by it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerValue {
    pub address: Option<u64>,
    pub target: Target,
}

impl PointerValue {
    pub fn null() -> Self {
        PointerValue { address: Some(0), target: Target::Null }
    }

    pub fn to_object(object: ObjectId) -> Self {
        PointerValue { address: None, target: Target::object(object) }
    }

    pub fn to_place(place: Place) -> Self {
        PointerValue { address: None, target: Target::to(place) }
    }
}

/// Why there is no value. Deliberately separate from every real value so an
/// absent value can never be mistaken for `0`, `nullptr` or `""`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unavailable {
    /// The observer has not reported it / cannot say.
    Unknown,
    /// Storage exists but is indeterminate (never initialized).
    Uninitialized,
    /// The compiler eliminated it.
    OptimizedAway,
    /// Memory could not be read.
    Unreadable,
    /// The owning scope has ended.
    OutOfScope,
    /// Bytes are present but are not a valid value of the type (e.g. a `bool`
    /// holding 7, a corrupt enum). `raw` is the observer's rendering, if any.
    Invalid { raw: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Value {
    Int { value: i64 },
    UInt { value: u64 },
    /// Note: JSON cannot carry NaN/infinity; a JSON transport must map them.
    Float { value: f64 },
    Bool { value: bool },
    /// Code point / code unit of a character type.
    Char { value: u32 },
    /// Text already materialized by the observer (`const char*` contents,
    /// `std::string` payload).
    String { value: String },
    Enum { value: i64, enumerator: Option<String> },
    Pointer { pointer: PointerValue },
    /// An lvalue/rvalue reference. Never null; may dangle.
    Reference { target: Target },
    /// Struct/class members in `TypeKind::Record::fields` order (bases first).
    Aggregate { fields: Vec<Value> },
    Array { elements: Vec<Value> },
    /// A union: which member is active (if known) and its value.
    Union { active: Option<u32>, value: Box<Value> },
    Unavailable { unavailable: Unavailable },
}

/// Whether a link is a pointer or a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Pointer,
    Reference,
}

/// A directed, machine-readable link: the pointer/reference stored at `source`
/// designates `target`. Only links to tracked places are edges; null and
/// unresolved targets are visible by walking the value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Edge {
    pub source: Place,
    pub kind: EdgeKind,
    pub target: Place,
}

impl Value {
    pub fn int(value: i64) -> Value {
        Value::Int { value }
    }

    pub fn uint(value: u64) -> Value {
        Value::UInt { value }
    }

    pub fn boolean(value: bool) -> Value {
        Value::Bool { value }
    }

    pub fn float(value: f64) -> Value {
        Value::Float { value }
    }

    pub fn null_pointer() -> Value {
        Value::Pointer { pointer: PointerValue::null() }
    }

    pub fn pointer_to(object: ObjectId) -> Value {
        Value::Pointer { pointer: PointerValue::to_object(object) }
    }

    pub fn pointer_to_place(place: Place) -> Value {
        Value::Pointer { pointer: PointerValue::to_place(place) }
    }

    pub fn reference_to(place: Place) -> Value {
        Value::Reference { target: Target::to(place) }
    }

    pub fn aggregate(fields: Vec<Value>) -> Value {
        Value::Aggregate { fields }
    }

    pub fn array(elements: Vec<Value>) -> Value {
        Value::Array { elements }
    }

    pub fn unavailable(reason: Unavailable) -> Value {
        Value::Unavailable { unavailable: reason }
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self, Value::Unavailable { .. })
    }

    /// Child at `step`, if the shape matches.
    pub fn child(&self, step: Step) -> Option<&Value> {
        match (self, step) {
            (Value::Aggregate { fields }, Step::Field(i)) => fields.get(i as usize),
            (Value::Array { elements }, Step::Index(i)) => elements.get(usize::try_from(i).ok()?),
            (Value::Union { active: Some(a), value }, Step::Field(i)) if *a == i => Some(value),
            _ => None,
        }
    }

    fn child_mut(&mut self, step: Step) -> Option<&mut Value> {
        match (self, step) {
            (Value::Aggregate { fields }, Step::Field(i)) => fields.get_mut(i as usize),
            (Value::Array { elements }, Step::Index(i)) => {
                elements.get_mut(usize::try_from(i).ok()?)
            }
            (Value::Union { active: Some(a), value }, Step::Field(i)) if *a == i => Some(value),
            _ => None,
        }
    }

    /// Follow `path` from this value.
    pub fn at(&self, path: &[Step]) -> Option<&Value> {
        path.iter().try_fold(self, |v, s| v.child(*s))
    }

    pub(crate) fn at_mut(&mut self, path: &[Step]) -> Option<&mut Value> {
        let mut cur = self;
        for s in path {
            cur = cur.child_mut(*s)?;
        }
        Some(cur)
    }

    /// Call `f(path, kind, target)` for every pointer/reference inside this value
    /// (including null/unresolved ones). `path` is relative to `self`. Iterative,
    /// so deeply nested values cannot overflow the stack.
    pub fn for_each_link<F: FnMut(&[Step], EdgeKind, &Target)>(&self, mut f: F) {
        let mut path: Vec<Step> = Vec::new();
        visit_node(self, &path, &mut f);
        // (value, index of the next child to visit); parallel to `path` + root.
        let mut stack: Vec<(&Value, usize)> = vec![(self, 0)];
        while let Some(top) = stack.last_mut() {
            let value = top.0;
            let next = top.1;
            let child: Option<(&Value, Step)> = match value {
                Value::Aggregate { fields } => {
                    fields.get(next).map(|c| (c, Step::Field(next as u32)))
                }
                Value::Array { elements } => {
                    elements.get(next).map(|c| (c, Step::Index(next as u64)))
                }
                Value::Union { active: Some(a), value } if next == 0 => {
                    Some((&**value, Step::Field(*a)))
                }
                _ => None,
            };
            match child {
                Some((c, step)) => {
                    top.1 += 1;
                    path.push(step);
                    visit_node(c, &path, &mut f);
                    stack.push((c, 0));
                }
                None => {
                    stack.pop();
                    path.pop();
                }
            }
        }
    }
}

fn visit_node<F: FnMut(&[Step], EdgeKind, &Target)>(value: &Value, path: &[Step], f: &mut F) {
    match value {
        Value::Pointer { pointer } => f(path, EdgeKind::Pointer, &pointer.target),
        Value::Reference { target } => f(path, EdgeKind::Reference, target),
        _ => {}
    }
}
