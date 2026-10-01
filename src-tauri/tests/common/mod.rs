//! Shared helpers for the observation end-to-end tests.
//!
//! Each test runs the *real* pipeline: libclang analysis -> source
//! instrumentation -> the existing compiler -> the existing execution manager ->
//! events from the running program -> the existing URR. Nothing is faked.
//!
//! Needs a libclang (analysis) and a C++ compiler (build). Without them the tests
//! skip loudly; set LATTICE_REQUIRE_LIBCLANG=1 to fail instead, and
//! LATTICE_LIBCLANG=<path to libclang.dll> to point at one.
#![allow(dead_code)] // each test binary uses a subset

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lattice_lib::config::{CleanupPolicy, ExecutionConfig};
use lattice_lib::model::*;
use lattice_lib::observe::discovery::find_libclang;
use lattice_lib::observe::{Observation, ObservingInstrumenter, Transport};
use lattice_lib::runtime::{ExecutionManager, RunRequest, RuntimeSession, SourceFile};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub fn temp_root() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lattice-obs-test-{}-{n}", std::process::id()))
}

pub fn request(src: &str) -> RunRequest {
    RunRequest {
        project: "test".into(),
        files: vec![SourceFile { name: "main.cpp".into(), contents: src.into() }],
        observe: false,
    }
}

pub fn config() -> ExecutionConfig {
    ExecutionConfig {
        runtime_root: temp_root(),
        run_timeout: Duration::from_secs(20),
        cleanup: CleanupPolicy::Always,
        ..ExecutionConfig::default()
    }
}

/// An execution manager with the observing instrumenter, or `None` (skip).
pub fn observed() -> Option<(ExecutionManager, Arc<ObservingInstrumenter>)> {
    observed_with(|_| {}, None)
}

pub fn observed_with(
    tweak: impl FnOnce(&mut ExecutionConfig),
    transport: Option<Transport>,
) -> Option<(ExecutionManager, Arc<ObservingInstrumenter>)> {
    let Some(libclang) = find_libclang() else {
        if std::env::var_os("LATTICE_REQUIRE_LIBCLANG").is_some() {
            panic!("no libclang found");
        }
        eprintln!("SKIPPED: no libclang found (set LATTICE_LIBCLANG)");
        return None;
    };
    let mut cfg = config();
    tweak(&mut cfg);
    let std_version = cfg.cxx_standard.clone();
    let plain = ExecutionManager::new(cfg);
    let Some(compiler) = plain.compiler_info() else {
        eprintln!("SKIPPED: no C++ compiler found");
        return None;
    };
    let mut inst = ObservingInstrumenter::new(libclang, &compiler.path, &std_version);
    if let Some(t) = transport {
        inst = inst.with_transport(t);
    }
    let inst = Arc::new(inst);
    Some((plain.with_instrumenter(inst.clone()), inst))
}

pub fn observe(src: &str) -> Option<(RuntimeSession, Observation)> {
    let (m, inst) = observed()?;
    let session = m.run(request(src), &|_| {});
    let root = session.workspace_path.clone().expect("workspace");
    let obs = inst.take_observation(&root).expect("an observation for this session");
    Some((session, obs))
}

pub fn path_text(path: &[Step]) -> String {
    path.iter()
        .map(|s| match s {
            Step::Field(i) => format!(".{i}"),
            Step::Index(i) => format!("[{i}]"),
        })
        .collect()
}

/// Events about **heap** objects only, with objects numbered 1, 2, ... in
/// allocation order. Stack variables (frames, locals) are observed too and take
/// object ids of their own, interleaved with the heap's; this view ignores them.
pub fn outline(t: &Timeline) -> Vec<String> {
    let mut ordinal: std::collections::HashMap<u64, usize> = Default::default();
    for e in t.events() {
        if let EventKind::ObjectAllocated { object } = &e.kind {
            if object.storage == StorageClass::Heap {
                let n = ordinal.len() + 1;
                ordinal.insert(object.id.0, n);
            }
        }
    }
    t.events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ObjectAllocated { object } => {
                ordinal.get(&object.id.0).map(|n| format!("allocated {n}"))
            }
            EventKind::ObjectConstructed { object } => {
                ordinal.get(&object.0).map(|n| format!("constructed {n}"))
            }
            EventKind::ObjectDestroyed { object, .. } => {
                ordinal.get(&object.0).map(|n| format!("destroyed {n}"))
            }
            EventKind::ValueChanged { place, .. } => ordinal
                .get(&place.object.0)
                .map(|n| format!("changed {n}{}", path_text(&place.path))),
            _ => None,
        })
        .collect()
}

/// Every event but type declarations, described by kind: frames, scopes,
/// variables and all objects. Object ids are the real ones.
pub fn full_outline(t: &Timeline) -> Vec<String> {
    t.events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TypeDeclared { .. } => None,
            EventKind::FunctionEntered { function, .. } => Some(format!("enter {function}")),
            EventKind::FunctionExited { .. } => Some("exit".into()),
            EventKind::ScopeEntered { .. } => Some("scope+".into()),
            EventKind::ScopeExited { .. } => Some("scope-".into()),
            EventKind::VariableCreated { variable } => Some(format!("var {}", variable.name)),
            EventKind::ObjectAllocated { object } => Some(format!("alloc {:?} #{}", object.storage, object.id.0)),
            EventKind::ObjectConstructed { object } => Some(format!("constructed #{}", object.0)),
            EventKind::ObjectDestroyed { object, .. } => Some(format!("destroy #{}", object.0)),
            EventKind::ValueChanged { place, .. } => {
                Some(format!("set #{}{}", place.object.0, path_text(&place.path)))
            }
            other => Some(format!("{other:?}")),
        })
        .collect()
}

/// Ids of the heap objects in `state`, in allocation order.
pub fn heap_ids(state: &RuntimeState) -> Vec<ObjectId> {
    state.objects().filter(|o| o.storage == StorageClass::Heap).map(|o| o.id).collect()
}

pub fn field_named(state: &RuntimeState, ty: TypeId, name: &str) -> u32 {
    state.types().fields(ty).unwrap().iter().position(|f| f.name == name).unwrap() as u32
}


/// Names of the variables of `frame`, in creation order.
pub fn var_names(state: &RuntimeState, frame: FrameId) -> Vec<String> {
    state.frame_variables(frame).map(|v| v.name.clone()).collect()
}

/// Function names on the main thread's call stack, outermost first.
pub fn stack_names(state: &RuntimeState) -> Vec<String> {
    state.stack(ThreadId::MAIN).iter().map(|f| state.frame(*f).unwrap().function.to_string()).collect()
}

/// The state after every event of the run, in order.
pub fn snapshots(t: &Timeline) -> Vec<RuntimeSnapshot> {
    (0..t.len() as u64).map(|n| t.snapshot_after(EventSeq(n)).unwrap()).collect()
}

/// Every object a variable called `name` was ever bound to (in order of first
/// appearance), with the distinct consecutive values it held while the variable
/// existed. Two variables with the same name in different frames or iterations are
/// different objects.
pub fn history(snaps: &[RuntimeSnapshot], name: &str) -> Vec<(ObjectId, Vec<Value>)> {
    let mut out: Vec<(ObjectId, Vec<Value>)> = Vec::new();
    for s in snaps {
        for v in s.variables().filter(|v| v.name == name) {
            let Some(val) = s.variable_value(v.id).cloned() else { continue };
            match out.iter_mut().find(|(o, _)| *o == v.object) {
                Some((_, vals)) => {
                    if vals.last() != Some(&val) {
                        vals.push(val);
                    }
                }
                None => out.push((v.object, vec![val])),
            }
        }
    }
    out
}

/// The single value sequence of the only variable called `name`.
pub fn values_of(snaps: &[RuntimeSnapshot], name: &str) -> Vec<Value> {
    let h = history(snaps, name);
    assert_eq!(h.len(), 1, "expected exactly one variable `{name}`, found {}", h.len());
    h.into_iter().next().unwrap().1
}

/// Program source with the given name, compiled as `main.cpp`.
pub fn run_plain(src: &str) -> RuntimeSession {
    ExecutionManager::new(config()).run(request(src), &|_| {})
}
