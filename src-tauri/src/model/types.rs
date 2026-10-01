//! Semantic C++ type descriptions, *supplied by the observer* (from the
//! compiler's own type information). Lattice never parses type strings.
//!
//! Types are stored once in a [`TypeTable`] and referenced by [`TypeId`]; objects
//! and values never carry type names. References between types are resolved
//! lazily, so recursive types (`struct Node { Node* next; }`) need no special
//! handling: declare them in any order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ids::TypeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    Bool,
    Char,
    SignedChar,
    UnsignedChar,
    WChar,
    Char8,
    Char16,
    Char32,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Float,
    Double,
    LongDouble,
    NullPtr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Struct,
    Class,
    Union,
}

/// A data member. Base-class subobjects are listed first with `is_base` set, so
/// a field index is a stable, uniform way to address any subobject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeId,
    pub is_base: bool,
    /// Byte offset, if known. Metadata only.
    pub offset: Option<u64>,
}

impl FieldDecl {
    pub fn new(name: impl Into<String>, ty: TypeId) -> Self {
        FieldDecl { name: name.into(), ty, is_base: false, offset: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enumerator {
    pub name: String,
    pub value: i64,
}

/// A template argument of an instantiated record (e.g. `std::vector<int>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateArg {
    Type(TypeId),
    Integral(i64),
    /// Anything else (packs, template-template args), as the compiler prints it.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeKind {
    Void,
    Primitive { primitive: Primitive },
    Record { record: RecordKind, fields: Vec<FieldDecl>, template_args: Vec<TemplateArg> },
    Enum { underlying: Option<TypeId>, enumerators: Vec<Enumerator>, scoped: bool },
    Pointer { pointee: TypeId },
    LValueReference { referent: TypeId },
    RValueReference { referent: TypeId },
    /// `len == None` for incomplete / unknown-bound arrays.
    Array { element: TypeId, len: Option<u64> },
    Function { ret: TypeId, params: Vec<TypeId>, variadic: bool },
    /// Known only by name (incomplete or not-yet-modelled types).
    Opaque,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Qualifiers {
    pub is_const: bool,
    pub is_volatile: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    pub id: TypeId,
    /// Display name exactly as the compiler spells it (`Node*`, `std::vector<int>`).
    /// For display only; never parsed.
    pub name: String,
    pub kind: TypeKind,
    pub size: Option<u64>,
    pub qualifiers: Qualifiers,
}

impl TypeDef {
    pub fn new(id: TypeId, name: impl Into<String>, kind: TypeKind) -> Self {
        TypeDef { id, name: name.into(), kind, size: None, qualifiers: Qualifiers::default() }
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypeTable {
    types: BTreeMap<TypeId, TypeDef>,
}

impl TypeTable {
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(&id)
    }

    pub fn contains(&self, id: TypeId) -> bool {
        self.types.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values()
    }

    pub fn name(&self, id: TypeId) -> Option<&str> {
        self.get(id).map(|t| t.name.as_str())
    }

    /// Fields of a record type, in declaration order (bases first).
    pub fn fields(&self, id: TypeId) -> Option<&[FieldDecl]> {
        match &self.get(id)?.kind {
            TypeKind::Record { fields, .. } => Some(fields),
            _ => None,
        }
    }

    pub fn field(&self, id: TypeId, index: u32) -> Option<&FieldDecl> {
        self.fields(id)?.get(index as usize)
    }

    /// Target type of a pointer or reference type.
    pub fn pointee(&self, id: TypeId) -> Option<TypeId> {
        match &self.get(id)?.kind {
            TypeKind::Pointer { pointee } => Some(*pointee),
            TypeKind::LValueReference { referent } | TypeKind::RValueReference { referent } => {
                Some(*referent)
            }
            _ => None,
        }
    }

    /// Insert a definition. Redeclaring an identical definition is a no-op;
    /// a conflicting one is an error and leaves the table unchanged.
    pub(crate) fn declare(&mut self, def: &TypeDef) -> Result<(), ()> {
        match self.types.get(&def.id) {
            Some(existing) if existing == def => Ok(()),
            Some(_) => Err(()),
            None => {
                self.types.insert(def.id, def.clone());
                Ok(())
            }
        }
    }
}
