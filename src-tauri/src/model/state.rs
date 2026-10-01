//! [`RuntimeState`]: the model of the program at one point in execution, built by
//! applying [`RuntimeEvent`]s in order.
//!
//! Ownership: the state owns everything by value, keyed by logical ids. There are
//! no pointers between entities (only ids), so cyclic program structures are
//! just cyclic *ids* and cannot cause ownership cycles or leaks here.
//!
//! Consistency: `apply` validates an event completely before mutating anything,
//! so a rejected event leaves the state untouched.
//!
//! The model never dereferences user memory and never maps addresses to
//! objects; the observer resolves addresses to ids before reporting.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use super::entities::*;
use super::event::{EventKind, ObjectDecl, RuntimeEvent, VariableDecl};
use super::ids::*;
use super::source::SourceLocation;
use super::types::TypeTable;
use super::value::{Edge, Place, Target, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// Event sequence numbers must strictly increase.
    OutOfOrder { last: EventSeq, got: EventSeq },
    TypeConflict(TypeId),
    UnknownType(TypeId),
    DuplicateObject(ObjectId),
    UnknownObject(ObjectId),
    DuplicateVariable(VariableId),
    UnknownVariable(VariableId),
    DuplicateFrame(FrameId),
    UnknownFrame(FrameId),
    DuplicateScope(ScopeId),
    UnknownScope(ScopeId),
    NotTopFrame(FrameId),
    NotInnermostScope(ScopeId),
    /// The path does not exist inside the object's current value.
    BadPlace(Place),
    /// Lifetime transition not allowed from the object's current state.
    BadLifetime { object: ObjectId, from: LifeState },
    /// A write to an object whose lifetime has ended (use-after-free in the
    /// observed program). Reported, not applied.
    WriteToDestroyed(ObjectId),
    InvalidVariable(VariableId, &'static str),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ApplyError {}

/// Is the thing a pointer/reference designates still there?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetStatus {
    Null,
    /// Root object is not destroyed and the path resolves.
    Live,
    /// Root object's lifetime has ended.
    Dangling,
    /// Root object is alive but the path does not resolve (past-the-end,
    /// replaced subobject).
    OutOfBounds,
    /// Not a tracked object.
    Unresolved,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeState {
    types: TypeTable,
    objects: BTreeMap<ObjectId, Object>,
    variables: BTreeMap<VariableId, Variable>,
    frames: BTreeMap<FrameId, Frame>,
    scopes: BTreeMap<ScopeId, Scope>,
    /// Per-thread call stacks, outermost frame first.
    stacks: BTreeMap<ThreadId, Vec<FrameId>>,
    globals: Vec<VariableId>,
    /// Reverse link index: target object -> links pointing into it. Makes
    /// "who points at this?" O(k) in the answer size instead of O(all objects).
    incoming: HashMap<ObjectId, BTreeSet<Edge>>,
    last_seq: Option<EventSeq>,
    last_thread: Option<ThreadId>,
    last_location: Option<SourceLocation>,
    truncated_at: Option<EventSeq>,
}

fn edges_of(base: &Place, value: &Value) -> Vec<Edge> {
    let mut out = Vec::new();
    value.for_each_link(|rel, kind, target| {
        if let Target::Place { place } = target {
            let mut path = base.path.clone();
            path.extend_from_slice(rel);
            out.push(Edge {
                source: Place { object: base.object, path },
                kind,
                target: place.clone(),
            });
        }
    });
    out
}

impl RuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- reading -------------------------------------------------------

    pub fn types(&self) -> &TypeTable {
        &self.types
    }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id)
    }

    /// All objects ever allocated (including destroyed ones), by id.
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.values()
    }

    /// Objects whose lifetime has not ended.
    pub fn live_objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.values().filter(|o| !o.is_destroyed())
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn variable(&self, id: VariableId) -> Option<&Variable> {
        self.variables.get(&id)
    }

    /// Currently existing variables (destroyed ones are gone), by id.
    pub fn variables(&self) -> impl Iterator<Item = &Variable> {
        self.variables.values()
    }

    pub fn globals(&self) -> &[VariableId] {
        &self.globals
    }

    pub fn frame(&self, id: FrameId) -> Option<&Frame> {
        self.frames.get(&id)
    }

    pub fn scope(&self, id: ScopeId) -> Option<&Scope> {
        self.scopes.get(&id)
    }

    /// Call stack of `thread`, outermost (e.g. `main`) first, top of stack last.
    pub fn stack(&self, thread: ThreadId) -> &[FrameId] {
        self.stacks.get(&thread).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Threads that currently have at least one frame.
    pub fn threads(&self) -> impl Iterator<Item = ThreadId> + '_ {
        self.stacks.iter().filter(|(_, s)| !s.is_empty()).map(|(t, _)| *t)
    }

    pub fn top_frame(&self, thread: ThreadId) -> Option<&Frame> {
        self.frames.get(self.stack(thread).last()?)
    }

    /// Variables of a frame (parameters and locals) in creation order.
    pub fn frame_variables(&self, frame: FrameId) -> impl Iterator<Item = &Variable> {
        self.frames
            .get(&frame)
            .into_iter()
            .flat_map(|f| f.variables.iter())
            .filter_map(|v| self.variables.get(v))
    }

    /// The storage object a variable is bound to.
    pub fn variable_object(&self, id: VariableId) -> Option<&Object> {
        self.objects.get(&self.variables.get(&id)?.object)
    }

    /// Current value stored in a variable's storage.
    pub fn variable_value(&self, id: VariableId) -> Option<&Value> {
        self.variable_object(id).map(|o| &o.value)
    }

    pub fn value_at(&self, place: &Place) -> Option<&Value> {
        self.objects.get(&place.object)?.value.at(&place.path)
    }

    pub fn type_of(&self, object: ObjectId) -> Option<&super::types::TypeDef> {
        self.types.get(self.objects.get(&object)?.ty)
    }

    /// Whether a pointer/reference target designates something still alive.
    pub fn target_status(&self, target: &Target) -> TargetStatus {
        match target {
            Target::Null => TargetStatus::Null,
            Target::Unresolved => TargetStatus::Unresolved,
            Target::Place { place } => match self.objects.get(&place.object) {
                None => TargetStatus::Unresolved,
                Some(o) if o.is_destroyed() => TargetStatus::Dangling,
                Some(o) if o.value.at(&place.path).is_some() => TargetStatus::Live,
                Some(_) => TargetStatus::OutOfBounds,
            },
        }
    }

    /// Links stored anywhere inside `object` (pointer/reference fields, array
    /// elements, nested members), in value order. Cost is O(size of the object).
    /// For a destroyed object this is its last known content.
    pub fn outgoing(&self, object: ObjectId) -> Vec<Edge> {
        match self.objects.get(&object) {
            Some(o) => edges_of(&Place::root(object), &o.value),
            None => Vec::new(),
        }
    }

    /// Links anywhere in the program that designate something inside `object`,
    /// ordered. Uses the reverse index; cost is O(result). Multiple results whose
    /// `target` is the same place are aliases of each other.
    pub fn incoming(&self, object: ObjectId) -> Vec<Edge> {
        self.incoming.get(&object).map(|s| s.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn last_seq(&self) -> Option<EventSeq> {
        self.last_seq
    }

    /// Set once the observer reported that it stopped (see
    /// [`EventKind::ObservationTruncated`]): the run continued past this point
    /// unobserved, so this state is where the *record* ends, not the program.
    pub fn truncated_at(&self) -> Option<EventSeq> {
        self.truncated_at
    }

    pub fn last_thread(&self) -> Option<ThreadId> {
        self.last_thread
    }

    /// Source location of the most recently applied event.
    pub fn last_location(&self) -> Option<&SourceLocation> {
        self.last_location.as_ref()
    }

    // ---- applying events ------------------------------------------------

    /// Apply one event. On `Err` the state is unchanged.
    pub fn apply(&mut self, event: &RuntimeEvent) -> Result<(), ApplyError> {
        if let Some(last) = self.last_seq {
            if event.seq <= last {
                return Err(ApplyError::OutOfOrder { last, got: event.seq });
            }
        }
        self.apply_kind(event.seq, event.thread, event.location.as_ref(), &event.kind)?;

        self.last_seq = Some(event.seq);
        self.last_thread = Some(event.thread);
        self.last_location = event.location.clone();
        if !matches!(event.kind, EventKind::FunctionExited { .. }) {
            if let Some(loc) = &event.location {
                if let Some(top) = self.stacks.get(&event.thread).and_then(|s| s.last()) {
                    if let Some(frame) = self.frames.get_mut(top) {
                        frame.location = Some(loc.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_kind(
        &mut self,
        seq: EventSeq,
        thread: ThreadId,
        location: Option<&SourceLocation>,
        kind: &EventKind,
    ) -> Result<(), ApplyError> {
        match kind {
            EventKind::TypeDeclared { def } => {
                self.types.declare(def).map_err(|()| ApplyError::TypeConflict(def.id))
            }

            EventKind::FunctionEntered { frame, function, call_site } => {
                if self.frames.contains_key(frame) {
                    return Err(ApplyError::DuplicateFrame(*frame));
                }
                let stack = self.stacks.entry(thread).or_default();
                let caller = stack.last().copied();
                stack.push(*frame);
                self.frames.insert(
                    *frame,
                    Frame {
                        id: *frame,
                        thread,
                        function: function.clone(),
                        caller,
                        call_site: call_site.clone(),
                        location: location.cloned(),
                        variables: Vec::new(),
                        open_scopes: Vec::new(),
                    },
                );
                Ok(())
            }

            EventKind::FunctionExited { frame } => {
                let f = self.frames.get(frame).ok_or(ApplyError::UnknownFrame(*frame))?;
                if self.stack(f.thread).last() != Some(frame) {
                    return Err(ApplyError::NotTopFrame(*frame));
                }
                let scopes: Vec<ScopeId> = f.open_scopes.iter().rev().copied().collect();
                for scope in scopes {
                    let vars = self.scopes.remove(&scope).map(|s| s.variables).unwrap_or_default();
                    for v in vars.into_iter().rev() {
                        self.end_variable(v, EndReason::FrameExit, seq);
                    }
                }
                let f = self.frames.remove(frame).expect("checked above");
                for v in f.variables.into_iter().rev() {
                    self.end_variable(v, EndReason::FrameExit, seq);
                }
                if let Some(stack) = self.stacks.get_mut(&f.thread) {
                    stack.pop();
                    if stack.is_empty() {
                        self.stacks.remove(&f.thread);
                    }
                }
                Ok(())
            }

            EventKind::ScopeEntered { scope, frame } => {
                if self.scopes.contains_key(scope) {
                    return Err(ApplyError::DuplicateScope(*scope));
                }
                let f = self.frames.get_mut(frame).ok_or(ApplyError::UnknownFrame(*frame))?;
                let parent = f.open_scopes.last().copied();
                f.open_scopes.push(*scope);
                self.scopes
                    .insert(*scope, Scope { id: *scope, frame: *frame, parent, variables: Vec::new() });
                Ok(())
            }

            EventKind::ScopeExited { scope } => {
                let s = self.scopes.get(scope).ok_or(ApplyError::UnknownScope(*scope))?;
                let frame = self.frames.get_mut(&s.frame).ok_or(ApplyError::UnknownFrame(s.frame))?;
                if frame.open_scopes.last() != Some(scope) {
                    return Err(ApplyError::NotInnermostScope(*scope));
                }
                frame.open_scopes.pop();
                let s = self.scopes.remove(scope).expect("checked above");
                for v in s.variables.into_iter().rev() {
                    self.end_variable(v, EndReason::ScopeExit, seq);
                }
                Ok(())
            }

            EventKind::ObjectAllocated { object } => self.allocate(seq, location, object),

            EventKind::ObjectConstructed { object } => {
                let o = self.objects.get_mut(object).ok_or(ApplyError::UnknownObject(*object))?;
                if o.lifetime.state != LifeState::Allocated {
                    return Err(ApplyError::BadLifetime { object: *object, from: o.lifetime.state });
                }
                o.lifetime.state = LifeState::Alive;
                Ok(())
            }

            EventKind::ObjectDestroyed { object, reason } => {
                let o = self.objects.get_mut(object).ok_or(ApplyError::UnknownObject(*object))?;
                if o.is_destroyed() {
                    return Err(ApplyError::BadLifetime { object: *object, from: o.lifetime.state });
                }
                o.lifetime.state = LifeState::Destroyed;
                o.lifetime.ended_at = Some(seq);
                o.lifetime.end_reason = Some(*reason);
                Ok(())
            }

            EventKind::VariableCreated { variable } => self.create_variable(variable),

            EventKind::VariableDestroyed { variable } => {
                if !self.variables.contains_key(variable) {
                    return Err(ApplyError::UnknownVariable(*variable));
                }
                self.end_variable(*variable, EndReason::ScopeExit, seq);
                Ok(())
            }

            EventKind::ValueChanged { place, value } => self.write(place, value),

            EventKind::ObservationTruncated { .. } => {
                self.truncated_at.get_or_insert(seq);
                Ok(())
            }
        }
    }

    /// Every pointer/reference inside `value` must designate a known object
    /// (`also` is an id about to be created). Untracked memory must be reported
    /// as `Target::Unresolved`, which keeps observer bugs loud.
    fn check_targets(&self, value: &Value, also: Option<ObjectId>) -> Result<(), ApplyError> {
        let mut bad = None;
        value.for_each_link(|_, _, target| {
            if let Target::Place { place } = target {
                if bad.is_none() && !self.objects.contains_key(&place.object) && Some(place.object) != also {
                    bad = Some(place.object);
                }
            }
        });
        bad.map_or(Ok(()), |o| Err(ApplyError::UnknownObject(o)))
    }

    fn allocate(
        &mut self,
        seq: EventSeq,
        location: Option<&SourceLocation>,
        d: &ObjectDecl,
    ) -> Result<(), ApplyError> {
        if self.objects.contains_key(&d.id) {
            return Err(ApplyError::DuplicateObject(d.id));
        }
        if !self.types.contains(d.ty) {
            return Err(ApplyError::UnknownType(d.ty));
        }
        if d.state == LifeState::Destroyed {
            return Err(ApplyError::BadLifetime { object: d.id, from: d.state });
        }
        self.check_targets(&d.value, Some(d.id))?;

        for e in edges_of(&Place::root(d.id), &d.value) {
            self.incoming.entry(e.target.object).or_default().insert(e);
        }
        self.objects.insert(
            d.id,
            Object {
                id: d.id,
                ty: d.ty,
                storage: d.storage,
                address: d.address,
                size: d.size,
                value: d.value.clone(),
                lifetime: Lifetime {
                    state: d.state,
                    allocated_at: seq,
                    ended_at: None,
                    end_reason: None,
                },
                origin: location.cloned(),
            },
        );
        Ok(())
    }

    fn create_variable(&mut self, d: &VariableDecl) -> Result<(), ApplyError> {
        if self.variables.contains_key(&d.id) {
            return Err(ApplyError::DuplicateVariable(d.id));
        }
        if !self.objects.contains_key(&d.object) {
            return Err(ApplyError::UnknownObject(d.object));
        }
        let scoped = matches!(d.kind, VariableKind::Local | VariableKind::Parameter);
        match (scoped, d.frame) {
            (true, None) => return Err(ApplyError::InvalidVariable(d.id, "locals and parameters need a frame")),
            (false, Some(_)) => return Err(ApplyError::InvalidVariable(d.id, "globals and statics have no frame")),
            _ => {}
        }
        if let Some(frame) = d.frame {
            if !self.frames.contains_key(&frame) {
                return Err(ApplyError::UnknownFrame(frame));
            }
        }
        if let Some(scope) = d.scope {
            let s = self.scopes.get(&scope).ok_or(ApplyError::UnknownScope(scope))?;
            if Some(s.frame) != d.frame {
                return Err(ApplyError::InvalidVariable(d.id, "scope belongs to another frame"));
            }
        }

        self.variables.insert(
            d.id,
            Variable {
                id: d.id,
                name: d.name.clone(),
                kind: d.kind,
                frame: d.frame,
                scope: d.scope,
                object: d.object,
            },
        );
        // `Frame::variables` lists every variable of the frame (scoped or not);
        // `Scope::variables` additionally lists those declared in that scope.
        if let Some(frame) = d.frame {
            self.frames.get_mut(&frame).expect("checked").variables.push(d.id);
        } else {
            self.globals.push(d.id);
        }
        if let Some(scope) = d.scope {
            self.scopes.get_mut(&scope).expect("checked").variables.push(d.id);
        }
        Ok(())
    }

    /// Remove a variable and end its automatic object if still alive.
    /// Infallible: callers have validated the variable exists.
    fn end_variable(&mut self, id: VariableId, reason: EndReason, seq: EventSeq) {
        let Some(v) = self.variables.remove(&id) else { return };
        if let Some(frame) = v.frame.and_then(|f| self.frames.get_mut(&f)) {
            frame.variables.retain(|x| *x != id);
        }
        if let Some(scope) = v.scope.and_then(|s| self.scopes.get_mut(&s)) {
            scope.variables.retain(|x| *x != id);
        }
        self.globals.retain(|x| *x != id);
        if let Some(o) = self.objects.get_mut(&v.object) {
            if o.storage == StorageClass::Automatic && !o.is_destroyed() {
                o.lifetime.state = LifeState::Destroyed;
                o.lifetime.ended_at = Some(seq);
                o.lifetime.end_reason = Some(reason);
            }
        }
    }

    fn write(&mut self, place: &Place, new: &Value) -> Result<(), ApplyError> {
        let obj = self.objects.get(&place.object).ok_or(ApplyError::UnknownObject(place.object))?;
        if obj.is_destroyed() {
            return Err(ApplyError::WriteToDestroyed(place.object));
        }
        let old = obj.value.at(&place.path).ok_or_else(|| ApplyError::BadPlace(place.clone()))?;
        self.check_targets(new, None)?;

        let removed = edges_of(place, old);
        let added = edges_of(place, new);
        for e in removed {
            if let Some(set) = self.incoming.get_mut(&e.target.object) {
                set.remove(&e);
                if set.is_empty() {
                    self.incoming.remove(&e.target.object);
                }
            }
        }
        for e in added {
            self.incoming.entry(e.target.object).or_default().insert(e);
        }
        let slot = self
            .objects
            .get_mut(&place.object)
            .and_then(|o| o.value.at_mut(&place.path))
            .expect("path validated above");
        *slot = new.clone();
        Ok(())
    }

    // ---- diagnostics ----------------------------------------------------

    /// Check internal invariants (reverse index matches the values, variables
    /// bind to existing objects, stacks and frames agree). Linear in state size;
    /// meant for tests and debugging, not the hot path.
    pub fn verify_invariants(&self) -> Result<(), String> {
        let mut expected: HashMap<ObjectId, BTreeSet<Edge>> = HashMap::new();
        for o in self.objects.values() {
            for e in edges_of(&Place::root(o.id), &o.value) {
                if !self.objects.contains_key(&e.target.object) {
                    return Err(format!("{} links to unknown {}", o.id, e.target.object));
                }
                expected.entry(e.target.object).or_default().insert(e);
            }
        }
        if expected != self.incoming {
            return Err("incoming-edge index out of sync with object values".into());
        }
        for v in self.variables.values() {
            if !self.objects.contains_key(&v.object) {
                return Err(format!("{} binds unknown {}", v.id, v.object));
            }
        }
        for (thread, stack) in &self.stacks {
            for (i, fid) in stack.iter().enumerate() {
                let f = self.frames.get(fid).ok_or_else(|| format!("stack has unknown {fid}"))?;
                let want = if i == 0 { None } else { Some(stack[i - 1]) };
                if f.caller != want || f.thread != *thread {
                    return Err(format!("{fid} caller/thread mismatch"));
                }
            }
        }
        if self.frames.len() != self.stacks.values().map(Vec::len).sum::<usize>() {
            return Err("frame not on any stack".into());
        }
        Ok(())
    }
}
