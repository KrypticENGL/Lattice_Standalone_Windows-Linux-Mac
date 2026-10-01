//! Minimal, dynamically loaded binding to the libclang C API.
//!
//! libclang is the analysis frontend because it is a *stable C ABI* that ships in
//! official LLVM distributions (`bin/libclang.dll`), needs no link-time LLVM, and
//! lets us skip every header subtree in-process. Loading it at run time means
//! Lattice starts (and every other feature works) without it.
//!
//! Only the handful of functions the instrumenter needs are bound. Structure
//! layouts follow `clang-c/Index.h` (x86-64).

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CXCursor {
    kind: c_int,
    xdata: c_int,
    data: [*const c_void; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CXSourceLocation {
    ptr_data: [*const c_void; 2],
    int_data: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CXSourceRange {
    ptr_data: [*const c_void; 2],
    begin_int_data: c_uint,
    end_int_data: c_uint,
}

#[repr(C)]
pub struct CXString {
    data: *const c_void,
    private_flags: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CXType {
    kind: c_int,
    data: [*const c_void; 2],
}

type Visitor = extern "C" fn(CXCursor, CXCursor, *mut c_void) -> c_int;

/// `CXFieldVisitor`: returns `CXVisitorResult` (0 break, 1 continue).
type FieldVisitor = extern "C" fn(CXCursor, *mut c_void) -> c_int;

/// `CXChildVisitResult`
pub const CHILD_BREAK: c_int = 0;
pub const CHILD_CONTINUE: c_int = 1;
pub const CHILD_RECURSE: c_int = 2;

/// `CXTypeKind` values we classify by. Stable across libclang versions.
pub mod type_kind {
    pub const BOOL: i32 = 3;
    pub const LONG_DOUBLE: i32 = 23;
    pub const NULLPTR: i32 = 24;
    pub const POINTER: i32 = 101;
    pub const LVALUE_REF: i32 = 103;
    pub const RVALUE_REF: i32 = 104;
    pub const RECORD: i32 = 105;
    pub const ENUM: i32 = 106;
    pub const INCOMPLETE_ARRAY: i32 = 114;
    pub const CONSTANT_ARRAY: i32 = 112;
    pub const VARIABLE_ARRAY: i32 = 115;
    pub const DEPENDENT: i32 = 26;
    pub const VOID: i32 = 2;
}

/// A 1-based source position and byte offset in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePos {
    pub offset: usize,
    pub line: u32,
    pub column: u32,
}

struct Api {
    _lib: Library,
    create_index: unsafe extern "C" fn(c_int, c_int) -> *mut c_void,
    dispose_index: unsafe extern "C" fn(*mut c_void),
    parse: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const *const c_char,
        c_int,
        *mut c_void,
        c_uint,
        c_uint,
    ) -> *mut c_void,
    dispose_tu: unsafe extern "C" fn(*mut c_void),
    tu_cursor: unsafe extern "C" fn(*mut c_void) -> CXCursor,
    visit_children: unsafe extern "C" fn(CXCursor, Visitor, *mut c_void) -> c_uint,
    cursor_kind: unsafe extern "C" fn(CXCursor) -> c_int,
    cursor_kind_spelling: unsafe extern "C" fn(c_int) -> CXString,
    cursor_spelling: unsafe extern "C" fn(CXCursor) -> CXString,
    cursor_extent: unsafe extern "C" fn(CXCursor) -> CXSourceRange,
    cursor_location: unsafe extern "C" fn(CXCursor) -> CXSourceLocation,
    cursor_type: unsafe extern "C" fn(CXCursor) -> CXType,
    cursor_referenced: unsafe extern "C" fn(CXCursor) -> CXCursor,
    cursor_semantic_parent: unsafe extern "C" fn(CXCursor) -> CXCursor,
    is_definition: unsafe extern "C" fn(CXCursor) -> c_uint,
    is_bitfield: unsafe extern "C" fn(CXCursor) -> c_uint,
    access_specifier: unsafe extern "C" fn(CXCursor) -> c_int,
    binary_operator_kind: unsafe extern "C" fn(CXCursor) -> c_int,
    unary_operator_kind: unsafe extern "C" fn(CXCursor) -> c_int,
    storage_class: unsafe extern "C" fn(CXCursor) -> c_int,
    tls_kind: unsafe extern "C" fn(CXCursor) -> c_int,
    is_expression: unsafe extern "C" fn(c_int) -> c_uint,
    array_element_type: unsafe extern "C" fn(CXType) -> CXType,
    binary_operator_spelling: unsafe extern "C" fn(c_int) -> CXString,
    range_start: unsafe extern "C" fn(CXSourceRange) -> CXSourceLocation,
    range_end: unsafe extern "C" fn(CXSourceRange) -> CXSourceLocation,
    expansion_location:
        unsafe extern "C" fn(CXSourceLocation, *mut *mut c_void, *mut c_uint, *mut c_uint, *mut c_uint),
    spelling_location:
        unsafe extern "C" fn(CXSourceLocation, *mut *mut c_void, *mut c_uint, *mut c_uint, *mut c_uint),
    is_from_main_file: unsafe extern "C" fn(CXSourceLocation) -> c_int,
    file_name: unsafe extern "C" fn(*mut c_void) -> CXString,
    type_declaration: unsafe extern "C" fn(CXType) -> CXCursor,
    visit_fields: unsafe extern "C" fn(CXType, FieldVisitor, *mut c_void) -> c_uint,
    visit_bases: unsafe extern "C" fn(CXType, FieldVisitor, *mut c_void) -> c_uint,
    type_size_of: unsafe extern "C" fn(CXType) -> i64,
    array_size: unsafe extern "C" fn(CXType) -> i64,
    field_offset_bits: unsafe extern "C" fn(CXCursor) -> i64,
    is_virtual_method: unsafe extern "C" fn(CXCursor) -> c_uint,
    is_virtual_base: unsafe extern "C" fn(CXCursor) -> c_uint,
    enum_int_type: unsafe extern "C" fn(CXCursor) -> CXType,
    canonical_type: unsafe extern "C" fn(CXType) -> CXType,
    pointee_type: unsafe extern "C" fn(CXType) -> CXType,
    type_spelling: unsafe extern "C" fn(CXType) -> CXString,
    num_diagnostics: unsafe extern "C" fn(*mut c_void) -> c_uint,
    get_diagnostic: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    diagnostic_severity: unsafe extern "C" fn(*mut c_void) -> c_int,
    diagnostic_location: unsafe extern "C" fn(*mut c_void) -> CXSourceLocation,
    is_in_system_header: unsafe extern "C" fn(CXSourceLocation) -> c_int,
    format_diagnostic: unsafe extern "C" fn(*mut c_void, c_uint) -> CXString,
    default_diagnostic_options: unsafe extern "C" fn() -> c_uint,
    dispose_diagnostic: unsafe extern "C" fn(*mut c_void),
    get_c_string: unsafe extern "C" fn(*const CXString) -> *const c_char,
    dispose_string: unsafe extern "C" fn(CXString),
    get_version: unsafe extern "C" fn() -> CXString,
}

// ---- dynamic loading ----------------------------------------------------------

#[cfg(windows)]
struct Library(*mut c_void);

// The handle is only used to resolve symbols; libraries are never unloaded.
#[cfg(windows)]
unsafe impl Send for Library {}
#[cfg(windows)]
unsafe impl Sync for Library {}

#[cfg(windows)]
impl Library {
    fn open(path: &Path) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH};
        // LOAD_WITH_ALTERED_SEARCH_PATH needs backslashes; accept either from callers.
        let normalized = path.to_string_lossy().replace('/', "\\");
        let wide: Vec<u16> = std::ffi::OsStr::new(&normalized).encode_wide().chain(Some(0)).collect();
        // Altered search path: the DLL's own directory is searched for its dependencies.
        let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if h.is_null() {
            Err(format!("could not load {}", path.display()))
        } else {
            Ok(Library(h))
        }
    }

    fn sym(&self, name: &str) -> Result<*const c_void, String> {
        use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        let c = CString::new(name).unwrap();
        match unsafe { GetProcAddress(self.0, c.as_ptr() as *const u8) } {
            Some(f) => Ok(f as *const c_void),
            None => Err(format!("libclang is missing `{name}` (too old?)")),
        }
    }
}

#[cfg(not(windows))]
struct Library;

#[cfg(not(windows))]
impl Library {
    fn open(_: &Path) -> Result<Self, String> {
        Err("dynamic libclang loading is implemented for Windows only".into())
    }
    fn sym(&self, _: &str) -> Result<*const c_void, String> {
        unreachable!()
    }
}

// Libraries are intentionally never unloaded.

/// A loaded libclang. Cheap to keep; create one per process.
pub struct Clang {
    api: Api,
    pub version: String,
}

macro_rules! bind {
    ($lib:expr, $name:literal) => {{
        let p = $lib.sym($name)?;
        unsafe { std::mem::transmute::<*const c_void, _>(p) }
    }};
}

impl Clang {
    pub fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::open(path)?;
        let api = Api {
            create_index: bind!(lib, "clang_createIndex"),
            dispose_index: bind!(lib, "clang_disposeIndex"),
            parse: bind!(lib, "clang_parseTranslationUnit"),
            dispose_tu: bind!(lib, "clang_disposeTranslationUnit"),
            tu_cursor: bind!(lib, "clang_getTranslationUnitCursor"),
            visit_children: bind!(lib, "clang_visitChildren"),
            cursor_kind: bind!(lib, "clang_getCursorKind"),
            cursor_kind_spelling: bind!(lib, "clang_getCursorKindSpelling"),
            cursor_spelling: bind!(lib, "clang_getCursorSpelling"),
            cursor_extent: bind!(lib, "clang_getCursorExtent"),
            cursor_location: bind!(lib, "clang_getCursorLocation"),
            cursor_type: bind!(lib, "clang_getCursorType"),
            cursor_referenced: bind!(lib, "clang_getCursorReferenced"),
            cursor_semantic_parent: bind!(lib, "clang_getCursorSemanticParent"),
            is_definition: bind!(lib, "clang_isCursorDefinition"),
            is_bitfield: bind!(lib, "clang_Cursor_isBitField"),
            access_specifier: bind!(lib, "clang_getCXXAccessSpecifier"),
            binary_operator_kind: bind!(lib, "clang_getCursorBinaryOperatorKind"),
            unary_operator_kind: bind!(lib, "clang_getCursorUnaryOperatorKind"),
            storage_class: bind!(lib, "clang_Cursor_getStorageClass"),
            tls_kind: bind!(lib, "clang_getCursorTLSKind"),
            is_expression: bind!(lib, "clang_isExpression"),
            array_element_type: bind!(lib, "clang_getArrayElementType"),
            binary_operator_spelling: bind!(lib, "clang_getBinaryOperatorKindSpelling"),
            range_start: bind!(lib, "clang_getRangeStart"),
            range_end: bind!(lib, "clang_getRangeEnd"),
            expansion_location: bind!(lib, "clang_getExpansionLocation"),
            spelling_location: bind!(lib, "clang_getSpellingLocation"),
            is_from_main_file: bind!(lib, "clang_Location_isFromMainFile"),
            file_name: bind!(lib, "clang_getFileName"),
            type_declaration: bind!(lib, "clang_getTypeDeclaration"),
            visit_fields: bind!(lib, "clang_Type_visitFields"),
            visit_bases: bind!(lib, "clang_visitCXXBaseClasses"),
            type_size_of: bind!(lib, "clang_Type_getSizeOf"),
            array_size: bind!(lib, "clang_getArraySize"),
            field_offset_bits: bind!(lib, "clang_Cursor_getOffsetOfField"),
            is_virtual_method: bind!(lib, "clang_CXXMethod_isVirtual"),
            is_virtual_base: bind!(lib, "clang_isVirtualBase"),
            enum_int_type: bind!(lib, "clang_getEnumDeclIntegerType"),
            canonical_type: bind!(lib, "clang_getCanonicalType"),
            pointee_type: bind!(lib, "clang_getPointeeType"),
            type_spelling: bind!(lib, "clang_getTypeSpelling"),
            num_diagnostics: bind!(lib, "clang_getNumDiagnostics"),
            get_diagnostic: bind!(lib, "clang_getDiagnostic"),
            diagnostic_severity: bind!(lib, "clang_getDiagnosticSeverity"),
            diagnostic_location: bind!(lib, "clang_getDiagnosticLocation"),
            is_in_system_header: bind!(lib, "clang_Location_isInSystemHeader"),
            format_diagnostic: bind!(lib, "clang_formatDiagnostic"),
            default_diagnostic_options: bind!(lib, "clang_defaultDiagnosticDisplayOptions"),
            dispose_diagnostic: bind!(lib, "clang_disposeDiagnostic"),
            get_c_string: bind!(lib, "clang_getCString"),
            dispose_string: bind!(lib, "clang_disposeString"),
            get_version: bind!(lib, "clang_getClangVersion"),
            _lib: lib,
        };
        let mut c = Clang { api, version: String::new() };
        c.version = c.string(unsafe { (c.api.get_version)() });
        Ok(c)
    }

    fn string(&self, s: CXString) -> String {
        unsafe {
            let p = (self.api.get_c_string)(&s);
            let out = if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() };
            (self.api.dispose_string)(s);
            out
        }
    }

    /// Parse `file` (on disk) with the given compiler arguments.
    pub fn parse(&self, file: &Path, args: &[String]) -> Result<TranslationUnit<'_>, String> {
        let file_c = CString::new(file.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let arg_c: Vec<CString> = args.iter().map(|a| CString::new(a.as_bytes()).unwrap()).collect();
        let arg_p: Vec<*const c_char> = arg_c.iter().map(|a| a.as_ptr()).collect();
        unsafe {
            let index = (self.api.create_index)(0, 0);
            let tu = (self.api.parse)(
                index,
                file_c.as_ptr(),
                arg_p.as_ptr(),
                arg_p.len() as c_int,
                std::ptr::null_mut(),
                0,
                0,
            );
            if tu.is_null() {
                (self.api.dispose_index)(index);
                return Err("libclang could not create a translation unit".into());
            }
            Ok(TranslationUnit { clang: self, index, tu })
        }
    }
}

pub struct TranslationUnit<'a> {
    clang: &'a Clang,
    index: *mut c_void,
    tu: *mut c_void,
}

impl Drop for TranslationUnit<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.clang.api.dispose_tu)(self.tu);
            (self.clang.api.dispose_index)(self.index);
        }
    }
}

impl<'a> TranslationUnit<'a> {
    pub fn clang(&self) -> &'a Clang {
        self.clang
    }

    pub fn root(&self) -> Cursor<'a> {
        Cursor { clang: self.clang, raw: unsafe { (self.clang.api.tu_cursor)(self.tu) } }
    }

    /// Error and fatal diagnostics in the *user's* code, formatted like the
    /// compiler would print them, and how many errors inside system headers were
    /// set aside. libclang may not understand every vendor-specific header
    /// (GCC's intrinsics headers, pulled in by `<windows.h>`, are an example); that
    /// says nothing about the user's program, whose own errors are what count.
    pub fn errors(&self) -> (Vec<String>, usize) {
        let a = &self.clang.api;
        let mut user = Vec::new();
        let mut system = 0;
        unsafe {
            for i in 0..(a.num_diagnostics)(self.tu) {
                let d = (a.get_diagnostic)(self.tu, i);
                if (a.diagnostic_severity)(d) >= 3 {
                    if (a.is_in_system_header)((a.diagnostic_location)(d)) != 0 {
                        system += 1;
                    } else {
                        user.push(self.clang.string((a.format_diagnostic)(d, (a.default_diagnostic_options)())));
                    }
                }
                (a.dispose_diagnostic)(d);
            }
        }
        (user, system)
    }
}

#[derive(Clone, Copy)]
pub struct Cursor<'a> {
    clang: &'a Clang,
    raw: CXCursor,
}

#[derive(Clone, Copy)]
pub struct Type<'a> {
    clang: &'a Clang,
    raw: CXType,
}

struct Payload<'a, F> {
    clang: &'a Clang,
    f: F,
}

extern "C" fn trampoline<'a, F: FnMut(Cursor<'a>) -> c_int>(
    cursor: CXCursor,
    _parent: CXCursor,
    data: *mut c_void,
) -> c_int {
    // `data` is the `Payload` on the stack of `visit_children`, alive for the call.
    let payload = unsafe { &mut *(data as *mut Payload<'a, F>) };
    (payload.f)(Cursor { clang: payload.clang, raw: cursor })
}

impl<'a> Cursor<'a> {
    pub fn kind_name(&self) -> String {
        let a = &self.clang.api;
        unsafe { self.clang.string((a.cursor_kind_spelling)((a.cursor_kind)(self.raw))) }
    }

    pub fn spelling(&self) -> String {
        unsafe { self.clang.string((self.clang.api.cursor_spelling)(self.raw)) }
    }

    pub fn is_definition(&self) -> bool {
        unsafe { (self.clang.api.is_definition)(self.raw) != 0 }
    }

    pub fn is_bitfield(&self) -> bool {
        unsafe { (self.clang.api.is_bitfield)(self.raw) != 0 }
    }

    /// 1 public, 2 protected, 3 private, 0 not applicable.
    pub fn access(&self) -> i32 {
        unsafe { (self.clang.api.access_specifier)(self.raw) }
    }

    pub fn ty(&self) -> Type<'a> {
        Type { clang: self.clang, raw: unsafe { (self.clang.api.cursor_type)(self.raw) } }
    }

    pub fn referenced(&self) -> Cursor<'a> {
        Cursor { clang: self.clang, raw: unsafe { (self.clang.api.cursor_referenced)(self.raw) } }
    }

    pub fn semantic_parent(&self) -> Cursor<'a> {
        Cursor { clang: self.clang, raw: unsafe { (self.clang.api.cursor_semantic_parent)(self.raw) } }
    }

    pub fn is_from_main_file(&self) -> bool {
        unsafe {
            let loc = (self.clang.api.cursor_location)(self.raw);
            (self.clang.api.is_from_main_file)(loc) != 0
        }
    }

    /// The file this cursor is written in (where a macro is *expanded*, if it is one), as
    /// libclang spells the path. `None` for built-in/invalid locations.
    pub fn file_name(&self) -> Option<String> {
        let a = &self.clang.api;
        unsafe {
            let loc = (a.cursor_location)(self.raw);
            let (mut f, mut l, mut c, mut o) = (std::ptr::null_mut(), 0, 0, 0);
            (a.expansion_location)(loc, &mut f, &mut l, &mut c, &mut o);
            if f.is_null() {
                return None;
            }
            let name = self.clang.string((a.file_name)(f));
            (!name.is_empty()).then_some(name)
        }
    }

    /// Offset of a data member from the start of its record, in bits; negative when
    /// libclang cannot say (dependent or incomplete types).
    pub fn field_offset_bits(&self) -> i64 {
        unsafe { (self.clang.api.field_offset_bits)(self.raw) }
    }

    pub fn is_virtual_method(&self) -> bool {
        unsafe { (self.clang.api.is_virtual_method)(self.raw) != 0 }
    }

    pub fn is_virtual_base(&self) -> bool {
        unsafe { (self.clang.api.is_virtual_base)(self.raw) != 0 }
    }

    /// The integer type an enum is stored as.
    pub fn enum_int_type(&self) -> Type<'a> {
        Type { clang: self.clang, raw: unsafe { (self.clang.api.enum_int_type)(self.raw) } }
    }

    /// `CX_SC_*`: 1 none, 2 extern, 3 static, 6 auto, 7 register.
    pub fn storage_class(&self) -> i32 {
        unsafe { (self.clang.api.storage_class)(self.raw) }
    }

    /// Non-zero for `thread_local` variables.
    pub fn tls_kind(&self) -> i32 {
        unsafe { (self.clang.api.tls_kind)(self.raw) }
    }

    pub fn is_expression(&self) -> bool {
        unsafe { (self.clang.api.is_expression)((self.clang.api.cursor_kind)(self.raw)) != 0 }
    }

    /// `CXUnaryOperatorKind`: 1 post-increment, 2 post-decrement, 3 pre-increment,
    /// 4 pre-decrement, 0 not a unary operator.
    pub fn unary_operator(&self) -> i32 {
        unsafe { (self.clang.api.unary_operator_kind)(self.raw) }
    }

    /// Spelling of the binary operator (`"="`, `"+="`, ...) if this is one.
    pub fn binary_operator(&self) -> Option<String> {
        let a = &self.clang.api;
        unsafe {
            let k = (a.binary_operator_kind)(self.raw);
            if k == 0 {
                None
            } else {
                Some(self.clang.string((a.binary_operator_spelling)(k)))
            }
        }
    }

    /// Source range as byte offsets `[start, end)` in the file. `None` if either
    /// end lies in a macro expansion (spelling and expansion location differ),
    /// because rewriting macro text is not safe.
    pub fn plain_range(&self) -> Option<(FilePos, FilePos)> {
        let a = &self.clang.api;
        unsafe {
            let range = (a.cursor_extent)(self.raw);
            let start = (a.range_start)(range);
            let end = (a.range_end)(range);
            let s = self.file_pos(start)?;
            let e = self.file_pos(end)?;
            Some((s, e))
        }
    }

    unsafe fn file_pos(&self, loc: CXSourceLocation) -> Option<FilePos> {
        let a = &self.clang.api;
        let (mut f1, mut l1, mut c1, mut o1) = (std::ptr::null_mut(), 0, 0, 0);
        let (mut f2, mut l2, mut c2, mut o2) = (std::ptr::null_mut(), 0, 0, 0);
        (a.expansion_location)(loc, &mut f1, &mut l1, &mut c1, &mut o1);
        (a.spelling_location)(loc, &mut f2, &mut l2, &mut c2, &mut o2);
        if o1 != o2 || f1.is_null() {
            return None;
        }
        Some(FilePos { offset: o1 as usize, line: l1, column: c1 })
    }

    /// Visit direct children. The callback returns `CHILD_*`.
    pub fn visit_children<F: FnMut(Cursor<'a>) -> c_int>(&self, f: F) {
        let mut payload = Payload { clang: self.clang, f };
        unsafe {
            (self.clang.api.visit_children)(
                self.raw,
                trampoline::<'a, F>,
                &mut payload as *mut Payload<'a, F> as *mut c_void,
            );
        }
    }

    pub fn children(&self) -> Vec<Cursor<'a>> {
        let mut v = Vec::new();
        self.visit_children(|c| {
            v.push(c);
            CHILD_CONTINUE
        });
        v
    }
}

impl<'a> Type<'a> {
    pub fn kind(&self) -> i32 {
        self.raw.kind
    }

    pub fn spelling(&self) -> String {
        unsafe { self.clang.string((self.clang.api.type_spelling)(self.raw)) }
    }

    pub fn canonical(&self) -> Type<'a> {
        Type { clang: self.clang, raw: unsafe { (self.clang.api.canonical_type)(self.raw) } }
    }

    pub fn array_element(&self) -> Type<'a> {
        Type { clang: self.clang, raw: unsafe { (self.clang.api.array_element_type)(self.raw) } }
    }

    /// The declaration of a class/enum type (the instantiation, for a template).
    pub fn declaration(&self) -> Cursor<'a> {
        Cursor { clang: self.clang, raw: unsafe { (self.clang.api.type_declaration)(self.raw) } }
    }

    /// The data members of a record type, in declaration order, including private ones
    /// and those of implicit template instantiations (which `visit_children` on the
    /// declaration does not show).
    pub fn fields(&self) -> Vec<Cursor<'a>> {
        self.collect(self.clang.api.visit_fields)
    }

    /// The base-class specifiers of a record type.
    pub fn bases(&self) -> Vec<Cursor<'a>> {
        self.collect(self.clang.api.visit_bases)
    }

    fn collect(&self, visit: unsafe extern "C" fn(CXType, FieldVisitor, *mut c_void) -> c_uint) -> Vec<Cursor<'a>> {
        extern "C" fn push(c: CXCursor, data: *mut c_void) -> c_int {
            // `data` is the `Vec` on the stack of `collect`, alive for the call.
            unsafe { (*(data as *mut Vec<CXCursor>)).push(c) };
            1
        }
        let mut raw: Vec<CXCursor> = Vec::new();
        unsafe {
            visit(self.raw, push, &mut raw as *mut Vec<CXCursor> as *mut c_void);
        }
        raw.into_iter().map(|c| Cursor { clang: self.clang, raw: c }).collect()
    }

    /// `sizeof`, in bytes; negative for incomplete or dependent types.
    pub fn size_of(&self) -> i64 {
        unsafe { (self.clang.api.type_size_of)(self.raw) }
    }

    /// Number of elements of a constant array; negative otherwise.
    pub fn array_size(&self) -> i64 {
        unsafe { (self.clang.api.array_size)(self.raw) }
    }

    pub fn pointee(&self) -> Type<'a> {
        Type { clang: self.clang, raw: unsafe { (self.clang.api.pointee_type)(self.raw) } }
    }
}
