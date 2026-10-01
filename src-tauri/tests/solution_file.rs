//! A solution folder carries the URR of a real observed run and replays it to the same state.
mod common;

use lattice_lib::solution::*;

#[test]
fn urr_state_survives_a_save_and_open() {
    let Some((_s, obs)) = common::observe("struct N{int v;N* n;};\nint main(){ N* a = new N{1,nullptr}; delete a; return 0; }\n") else {
        return;
    };
    let sln = Solution::new(
        "t".into(),
        vec![SolutionSource { name: "main.cpp".into(), contents: "x".into() }],
        None,
        SolutionSettings::default(),
    );
    let urr = UrrState { schema: URR_SCHEMA, events: obs.timeline.events().to_vec(), source_hash: hash_sources(&sln.files) };
    let root = std::env::temp_dir().join(format!("lattice-urr-{}", std::process::id()));
    write(&root, &sln, Some(&urr)).unwrap();
    assert!(root.join("lattice.sln").is_file() && root.join("src/main.cpp").is_file());

    let (back, urr_back) = read(&root).unwrap();
    let t = urr_back.expect("urr present").timeline().unwrap();
    assert_eq!(t.events(), obs.timeline.events());
    assert_eq!(t.latest().objects().count(), obs.timeline.latest().objects().count());
    assert_eq!(back.files, sln.files);
    let _ = std::fs::remove_dir_all(root);
}
