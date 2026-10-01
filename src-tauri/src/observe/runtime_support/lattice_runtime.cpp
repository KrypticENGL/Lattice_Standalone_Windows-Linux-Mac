// lattice-runtime implementation. See lattice_runtime.h.
//
// What this file is responsible for:
//   * logical object identity (ObjectId): monotonically increasing, never reused,
//     independent of addresses;
//   * a registry of live tracked objects, to map addresses to (object, field path);
//   * a registry of types, filled from compiler-generated descriptors;
//   * turning observations into events, one JSON object per line, in exactly the
//     shape Lattice's Rust `RuntimeEvent` (serde) deserializes;
//   * the transport: the events go to the file/pipe named by LATTICE_EVENT_SINK.
//     If it is unset the program runs normally and nothing is recorded.
//
// It never touches memory it was not told about: it reads only blocks it handed
// out itself through the tagged allocation function, using layouts the compiler
// produced (sizeof/offsetof).

#if defined(_MSC_VER) && !defined(_CRT_SECURE_NO_WARNINGS)
// getenv/fopen are fine here; runtime warnings must not leak into the user's build output.
#define _CRT_SECURE_NO_WARNINGS
#endif

#include "lattice_runtime.h"

#include <atomic>
#include <chrono>
#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <unordered_map>
#include <vector>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#endif

namespace lattice::rt {
namespace {

// Allocations made by the runtime itself (strings, maps) must never be tracked
// or recurse into the runtime.
thread_local bool t_inside = false;

// A spinlock rather than a mutex: std::mutex pulls in a pthread link dependency
// that not every toolchain satisfies implicitly, and this code is injected into
// builds we do not control. (Contention is irrelevant while the first milestone
// is single-threaded; a real multi-threaded design will replace it.)
struct SpinLock {
    std::atomic_flag flag = ATOMIC_FLAG_INIT;
    void lock() {
        while (flag.test_and_set(std::memory_order_acquire)) {
#if defined(_WIN32)
            SwitchToThread();  // the holder may be blocked writing to the sink
#endif
        }
    }
    bool try_lock() { return !flag.test_and_set(std::memory_order_acquire); }
    void unlock() { flag.clear(std::memory_order_release); }
};

struct Locked {
    SpinLock& l;
    explicit Locked(SpinLock& lock) : l(lock) { l.lock(); }
    ~Locked() { l.unlock(); }
    Locked(const Locked&) = delete;
    Locked& operator=(const Locked&) = delete;
};

struct Guard {
    bool previous;
    Guard() : previous(t_inside) { t_inside = true; }
    ~Guard() { t_inside = previous; }
    Guard(const Guard&) = delete;
    Guard& operator=(const Guard&) = delete;
};

enum class Kind { Undefined, Defining, Primitive, Enum, Pointer, Reference, Array, Record, Opaque };

struct FieldRec {
    std::string name;
    TypeId type;
    std::size_t offset;
};

struct TypeRec {
    Kind kind = Kind::Undefined;
    std::string name;
    std::size_t size = 0;
    Prim prim = Prim::Int;
    bool is_signed = false;
    TypeId elem = 0;  // pointee or element
    std::size_t len = 0;
    std::vector<FieldRec> fields;
};

struct Block {
    std::uint64_t id;
    std::size_t size;
    TypeId type;
    bool constructed;
    Site site;
    // Stack variables (as opposed to heap objects): their end is reported by the
    // scope/function they belong to, not by a deallocation.
    bool stack = false;
    std::uint64_t owner = 0;  // id of the stack entry (scope/frame) that declared it
    std::string last;         // last reported value, to report only real changes
};

// The runtime's view of the call stack: function frames and lexical scopes, in
// the order they were entered. Variables belong to the innermost entry.
struct VarRec {
    std::uintptr_t address;  // 0 for references (no storage of their own)
    std::uint64_t object;
};

struct Entry {
    bool is_frame;
    std::uint64_t id;
    std::vector<VarRec> vars;
};

struct State {
    SpinLock mu;
    std::FILE* out = nullptr;
    bool enabled = false;
    std::uint64_t seq = 0;
    // Event budget (LATTICE_EVENT_LIMIT); 0 = unlimited. When it is spent the
    // runtime records that once and stops observing.
    std::uint64_t limit = 0;
    std::uint64_t next_object = 1;
    std::uint64_t next_variable = 1;
    std::uint64_t next_frame = 1;
    std::uint64_t next_scope = 1;
    // One stack for the whole process: the first milestone with frames is
    // single-threaded (a per-thread stack is the multi-threading milestone).
    std::vector<Entry> stack;
    std::chrono::steady_clock::time_point t0 = std::chrono::steady_clock::now();
    std::vector<TypeRec> types;  // index == TypeId; [0] unused
    std::map<std::uintptr_t, Block> live;
    std::unordered_map<std::uintptr_t, Site> pending_delete;
    // Events waiting to be written. Guarded by `mu`, like everything else.
    std::string buf;

    State();
};

constexpr std::size_t kFlushBytes = 64 * 1024;

// Write everything pending. Caller holds the lock (or is in a crash path and
// accepts the risk).
void flush_locked(State& st) {
    if (st.buf.empty() || st.out == nullptr) return;
    std::fwrite(st.buf.data(), 1, st.buf.size(), st.out);
    std::fflush(st.out);
    st.buf.clear();
}

State& S();

void flush_now() {
    State& st = S();
    Locked lock(st.mu);
    flush_locked(st);
}

// Crash paths: best effort. If the interrupted code held the lock we write
// anyway; losing ordering is better than losing the tail of the run.
void flush_crash() {
    State& st = S();
    bool locked = st.mu.try_lock();
    flush_locked(st);
    if (locked) st.mu.unlock();
}

using SignalHandler = void (*)(int);
SignalHandler g_prev_abort = SIG_DFL;

void on_abort(int sig) {
    flush_crash();
    std::signal(sig, g_prev_abort == SIG_IGN ? SIG_DFL : g_prev_abort);
    std::raise(sig);
}

#if defined(_WIN32)
// Without a flusher, a program that hangs (infinite loop, deadlock) would keep
// its last events in the buffer until it is killed. This thread bounds the loss
// to one interval. (A Win32 thread: std::thread would add a pthread link
// dependency that not every toolchain satisfies implicitly.)
DWORD WINAPI flusher(LPVOID param) {
    State* st = static_cast<State*>(param);
    for (;;) {
        Sleep(20);
        Locked lock(st->mu);
        flush_locked(*st);
    }
}

LONG WINAPI on_unhandled_exception(EXCEPTION_POINTERS*) {
    flush_crash();
    return EXCEPTION_CONTINUE_SEARCH;
}
#endif

State::State() {
    types.emplace_back();
    const char* path = std::getenv("LATTICE_EVENT_SINK");
    if (path != nullptr && *path != 0) {
        out = std::fopen(path, "wb");
        enabled = (out != nullptr);
    }
    if (const char* lim = std::getenv("LATTICE_EVENT_LIMIT")) {
        limit = std::strtoull(lim, nullptr, 10);
    }
    if (!enabled) return;
    buf.reserve(kFlushBytes + 4096);
    std::atexit([] { flush_now(); });
    g_prev_abort = std::signal(SIGABRT, on_abort);
#if defined(_WIN32)
    SetUnhandledExceptionFilter(on_unhandled_exception);
    CreateThread(nullptr, 0, flusher, this, 0, nullptr);
#endif
}

// Intentionally leaked: objects may be deleted during static destruction.
State& S() {
    static State* s = [] {
        Guard g;
        return new State;
    }();
    return *s;
}

// ---- JSON ---------------------------------------------------------------

void esc(std::string& o, const char* s, std::size_t n) {
    o += '"';
    for (std::size_t i = 0; i < n; ++i) {
        unsigned char c = static_cast<unsigned char>(s[i]);
        switch (c) {
            case '"': o += "\\\""; break;
            case '\\': o += "\\\\"; break;
            case '\n': o += "\\n"; break;
            case '\r': o += "\\r"; break;
            case '\t': o += "\\t"; break;
            default:
                if (c < 0x20) {
                    char buf[8];
                    std::snprintf(buf, sizeof buf, "\\u%04x", c);
                    o += buf;
                } else {
                    o += static_cast<char>(c);
                }
        }
    }
    o += '"';
}

void esc(std::string& o, const std::string& s) { esc(o, s.data(), s.size()); }

void num(std::string& o, std::uint64_t v) { o += std::to_string(v); }

void site_json(std::string& o, const Site* s) {
    if (s == nullptr) {
        o += "null";
        return;
    }
    o += "{\"file\":";
    esc(o, s->file, std::strlen(s->file));
    o += ",\"line\":" + std::to_string(s->line);
    o += ",\"column\":";
    o += s->column > 0 ? std::to_string(s->column) : "null";
    o += ",\"function\":";
    if (s->function != nullptr && *s->function != '\0') {
        esc(o, s->function, std::strlen(s->function));
    } else {
        o += "null";
    }
    o += ",\"instruction\":null}";
}

// Caller holds the lock.
void emit(State& st, const Site* site, const std::string& kind) {
    if (!st.enabled) return;
    if (st.limit != 0 && st.seq >= st.limit) {
        // The budget is spent. Say so (once, as the very last event) and stop
        // observing: every hook checks `enabled`, so the program also stops paying
        // for observation. The program itself keeps running unchanged.
        st.buf += "{\"seq\":" + std::to_string(st.seq++) +
                  ",\"thread\":0,\"timestamp_ns\":null,\"location\":null,\"kind\":"
                  "{\"event\":\"observation_truncated\",\"limit\":" + std::to_string(st.limit) + "}}";
        st.buf += static_cast<char>(10);
        st.enabled = false;
#if !defined(_WIN32)
        flush_locked(st);
#endif
        return;
    }
    auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                  std::chrono::steady_clock::now() - st.t0)
                  .count();
    std::string& line = st.buf;
    line += "{\"seq\":" + std::to_string(st.seq++);
    line += ",\"thread\":0,\"timestamp_ns\":" + std::to_string(ns);
    line += ",\"location\":";
    site_json(line, site);
    line += ",\"kind\":";
    line += kind;
    line += "}\n";
    // Batched: written when enough is pending, by the flusher thread shortly
    // after, at exit, or on a crash. Without a flusher thread (non-Windows
    // builds) every event is written immediately.
#if defined(_WIN32)
    if (line.size() >= kFlushBytes) flush_locked(st);
#else
    flush_locked(st);
#endif
}

const char* prim_json(Prim p) {
    switch (p) {
        case Prim::Bool: return "bool";
        case Prim::Char: return "char";
        case Prim::SignedChar: return "signed_char";
        case Prim::UnsignedChar: return "unsigned_char";
        case Prim::WChar: return "w_char";
        case Prim::Char8: return "char8";
        case Prim::Char16: return "char16";
        case Prim::Char32: return "char32";
        case Prim::Short: return "short";
        case Prim::UnsignedShort: return "unsigned_short";
        case Prim::Int: return "int";
        case Prim::UnsignedInt: return "unsigned_int";
        case Prim::Long: return "long";
        case Prim::UnsignedLong: return "unsigned_long";
        case Prim::LongLong: return "long_long";
        case Prim::UnsignedLongLong: return "unsigned_long_long";
        case Prim::Float: return "float";
        case Prim::Double: return "double";
        case Prim::LongDouble: return "long_double";
    }
    return "int";
}

const char* prim_c_name(Prim p) {
    switch (p) {
        case Prim::Bool: return "bool";
        case Prim::Char: return "char";
        case Prim::SignedChar: return "signed char";
        case Prim::UnsignedChar: return "unsigned char";
        case Prim::WChar: return "wchar_t";
        case Prim::Char8: return "char8_t";
        case Prim::Char16: return "char16_t";
        case Prim::Char32: return "char32_t";
        case Prim::Short: return "short";
        case Prim::UnsignedShort: return "unsigned short";
        case Prim::Int: return "int";
        case Prim::UnsignedInt: return "unsigned int";
        case Prim::Long: return "long";
        case Prim::UnsignedLong: return "unsigned long";
        case Prim::LongLong: return "long long";
        case Prim::UnsignedLongLong: return "unsigned long long";
        case Prim::Float: return "float";
        case Prim::Double: return "double";
        case Prim::LongDouble: return "long double";
    }
    return "int";
}

// ---- types -------------------------------------------------------------

void emit_type_declared(State& st, TypeId id) {
    if (!st.enabled) return;
    const TypeRec& t = st.types[id];
    std::string k = "{\"event\":\"type_declared\",\"def\":{\"id\":" + std::to_string(id) + ",\"name\":";
    esc(k, t.name);
    k += ",\"kind\":";
    switch (t.kind) {
        case Kind::Primitive:
            k += std::string("{\"kind\":\"primitive\",\"primitive\":\"") + prim_json(t.prim) + "\"}";
            break;
        case Kind::Enum:
            k += "{\"kind\":\"enum\",\"underlying\":null,\"enumerators\":[],\"scoped\":false}";
            break;
        case Kind::Pointer:
            k += "{\"kind\":\"pointer\",\"pointee\":" + std::to_string(t.elem) + "}";
            break;
        case Kind::Reference:
            k += "{\"kind\":\"l_value_reference\",\"referent\":" + std::to_string(t.elem) + "}";
            break;
        case Kind::Array:
            k += "{\"kind\":\"array\",\"element\":" + std::to_string(t.elem) +
                 ",\"len\":" + std::to_string(t.len) + "}";
            break;
        case Kind::Record: {
            k += "{\"kind\":\"record\",\"record\":\"struct\",\"fields\":[";
            for (std::size_t i = 0; i < t.fields.size(); ++i) {
                if (i) k += ',';
                k += "{\"name\":";
                esc(k, t.fields[i].name);
                k += ",\"ty\":" + std::to_string(t.fields[i].type);
                k += ",\"is_base\":false,\"offset\":" + std::to_string(t.fields[i].offset) + "}";
            }
            k += "],\"template_args\":[]}";
            break;
        }
        default:
            k += "{\"kind\":\"opaque\"}";
    }
    k += ",\"size\":";
    k += t.size ? std::to_string(t.size) : "null";
    k += ",\"qualifiers\":{\"is_const\":false,\"is_volatile\":false}}}";
    emit(st, nullptr, k);
}

// ---- values -------------------------------------------------------------

void unavailable(std::string& o, const char* reason) {
    o += "{\"type\":\"unavailable\",\"unavailable\":{\"reason\":\"";
    o += reason;
    o += "\"}}";
}

void invalid(std::string& o, const std::string& raw) {
    o += "{\"type\":\"unavailable\",\"unavailable\":{\"reason\":\"invalid\",\"raw\":";
    esc(o, raw);
    o += "}}";
}

struct Step {
    bool field;
    std::uint64_t index;
};

// Find the place inside `type` (at byte `off`) that holds a `want`. Prefers the
// outermost match so `&node` is the node, not its first field.
bool resolve(const State& st, TypeId type, std::size_t off, TypeId want, std::vector<Step>& path) {
    const TypeRec& t = st.types[type];
    if (off == 0 && type == want) return true;
    if (t.kind == Kind::Record) {
        for (std::size_t i = 0; i < t.fields.size(); ++i) {
            const FieldRec& f = t.fields[i];
            std::size_t fsize = st.types[f.type].size;
            if (off >= f.offset && off < f.offset + (fsize ? fsize : 1)) {
                path.push_back({true, i});
                if (resolve(st, f.type, off - f.offset, want, path)) return true;
                path.pop_back();
                return false;
            }
        }
        return false;
    }
    if (t.kind == Kind::Array) {
        std::size_t esize = st.types[t.elem].size;
        if (esize == 0) return false;
        std::size_t idx = off / esize;
        if (idx >= t.len) return false;
        path.push_back({false, idx});
        if (resolve(st, t.elem, off % esize, want, path)) return true;
        path.pop_back();
        return false;
    }
    return false;
}

struct Located {
    std::uint64_t object = 0;
    std::vector<Step> path;
    bool ok = false;
};

Located locate(const State& st, std::uintptr_t addr, TypeId want) {
    Located r;
    auto it = st.live.upper_bound(addr);
    if (it == st.live.begin()) return r;
    --it;
    const Block& b = it->second;
    std::size_t off = addr - it->first;
    if (off >= b.size) return r;  // includes one-past-the-end: not a tracked place
    r.object = b.id;
    if (resolve(st, b.type, off, want, r.path)) {
        r.ok = true;
    } else if (off == 0) {
        r.path.clear();  // untyped pointer to the start of an object
        r.ok = true;
    }
    return r;
}

void path_json(std::string& o, const std::vector<Step>& path) {
    o += '[';
    for (std::size_t i = 0; i < path.size(); ++i) {
        if (i) o += ',';
        o += path[i].field ? "{\"field\":" : "{\"index\":";
        num(o, path[i].index);
        o += '}';
    }
    o += ']';
}

void place_json(std::string& o, const Located& l) {
    o += "{\"object\":";
    num(o, l.object);
    o += ",\"path\":";
    path_json(o, l.path);
    o += '}';
}

void pointer_json(std::string& o, const State& st, std::uintptr_t v, TypeId pointee) {
    o += "{\"type\":\"pointer\",\"pointer\":{\"address\":";
    num(o, v);
    o += ",\"target\":";
    if (v == 0) {
        o += "{\"target\":\"null\"}";
    } else {
        Located l = locate(st, v, pointee);
        if (l.ok) {
            o += "{\"target\":\"place\",\"place\":";
            place_json(o, l);
            o += '}';
        } else {
            o += "{\"target\":\"unresolved\"}";
        }
    }
    o += "}}";
}

template <class T>
T load(const unsigned char* p) {
    T v;
    std::memcpy(&v, p, sizeof v);
    return v;
}

std::int64_t load_signed(const unsigned char* p, std::size_t size) {
    switch (size) {
        case 1: return load<std::int8_t>(p);
        case 2: return load<std::int16_t>(p);
        case 4: return load<std::int32_t>(p);
        default: return load<std::int64_t>(p);
    }
}

std::uint64_t load_unsigned(const unsigned char* p, std::size_t size) {
    switch (size) {
        case 1: return load<std::uint8_t>(p);
        case 2: return load<std::uint16_t>(p);
        case 4: return load<std::uint32_t>(p);
        default: return load<std::uint64_t>(p);
    }
}

void float_json(std::string& o, double v) {
    if (std::isnan(v) || std::isinf(v)) {
        invalid(o, std::isnan(v) ? "nan" : (v < 0 ? "-inf" : "inf"));
        return;
    }
    char buf[40];
    std::snprintf(buf, sizeof buf, "%.17g", v);
    o += "{\"type\":\"float\",\"value\":";
    o += buf;
    // JSON numbers without '.', 'e' are read back as integers by some decoders;
    // serde accepts either for f64.
    o += '}';
}

constexpr std::size_t kMaxArrayElements = 4096;

void value_json(std::string& o, const State& st, TypeId type, const unsigned char* p) {
    const TypeRec& t = st.types[type];
    switch (t.kind) {
        case Kind::Primitive: {
            switch (t.prim) {
                case Prim::Bool: {
                    unsigned v = load<std::uint8_t>(p);
                    if (v > 1) {
                        invalid(o, "bool=" + std::to_string(v));
                    } else {
                        o += v ? "{\"type\":\"bool\",\"value\":true}" : "{\"type\":\"bool\",\"value\":false}";
                    }
                    return;
                }
                case Prim::WChar:
                case Prim::Char8:
                case Prim::Char16:
                case Prim::Char32:
                case Prim::Char:
                    o += "{\"type\":\"char\",\"value\":" + std::to_string(load_unsigned(p, t.size)) + "}";
                    return;
                case Prim::Float: float_json(o, load<float>(p)); return;
                case Prim::Double: float_json(o, load<double>(p)); return;
                case Prim::LongDouble:
                    if (t.size == sizeof(long double)) {
                        float_json(o, static_cast<double>(load<long double>(p)));
                    } else {
                        unavailable(o, "unknown");
                    }
                    return;
                case Prim::UnsignedChar:
                case Prim::UnsignedShort:
                case Prim::UnsignedInt:
                case Prim::UnsignedLong:
                case Prim::UnsignedLongLong:
                    o += "{\"type\":\"u_int\",\"value\":" + std::to_string(load_unsigned(p, t.size)) + "}";
                    return;
                default:
                    o += "{\"type\":\"int\",\"value\":" + std::to_string(load_signed(p, t.size)) + "}";
                    return;
            }
        }
        case Kind::Enum:
            o += "{\"type\":\"enum\",\"value\":" +
                 std::to_string(t.is_signed ? load_signed(p, t.size)
                                            : static_cast<std::int64_t>(load_unsigned(p, t.size))) +
                 ",\"enumerator\":null}";
            return;
        case Kind::Pointer:
            pointer_json(o, st, load<std::uintptr_t>(p), t.elem);
            return;
        case Kind::Array: {
            std::size_t esize = st.types[t.elem].size;
            if (t.len > kMaxArrayElements || esize == 0) {
                unavailable(o, "unknown");  // large arrays: a future milestone (chunking)
                return;
            }
            o += "{\"type\":\"array\",\"elements\":[";
            for (std::size_t i = 0; i < t.len; ++i) {
                if (i) o += ',';
                value_json(o, st, t.elem, p + i * esize);
            }
            o += "]}";
            return;
        }
        case Kind::Record: {
            o += "{\"type\":\"aggregate\",\"fields\":[";
            for (std::size_t i = 0; i < t.fields.size(); ++i) {
                if (i) o += ',';
                value_json(o, st, t.fields[i].type, p + t.fields[i].offset);
            }
            o += "]}";
            return;
        }
        default:
            unavailable(o, "unknown");
    }
}

// The value an object has before its constructor ran: right shape, no content.
void shape_json(std::string& o, const State& st, TypeId type) {
    const TypeRec& t = st.types[type];
    if (t.kind == Kind::Record) {
        o += "{\"type\":\"aggregate\",\"fields\":[";
        for (std::size_t i = 0; i < t.fields.size(); ++i) {
            if (i) o += ',';
            shape_json(o, st, t.fields[i].type);
        }
        o += "]}";
    } else if (t.kind == Kind::Array && t.len <= kMaxArrayElements && st.types[t.elem].size != 0) {
        o += "{\"type\":\"array\",\"elements\":[";
        for (std::size_t i = 0; i < t.len; ++i) {
            if (i) o += ',';
            shape_json(o, st, t.elem);
        }
        o += "]}";
    } else {
        unavailable(o, "uninitialized");
    }
}

void emit_value_changed(State& st, const Site* site, std::uint64_t object,
                        const std::vector<Step>& path, const std::string& value) {
    std::string k = "{\"event\":\"value_changed\",\"place\":{\"object\":" + std::to_string(object) + ",\"path\":";
    path_json(k, path);
    k += "},\"value\":";
    k += value;
    k += '}';
    emit(st, site, k);
}

void destroy_block(State& st, std::map<std::uintptr_t, Block>::iterator it) {
    Site site{};
    const Site* site_ptr = nullptr;
    auto pend = st.pending_delete.find(it->first);
    if (pend != st.pending_delete.end()) {
        site = pend->second;
        site_ptr = &site;
        st.pending_delete.erase(pend);
    }
    emit(st, site_ptr,
         "{\"event\":\"object_destroyed\",\"object\":" + std::to_string(it->second.id) +
             ",\"reason\":\"freed\"}");
    st.live.erase(it);
}

}  // namespace

// ---- detail: types ---------------------------------------------------------

namespace detail {

TypeId reserve_type() {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    st.types.emplace_back();
    return static_cast<TypeId>(st.types.size() - 1);
}

bool begin_define(TypeId id) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (st.types[id].kind != Kind::Undefined) return false;
    st.types[id].kind = Kind::Defining;
    return true;
}

void set_name(TypeId id, const char* name, std::size_t len) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    st.types[id].name.assign(name, len);
}

void define_primitive(TypeId id, Prim prim, std::size_t size) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Primitive;
    t.prim = prim;
    t.size = size;
    t.name = prim_c_name(prim);
    emit_type_declared(st, id);
}

void define_enum(TypeId id, std::size_t size, bool is_signed) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Enum;
    t.size = size;
    t.is_signed = is_signed;
    emit_type_declared(st, id);
}

void define_pointer(TypeId id, TypeId pointee, std::size_t size) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Pointer;
    t.elem = pointee;
    t.size = size;
    const std::string& pn = st.types[pointee].name;
    t.name = (pn.empty() ? std::string("?") : pn) + "*";
    emit_type_declared(st, id);
}

void define_array(TypeId id, TypeId element, std::size_t len, std::size_t size) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Array;
    t.elem = element;
    t.len = len;
    t.size = size;
    const std::string& en = st.types[element].name;
    t.name = (en.empty() ? std::string("?") : en) + "[" + std::to_string(len) + "]";
    emit_type_declared(st, id);
}

void define_record(TypeId id, std::size_t size, const FieldInfo* fields, std::size_t count) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Record;
    t.size = size;
    t.fields.clear();
    for (std::size_t i = 0; i < count; ++i) {
        t.fields.push_back({fields[i].name, fields[i].type, fields[i].offset});
    }
    emit_type_declared(st, id);
}

void define_opaque(TypeId id, std::size_t size) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Opaque;
    t.size = size;
    emit_type_declared(st, id);
}

void define_reference(TypeId id, TypeId referent, std::size_t size) {
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    TypeRec& t = st.types[id];
    t.kind = Kind::Reference;
    t.elem = referent;
    t.size = size;
    const std::string& en = st.types[referent].name;
    t.name = (en.empty() ? std::string("?") : en) + "&";
    emit_type_declared(st, id);
}

// ---- detail: frames, scopes, variables -------------------------------------------

namespace {

// The object ended with its scope: forget where it lived so a later variable at the
// same stack address is a new object. (The model ends it itself when the scope or
// function exits, so no event is emitted for the object.)
void release_vars(State& st, const Entry& e) {
    for (const VarRec& v : e.vars) {
        if (v.address == 0) continue;
        auto it = st.live.find(v.address);
        if (it != st.live.end() && it->second.id == v.object) st.live.erase(it);
    }
}

std::uint64_t nearest_frame(const State& st) {
    for (auto it = st.stack.rbegin(); it != st.stack.rend(); ++it) {
        if (it->is_frame) return it->id;
    }
    return 0;
}

// A stack address can only hold one variable at a time; stale entries (from an
// exit that skipped normal unwinding) must not shadow a new variable.
void evict_stale_stack_blocks(State& st, std::uintptr_t addr, std::size_t size) {
    auto it = st.live.upper_bound(addr);
    if (it != st.live.begin()) {
        auto prev = std::prev(it);
        if (prev->second.stack && addr < prev->first + prev->second.size) st.live.erase(prev);
    }
    it = st.live.lower_bound(addr);
    while (it != st.live.end() && it->first < addr + (size ? size : 1)) {
        it = it->second.stack ? st.live.erase(it) : std::next(it);
    }
}

const char* var_kind_json(VarKind k) { return k == VarKind::Parameter ? "parameter" : "local"; }

void emit_variable(State& st, const Site& site, VarKind kind, const char* name, std::uint64_t object) {
    const Entry& top = st.stack.back();
    std::string k = "{\"event\":\"variable_created\",\"variable\":{\"id\":" + std::to_string(st.next_variable++);
    k += ",\"name\":";
    esc(k, name, std::strlen(name));
    k += ",\"kind\":\"";
    k += var_kind_json(kind);
    k += "\",\"frame\":" + std::to_string(nearest_frame(st));
    k += ",\"scope\":";
    k += top.is_frame ? std::string("null") : std::to_string(top.id);
    k += ",\"object\":" + std::to_string(object) + "}}";
    emit(st, &site, k);
}

}  // namespace

void enter_function(const Site& site, const char* name) {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    std::uint64_t id = st.next_frame++;
    st.stack.push_back(Entry{true, id, {}});
    std::string k = "{\"event\":\"function_entered\",\"frame\":" + std::to_string(id) + ",\"function\":";
    esc(k, name, std::strlen(name));
    k += ",\"call_site\":null}";
    emit(st, &site, k);
}

void exit_function() {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    // Scopes left open (an exit that skipped their guards) end with the function.
    while (!st.stack.empty()) {
        Entry e = std::move(st.stack.back());
        st.stack.pop_back();
        release_vars(st, e);
        if (e.is_frame) {
            emit(st, nullptr, "{\"event\":\"function_exited\",\"frame\":" + std::to_string(e.id) + "}");
            break;
        }
    }
}

void enter_scope(const Site& site) {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    std::uint64_t frame = nearest_frame(st);
    if (frame == 0) return;  // not inside an observed function
    std::uint64_t id = st.next_scope++;
    st.stack.push_back(Entry{false, id, {}});
    emit(st, &site,
         "{\"event\":\"scope_entered\",\"scope\":" + std::to_string(id) + ",\"frame\":" + std::to_string(frame) + "}");
}

void exit_scope() {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled || st.stack.empty() || st.stack.back().is_frame) return;
    Entry e = std::move(st.stack.back());
    st.stack.pop_back();
    release_vars(st, e);
    emit(st, nullptr, "{\"event\":\"scope_exited\",\"scope\":" + std::to_string(e.id) + "}");
}

void declare(VarKind kind, const char* name, TypeId type, const void* address, std::size_t size,
             bool uninitialized, const Site& site) {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled || st.stack.empty()) return;
    Entry& top = st.stack.back();
    auto addr = reinterpret_cast<std::uintptr_t>(address);
    const auto* mem = static_cast<const unsigned char*>(address);

    // Declared earlier in this same scope (a `for` loop variable is offered again on
    // every iteration): report only what changed.
    auto known = st.live.find(addr);
    if (known != st.live.end() && known->second.stack && known->second.owner == top.id &&
        known->second.type == type) {
        if (!uninitialized) {
            std::string v;
            value_json(v, st, type, mem);
            if (v != known->second.last) {
                known->second.last = v;
                emit_value_changed(st, &site, known->second.id, {}, v);
            }
        }
        return;
    }

    evict_stale_stack_blocks(st, addr, size);
    std::uint64_t object = st.next_object++;
    std::string value;
    if (uninitialized) {
        shape_json(value, st, type);
    } else {
        value_json(value, st, type, mem);
    }
    st.live[addr] = Block{object, size, type, true, site, true, top.id, uninitialized ? std::string() : value};

    std::string k = "{\"event\":\"object_allocated\",\"object\":{\"id\":" + std::to_string(object);
    k += ",\"ty\":" + std::to_string(type);
    k += ",\"storage\":\"automatic\",\"address\":" + std::to_string(addr);
    k += ",\"size\":" + std::to_string(size);
    k += ",\"state\":\"alive\",\"value\":";
    k += value;
    k += "}}";
    emit(st, &site, k);
    emit_variable(st, site, kind, name, object);
    top.vars.push_back({addr, object});
}

void declare_reference(VarKind kind, const char* name, TypeId ref_type, TypeId referent,
                       const void* address, const Site& site) {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled || st.stack.empty()) return;
    Entry& top = st.stack.back();
    std::uint64_t object = st.next_object++;

    // A reference has no storage of its own to describe: its value is what it is
    // bound to. Writes through it are observed at the referent.
    std::string value = "{\"type\":\"reference\",\"target\":";
    Located l = locate(st, reinterpret_cast<std::uintptr_t>(address), referent);
    if (l.ok) {
        value += "{\"target\":\"place\",\"place\":";
        place_json(value, l);
        value += '}';
    } else {
        value += "{\"target\":\"unresolved\"}";
    }
    value += '}';

    std::string k = "{\"event\":\"object_allocated\",\"object\":{\"id\":" + std::to_string(object);
    k += ",\"ty\":" + std::to_string(ref_type);
    k += ",\"storage\":\"automatic\",\"address\":null,\"size\":null,\"state\":\"alive\",\"value\":";
    k += value;
    k += "}}";
    emit(st, &site, k);
    emit_variable(st, site, kind, name, object);
    top.vars.push_back({0, object});
}

// ---- detail: observation ----------------------------------------------------

void after_write(const void* address, TypeId type, const Site& site) {
    if (t_inside) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    auto addr = reinterpret_cast<std::uintptr_t>(address);
    Located l = locate(st, addr, type);
    if (!l.ok) return;  // not inside a tracked object (e.g. a stack variable): not observed yet
    std::string value;
    value_json(value, st, type, static_cast<const unsigned char*>(address));
    emit_value_changed(st, &site, l.object, l.path, value);
}

void note_delete(const void* address, const Site& site) {
    if (t_inside || address == nullptr) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    auto addr = reinterpret_cast<std::uintptr_t>(address);
    if (st.live.count(addr)) st.pending_delete[addr] = site;
}

void constructed(const void* address) {
    if (t_inside || address == nullptr) return;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return;
    auto it = st.live.find(reinterpret_cast<std::uintptr_t>(address));
    if (it == st.live.end() || it->second.constructed) return;
    Block& b = it->second;
    b.constructed = true;
    emit(st, &b.site, "{\"event\":\"object_constructed\",\"object\":" + std::to_string(b.id) + "}");

    // The constructor has run: report what it left behind. The constructor's own
    // writes happened before this point and were not individually observed (only
    // assignments in instrumented code are), so this is the authoritative value.
    const TypeRec& t = st.types[b.type];
    const auto* base = static_cast<const unsigned char*>(address);
    if (t.kind == Kind::Record) {
        for (std::size_t i = 0; i < t.fields.size(); ++i) {
            std::string v;
            value_json(v, st, t.fields[i].type, base + t.fields[i].offset);
            emit_value_changed(st, &b.site, b.id, {{true, i}}, v);
        }
    } else {
        std::string v;
        value_json(v, st, b.type, base);
        emit_value_changed(st, &b.site, b.id, {}, v);
    }
}

}  // namespace detail
}  // namespace lattice::rt

// ---- allocation functions ----------------------------------------------------
//
// The global allocation functions are replaced (malloc/free) so that *every*
// deallocation of a tracked block is seen, whoever performs it: user code,
// unique_ptr, containers. Allocation is tracked only when a rewritten `new`
// expression selects the tagged overload below; everything else passes through
// untouched (and unobserved).

namespace {

void* raw_allocate(std::size_t size) {
    void* p = std::malloc(size != 0 ? size : 1);
    if (p == nullptr) throw std::bad_alloc();
    return p;
}

void release(void* p) noexcept {
    if (p == nullptr) return;
    using namespace lattice::rt;
    if (!t_inside) {
        Guard g;
        State& st = S();
        Locked lock(st.mu);
        if (st.enabled) {
            auto it = st.live.find(reinterpret_cast<std::uintptr_t>(p));
            if (it != st.live.end()) destroy_block(st, it);
        }
    }
    std::free(p);
}

}  // namespace

void* operator new(std::size_t size) { return raw_allocate(size); }
void* operator new[](std::size_t size) { return raw_allocate(size); }
void operator delete(void* p) noexcept { release(p); }
void operator delete[](void* p) noexcept { release(p); }
void operator delete(void* p, std::size_t) noexcept { release(p); }
void operator delete[](void* p, std::size_t) noexcept { release(p); }

void* operator new(std::size_t size, const lattice::rt::NewTag& tag) {
    using namespace lattice::rt;
    void* p = raw_allocate(size);
    if (t_inside) return p;
    Guard g;
    State& st = S();
    Locked lock(st.mu);
    if (!st.enabled) return p;
    // A fresh logical id for every allocation, so an address that is reused
    // later is still a different object.
    std::uint64_t id = st.next_object++;
    st.live[reinterpret_cast<std::uintptr_t>(p)] = Block{id, size, tag.type, false, tag.site, false, 0, std::string()};
    std::string k = "{\"event\":\"object_allocated\",\"object\":{\"id\":" + std::to_string(id);
    k += ",\"ty\":" + std::to_string(tag.type);
    k += ",\"storage\":\"heap\",\"address\":" + std::to_string(reinterpret_cast<std::uintptr_t>(p));
    k += ",\"size\":" + std::to_string(size);
    k += ",\"state\":\"allocated\",\"value\":";
    shape_json(k, st, tag.type);
    k += "}}";
    emit(st, &tag.site, k);
    return p;
}

// Called by the language if the constructor of a tagged `new` throws.
void operator delete(void* p, const lattice::rt::NewTag&) noexcept { release(p); }
