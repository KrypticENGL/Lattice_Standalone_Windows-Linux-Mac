//! The user-facing timeline over a real observed run: far fewer steps than raw
//! events, tiling them exactly, with statement-level labels.
mod common;

use lattice_lib::observe::{aggregate, StepKind};

#[test]
fn pointer_program_aggregates_into_statements() {
    let Some((_session, obs)) = common::observe(
        r#"
struct Node { int value; Node* next; };
int main() {
    Node* a = new Node{10, nullptr};
    Node* b = new Node{20, nullptr};
    a->next = b;
    delete b;
    delete a;
    return 0;
}
"#,
    ) else {
        return;
    };
    let events = obs.timeline.events();
    let steps = aggregate(events);
    for s in &steps {
        eprintln!("[{:>3}..{:>3}] {:?} L{:?} {}", s.start, s.end, s.kind, s.line, s.label);
    }
    assert!(steps.len() < events.len(), "{} steps for {} events", steps.len(), events.len());
    assert_eq!(steps.first().unwrap().start, 0);
    for w in steps.windows(2) {
        assert_eq!(w[0].end, w[1].start);
    }
    assert_eq!(steps.last().unwrap().end, events.len() as u64);
    let labels: Vec<&str> = steps.iter().map(|s| s.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.starts_with("a = new Node")), "{labels:?}");
    assert!(labels.iter().any(|l| l.starts_with("b = new Node")), "{labels:?}");
    assert!(labels.iter().any(|l| l.contains("next = b")), "{labels:?}");
    assert!(steps.iter().any(|s| s.kind == StepKind::Return));
}
