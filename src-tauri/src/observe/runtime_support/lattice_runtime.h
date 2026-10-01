// lattice-runtime: in-process support for Lattice's runtime observation.
//
// Force-included (-include) into every instrumented translation unit; the user's
// code never mentions it. Generated instrumentation calls the small surface below.
// Depends on nothing but the C++ standard library: no UI, no Lattice host types.
// See docs/runtime-observation-architecture.md.
#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <new>
#include <string_view>
#include <type_traits>

// offsetof on non-standard-layout types is "conditionally supported": GCC/Clang
// warn (and do the right thing). Generated descriptors silence exactly that.
#if defined(__GNUC__) || defined(__clang__)
#define LATTICE_DIAG_PUSH     _Pragma("GCC diagnostic push") _Pragma("GCC diagnostic ignored \"-Winvalid-offsetof\"")
#define LATTICE_DIAG_POP _Pragma("GCC diagnostic pop")
#else
#define LATTICE_DIAG_PUSH
#define LATTICE_DIAG_POP
#endif

namespace lattice::rt {

using TypeId = std::uint32_t;

// Where an event came from. All pointers are string literals (static storage).
struct Site {
    const char* file;
    int line;
    int column;
    const char* function;
};

struct FieldInfo {
    const char* name;
    TypeId type;
    std::size_t offset;
};

enum class Prim : std::uint8_t {
    Bool, Char, SignedChar, UnsignedChar, WChar, Char8, Char16, Char32,
    Short, UnsignedShort, Int, UnsignedInt, Long, UnsignedLong,
    LongLong, UnsignedLongLong, Float, Double, LongDouble
};

// ---- layouts ---------------------------------------------------------------
// The compiler's own description of a type the instrumenter does not generate a
// descriptor for (a library class, a template instantiation): plain data, produced by
// the analysis, turned into a type description by `detail::define_from_layout`.
enum class LKind : std::uint8_t { Prim, Enum, Pointer, Array, Record, Opaque, External };

struct LField {
    const char* name;
    std::uint32_t offset;  // bytes from the start of the record
    std::uint32_t type;    // node index
};

struct LNode {
    LKind kind;
    Prim prim;           // Prim
    bool is_signed;      // Prim, Enum
    std::uint32_t size;
    const char* name;
    std::uint32_t ref;   // Pointer: pointee node; Array: element node; External: slot
    std::uint32_t count; // Array: length; Record: number of fields
    const LField* fields;
};

struct Layout {
    const LNode* nodes;
    std::uint32_t count;
    std::uint32_t root;
};

using ExtFn = TypeId (*)();

// A layout and the project types it refers to (`External` nodes index `ext`).
struct LayoutRef {
    const Layout* layout = nullptr;
    const ExtFn* ext = nullptr;
};

// Argument of the placement allocation function our `new` rewrite selects.
// For `new T[n]` (`elem_size != 0`) `type` is the *element* type: the length is only
// known to the allocation function, as `size / elem_size`. `type == 0` means "do not
// track this allocation".
struct NewTag {
    Site site;
    TypeId type;
    std::size_t elem_size;
};

namespace detail {
enum class VarKind : std::uint8_t { Local, Parameter };

// Type registry (filled lazily, in the order types are first needed).
TypeId reserve_type();
bool begin_define(TypeId id);  // true: the caller must now define it
void set_name(TypeId id, const char* name, std::size_t len);
void define_primitive(TypeId id, Prim prim, std::size_t size);
void define_enum(TypeId id, std::size_t size, bool is_signed);
void define_pointer(TypeId id, TypeId pointee, std::size_t size);
void define_array(TypeId id, TypeId element, std::size_t len, std::size_t size);
void define_record(TypeId id, std::size_t size, const FieldInfo* fields, std::size_t count);
void define_opaque(TypeId id, std::size_t size);
void define_reference(TypeId id, TypeId referent, std::size_t size);

// Frames, scopes and variables.
void enter_function(const Site& site, const char* name);
void exit_function();
void enter_scope(const Site& site);
void exit_scope();
void declare(VarKind kind, const char* name, TypeId type, const void* address, std::size_t size,
             bool uninitialized, const Site& site);
void declare_reference(VarKind kind, const char* name, TypeId ref_type, TypeId referent,
                       const void* address, const Site& site);

// Observation hooks.
void after_write(const void* address, TypeId type, const Site& site);
void note_delete(const void* address, const Site& site);
void constructed(const void* address);
// Like `constructed`, for storage whose content is still indeterminate (default-
// initialized scalars): the object is alive but its elements stay "uninitialized".
void constructed_uninit(const void* address);

// Describe `id` (already reserved, and being defined by the caller) from a layout.
void define_from_layout(TypeId id, const Layout& layout, const ExtFn* ext);
// The type id the program's own `int`, `char`, ... has: layouts share it, so a pointer
// typed by a layout and one typed by the program's code agree on what they point at.
TypeId prim_type_id(Prim prim);

// Compare tracked memory with what was last reported and report the differences.
void sync(const Site& site);
}  // namespace detail

template <class T>
constexpr std::string_view pretty_name() {
#if defined(_MSC_VER) && !defined(__clang__)
    std::string_view s = __FUNCSIG__;
    std::string_view key = "pretty_name<";
    std::size_t a = s.find(key) + key.size();
    return s.substr(a, s.rfind(">(void)") - a);
#else
    std::string_view s = __PRETTY_FUNCTION__;
    std::size_t a = s.find("T = ");
    a = (a == std::string_view::npos) ? 0 : a + 4;
    std::size_t b = s.find_first_of(";]", a);
    return s.substr(a, b == std::string_view::npos ? std::string_view::npos : b - a);
#endif
}

template <class T>
TypeId type_id(const LayoutRef& layout = LayoutRef{});

// Class and union types are described by a generated function found through
// argument-dependent lookup (see the instrumenter). Anything without one is opaque.
template <class T>
void lattice_rt_describe(TypeId id, const T*) {
    auto n = pretty_name<T>();
    detail::set_name(id, n.data(), n.size());
    detail::define_opaque(id, sizeof(T));
}

template <class T>
void describe_type(TypeId id) {
    if constexpr (std::is_same_v<T, bool>) {
        detail::define_primitive(id, Prim::Bool, sizeof(T));
    } else if constexpr (std::is_same_v<T, char>) {
        detail::define_primitive(id, Prim::Char, sizeof(T));
    } else if constexpr (std::is_same_v<T, signed char>) {
        detail::define_primitive(id, Prim::SignedChar, sizeof(T));
    } else if constexpr (std::is_same_v<T, unsigned char>) {
        detail::define_primitive(id, Prim::UnsignedChar, sizeof(T));
    } else if constexpr (std::is_same_v<T, wchar_t>) {
        detail::define_primitive(id, Prim::WChar, sizeof(T));
    } else if constexpr (std::is_same_v<T, char16_t>) {
        detail::define_primitive(id, Prim::Char16, sizeof(T));
    } else if constexpr (std::is_same_v<T, char32_t>) {
        detail::define_primitive(id, Prim::Char32, sizeof(T));
    } else if constexpr (std::is_same_v<T, short>) {
        detail::define_primitive(id, Prim::Short, sizeof(T));
    } else if constexpr (std::is_same_v<T, unsigned short>) {
        detail::define_primitive(id, Prim::UnsignedShort, sizeof(T));
    } else if constexpr (std::is_same_v<T, int>) {
        detail::define_primitive(id, Prim::Int, sizeof(T));
    } else if constexpr (std::is_same_v<T, unsigned>) {
        detail::define_primitive(id, Prim::UnsignedInt, sizeof(T));
    } else if constexpr (std::is_same_v<T, long>) {
        detail::define_primitive(id, Prim::Long, sizeof(T));
    } else if constexpr (std::is_same_v<T, unsigned long>) {
        detail::define_primitive(id, Prim::UnsignedLong, sizeof(T));
    } else if constexpr (std::is_same_v<T, long long>) {
        detail::define_primitive(id, Prim::LongLong, sizeof(T));
    } else if constexpr (std::is_same_v<T, unsigned long long>) {
        detail::define_primitive(id, Prim::UnsignedLongLong, sizeof(T));
    } else if constexpr (std::is_same_v<T, float>) {
        detail::define_primitive(id, Prim::Float, sizeof(T));
    } else if constexpr (std::is_same_v<T, double>) {
        detail::define_primitive(id, Prim::Double, sizeof(T));
    } else if constexpr (std::is_same_v<T, long double>) {
        detail::define_primitive(id, Prim::LongDouble, sizeof(T));
    } else if constexpr (std::is_reference_v<T>) {
        // Rvalue references are modelled like lvalue ones: both are bindings.
        detail::define_reference(id, type_id<std::remove_reference_t<T>>(), sizeof(void*));
    } else if constexpr (std::is_pointer_v<T>) {
        detail::define_pointer(id, type_id<std::remove_pointer_t<T>>(), sizeof(T));
    } else if constexpr (std::is_array_v<T>) {
        detail::define_array(id, type_id<std::remove_extent_t<T>>(), std::extent_v<T>, sizeof(T));
    } else if constexpr (std::is_enum_v<T>) {
        auto n = pretty_name<T>();
        detail::set_name(id, n.data(), n.size());
        detail::define_enum(id, sizeof(T), std::is_signed_v<std::underlying_type_t<T>>);
    } else if constexpr (std::is_class_v<T> || std::is_union_v<T>) {
        lattice_rt_describe(id, static_cast<const T*>(nullptr));
    } else {
        // void, functions, references, member pointers, nullptr_t: named, not modelled.
        auto n = pretty_name<T>();
        detail::set_name(id, n.data(), n.size());
        if constexpr (std::is_object_v<T>) {
            detail::define_opaque(id, sizeof(T));
        } else {
            detail::define_opaque(id, 0);
        }
    }
}

template <class T>
TypeId type_id(const LayoutRef& layout) {
    using U = std::remove_cv_t<T>;
    static const TypeId id = detail::reserve_type();
    if (detail::begin_define(id)) {
        if (layout.layout != nullptr) {
            detail::define_from_layout(id, *layout.layout, layout.ext);
        } else {
            describe_type<U>(id);
        }
    }
    return id;
}

template <class T>
TypeId ext_id() {
    return type_id<T>();
}

// A layout with the project types it names (see `LayoutRef`).
template <class... Ts>
LayoutRef lay(const Layout& layout) {
    static const ExtFn fns[] = {&ext_id<Ts>..., nullptr};
    return LayoutRef{&layout, fns};
}

inline TypeId detail::prim_type_id(Prim p) {
    switch (p) {
        case Prim::Bool: return type_id<bool>();
        case Prim::Char: return type_id<char>();
        case Prim::SignedChar: return type_id<signed char>();
        case Prim::UnsignedChar: return type_id<unsigned char>();
        case Prim::WChar: return type_id<wchar_t>();
        case Prim::Char8: return type_id<char>();
        case Prim::Char16: return type_id<char16_t>();
        case Prim::Char32: return type_id<char32_t>();
        case Prim::Short: return type_id<short>();
        case Prim::UnsignedShort: return type_id<unsigned short>();
        case Prim::Int: return type_id<int>();
        case Prim::UnsignedInt: return type_id<unsigned>();
        case Prim::Long: return type_id<long>();
        case Prim::UnsignedLong: return type_id<unsigned long>();
        case Prim::LongLong: return type_id<long long>();
        case Prim::UnsignedLongLong: return type_id<unsigned long long>();
        case Prim::Float: return type_id<float>();
        case Prim::Double: return type_id<double>();
        case Prim::LongDouble: return type_id<long double>();
    }
    return type_id<int>();
}

// A plain address from a pointer of any cv-qualification (a `volatile int*` does
// not convert to `const void*` implicitly; user code may well have volatile variables).
template <class T>
const void* raw(T* p) {
    return const_cast<const void*>(static_cast<const volatile void*>(p));
}

// ---- generated-code entry points ------------------------------------------

template <class T>
NewTag tag(const Site& site, const LayoutRef& layout = LayoutRef{}) {
    return NewTag{site, type_id<T>(layout), 0};
}

// `new T[n]`. A type with a non-trivial destructor makes the compiler prepend an array
// cookie to the allocation and hand back an interior pointer, so the block would no
// longer be the array: those allocations are left untracked (`type == 0`).
template <class T>
NewTag tag_array(const Site& site, const LayoutRef& layout = LayoutRef{}) {
    if constexpr (std::is_trivially_destructible_v<T>) {
        return NewTag{site, type_id<T>(layout), sizeof(T)};
    } else {
        return NewTag{site, 0, 0};
    }
}

// Wraps a whole `new` expression: the object has finished constructing.
template <class T>
T* constructed(T* p) {
    detail::constructed(raw(p));
    return p;
}

// Wraps a whole `new T[n]` expression with no initializer: the elements of a scalar
// type are indeterminate, and are reported as such rather than read.
template <class T>
T* constructed_uninit(T* p) {
    detail::constructed_uninit(raw(p));
    return p;
}

// Wraps the operand of `delete`: remembers where the delete was written.
template <class T>
T* del_hint(T* p, const Site& site) {
    detail::note_delete(raw(p), site);
    return p;
}

// Entering a function / a block: constructed at the top of the body, so the
// matching exit runs on every way out (return, exception, goto).
struct Frame {
    Frame(const Site& site, const char* name) { detail::enter_function(site, name); }
    ~Frame() { detail::exit_function(); }
    Frame(const Frame&) = delete;
    Frame& operator=(const Frame&) = delete;
};

struct Scope {
    explicit Scope(const Site& site) { detail::enter_scope(site); }
    ~Scope() { detail::exit_scope(); }
    Scope(const Scope&) = delete;
    Scope& operator=(const Scope&) = delete;
};

// Variable declarations. `x` is taken by reference and never copied or modified.
template <class T>
void local(T& x, const char* name, const Site& site, const LayoutRef& layout = LayoutRef{}) {
    detail::declare(detail::VarKind::Local, name, type_id<T>(layout), raw(std::addressof(x)), sizeof(T), false, site);
}
template <class T>
void local_uninit(T& x, const char* name, const Site& site, const LayoutRef& layout = LayoutRef{}) {
    detail::declare(detail::VarKind::Local, name, type_id<T>(layout), raw(std::addressof(x)), sizeof(T), true, site);
}
template <class T>
void param(T& x, const char* name, const Site& site, const LayoutRef& layout = LayoutRef{}) {
    detail::declare(detail::VarKind::Parameter, name, type_id<T>(layout), raw(std::addressof(x)), sizeof(T), false, site);
}
// The referent's type is described first (with its layout, if any): the reference type
// is built from it.
template <class T>
void local_ref(T& x, const char* name, const Site& site, const LayoutRef& layout = LayoutRef{}) {
    TypeId referent = type_id<T>(layout);
    detail::declare_reference(detail::VarKind::Local, name, type_id<T&>(), referent, raw(std::addressof(x)), site);
}
template <class T>
void param_ref(T& x, const char* name, const Site& site, const LayoutRef& layout = LayoutRef{}) {
    TypeId referent = type_id<T>(layout);
    detail::declare_reference(detail::VarKind::Parameter, name, type_id<T&>(), referent, raw(std::addressof(x)), site);
}

// After a statement: report whatever changed in memory the program owns that its own
// instrumented code did not already report (calls into library code, constructors...).
inline void sync(const Site& site) {
    detail::sync(site);
}

// Wraps a postfix `x++` / `x--` (whose value is the *old* value): both arguments
// are evaluated before the call, so `p` already holds the new value when observed.
template <class T, class U>
T post_written(T old_value, U* p, const Site& site) {
    detail::after_write(raw(p), type_id<U>(), site);
    return old_value;
}

// Wraps `&(lhs = rhs)`: a scalar/pointer just changed. Returns the lvalue so the
// surrounding expression keeps its original type and value category.
template <class T>
T& written(T* p, const Site& site) {
    detail::after_write(raw(p), type_id<T>(), site);
    return *p;
}

}  // namespace lattice::rt

// Allocation function selected by the rewritten `new` expressions. Not a
// replacement of the global one: only tagged allocations become tracked objects.
void* operator new(std::size_t size, const lattice::rt::NewTag& tag);
void operator delete(void* p, const lattice::rt::NewTag& tag) noexcept;
void* operator new[](std::size_t size, const lattice::rt::NewTag& tag);
void operator delete[](void* p, const lattice::rt::NewTag& tag) noexcept;
