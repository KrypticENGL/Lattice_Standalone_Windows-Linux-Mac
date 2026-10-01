//! A Lattice solution on disk: a folder, opened through its `lattice.sln`.
//!
//! ```text
//! MyProject/
//!   lattice.sln        manifest (JSON): name, file order, open file, settings. Opens the solution.
//!   src/               the source files, one real file each
//!   target/            executables built by runs
//!   .lattice/urr.json  the recorded URR state of the last observed run (optional)
//! ```
//!
//! The URR is stored as its events, the model's single source of truth, so every snapshot,
//! timeline step and view is rebuilt from it on open instead of being stored (and able to
//! disagree). A solution is a folder rather than one file because more will live in it.
//!
//! Plain data and file I/O only; nothing here knows about Tauri.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{RuntimeEvent, Timeline};
use crate::runtime::SourceFile;

/// Marker in every manifest, so a stray `.sln` is not mistaken for a Lattice one.
pub const FORMAT: &str = "lattice-solution";
/// Bumped when the layout changes incompatibly. Newer solutions are refused, not guessed at.
pub const VERSION: u32 = 2;
/// The file that opens a solution.
pub const MANIFEST: &str = "lattice.sln";
pub const SRC_DIR: &str = "src";
pub const TARGET_DIR: &str = "target";
const URR_PATH: &str = ".lattice/urr.json";
/// Version of the event schema (`model::EventKind`) stored in [`UrrState`].
pub const URR_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionSource {
    pub name: String,
    pub contents: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionSettings {
    /// Whether runs record their runtime state.
    #[serde(default)]
    pub observe: bool,
}

/// `lattice.sln`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
    /// Source files under `src/`, in tab order.
    pub files: Vec<String>,
    /// The file that was open in the editor.
    #[serde(default)]
    pub active_file: Option<String>,
    #[serde(default)]
    pub settings: SolutionSettings,
    /// Whether `.lattice/urr.json` holds the runtime state of the last observed run.
    #[serde(default)]
    pub has_urr: bool,
}

/// The recorded run: the URR event stream plus what is needed to tell if it is stale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrrState {
    pub schema: u32,
    pub events: Vec<RuntimeEvent>,
    /// Hash of the sources the run was made from; differs from the files' hash once they were edited.
    pub source_hash: String,
}

impl UrrState {
    /// The URR as a timeline, rebuilt by replaying its events (which validates them).
    pub fn timeline(&self) -> Result<Timeline, String> {
        if self.schema > URR_SCHEMA {
            return Err(format!("The saved runtime state uses a newer event schema ({}).", self.schema));
        }
        let mut t = Timeline::new();
        for e in &self.events {
            t.push(e.clone()).map_err(|e| format!("The saved runtime state is inconsistent: {e}"))?;
        }
        Ok(t)
    }
}

/// A solution in memory: its manifest and the contents of its source files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solution {
    pub manifest: Manifest,
    pub files: Vec<SolutionSource>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// FNV-1a over names and contents: cheap, stable across runs, and only used to notice edits.
pub fn hash_sources(files: &[SolutionSource]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    };
    let mut sorted: Vec<&SolutionSource> = files.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for f in sorted {
        eat(f.name.as_bytes());
        eat(f.contents.as_bytes());
    }
    format!("{h:016x}")
}

pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST)
}

pub fn src_dir(root: &Path) -> PathBuf {
    root.join(SRC_DIR)
}

pub fn target_dir(root: &Path) -> PathBuf {
    root.join(TARGET_DIR)
}

/// The solution folder for a path the user picked: the folder itself, or the `lattice.sln` in it.
pub fn root_of(picked: &Path) -> PathBuf {
    if picked.is_file() || picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case(MANIFEST)) {
        picked.parent().map(Path::to_path_buf).unwrap_or_else(|| picked.to_path_buf())
    } else {
        picked.to_path_buf()
    }
}

/// Check the invariants a solution must hold to be opened or written.
pub fn validate(name: &str, files: &[SolutionSource]) -> Result<(), String> {
    if name.trim().is_empty() || name.contains(['/', '\\', ':', '\0']) {
        return Err(format!("Invalid solution name: {name:?}."));
    }
    if files.is_empty() {
        return Err("A solution needs at least one source file.".into());
    }
    let mut seen = std::collections::HashSet::new();
    for f in files {
        SourceFile { name: f.name.clone(), contents: String::new() }.validate()?;
        if !seen.insert(f.name.to_ascii_lowercase()) {
            return Err(format!("Two files are named {:?}.", f.name));
        }
    }
    Ok(())
}

impl Solution {
    pub fn new(name: String, files: Vec<SolutionSource>, active_file: Option<String>, settings: SolutionSettings) -> Self {
        let now = now_ms();
        Solution {
            manifest: Manifest {
                format: FORMAT.into(),
                version: VERSION,
                name,
                created_at_ms: now,
                updated_at_ms: now,
                files: files.iter().map(|f| f.name.clone()).collect(),
                active_file,
                settings,
                has_urr: false,
            },
            files,
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not write {}: {e}", path.display())
    })
}

/// Whether `root` already holds a solution.
pub fn exists(root: &Path) -> bool {
    manifest_path(root).is_file()
}

/// Write `solution` (and `urr`, if any) into the folder `root`, creating `src/` and `target/`.
/// Source files that an earlier save listed but the solution no longer has are removed; no
/// other file in the folder is touched. The manifest is written last, so a failure part-way
/// leaves the previous solution openable.
pub fn write(root: &Path, solution: &Solution, urr: Option<&UrrState>) -> Result<(), String> {
    validate(&solution.manifest.name, &solution.files)?;
    let previous = read_manifest(root).ok();
    let src = src_dir(root);
    fs::create_dir_all(&src).map_err(|e| format!("Could not create {}: {e}", src.display()))?;
    fs::create_dir_all(target_dir(root)).map_err(|e| format!("Could not create the target folder: {e}"))?;

    for f in &solution.files {
        write_atomic(&src.join(&f.name), f.contents.as_bytes())?;
    }
    if let Some(prev) = &previous {
        for gone in prev.files.iter().filter(|n| !solution.files.iter().any(|f| &f.name == *n)) {
            let _ = fs::remove_file(src.join(gone));
        }
    }

    let urr_file = root.join(URR_PATH);
    match urr {
        Some(u) => {
            let bytes = serde_json::to_vec(u).map_err(|e| format!("Could not encode the runtime state: {e}"))?;
            write_atomic(&urr_file, &bytes)?;
        }
        None => {
            let _ = fs::remove_file(&urr_file);
        }
    }

    let mut manifest = solution.manifest.clone();
    manifest.version = VERSION;
    manifest.updated_at_ms = now_ms();
    manifest.has_urr = urr.is_some();
    if let Some(prev) = previous.filter(|p| p.created_at_ms > 0) {
        manifest.created_at_ms = prev.created_at_ms;
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| format!("Could not encode the manifest: {e}"))?;
    write_atomic(&manifest_path(root), &bytes)
}

fn read_manifest(root: &Path) -> Result<Manifest, String> {
    let path = manifest_path(root);
    let bytes = fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("{} is not a valid Lattice solution: {e}", path.display()))?;
    if manifest.format != FORMAT {
        return Err(format!("{} is not a Lattice solution.", path.display()));
    }
    if manifest.version > VERSION {
        return Err(format!(
            "This solution was saved by a newer Lattice (version {}, this app reads up to {VERSION}).",
            manifest.version
        ));
    }
    Ok(manifest)
}

/// Read the solution in `root`: the manifest's files in its order, then any other valid
/// source file found in `src/` (added by hand), and the saved URR state if there is one.
pub fn read(root: &Path) -> Result<(Solution, Option<UrrState>), String> {
    let mut manifest = read_manifest(root)?;
    let src = src_dir(root);
    let mut names = manifest.files.clone();
    if let Ok(rd) = fs::read_dir(&src) {
        let mut extra: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| SourceFile { name: n.clone(), contents: String::new() }.validate().is_ok())
            .filter(|n| !names.iter().any(|x| x.eq_ignore_ascii_case(n)))
            .collect();
        extra.sort();
        names.extend(extra);
    }
    let mut files = Vec::with_capacity(names.len());
    for n in &names {
        let p = src.join(n);
        let contents = fs::read_to_string(&p).map_err(|e| format!("Could not read {}: {e}", p.display()))?;
        files.push(SolutionSource { name: n.clone(), contents });
    }
    validate(&manifest.name, &files)?;
    manifest.files = names;

    // A damaged URR file loses only the recording, never the sources.
    let urr = if manifest.has_urr {
        fs::read(root.join(URR_PATH)).ok().and_then(|b| serde_json::from_slice::<UrrState>(&b).ok())
    } else {
        None
    };
    Ok((Solution { manifest, files }, urr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Solution {
        Solution::new(
            "demo".into(),
            vec![
                SolutionSource { name: "main.cpp".into(), contents: "#include \"list.h\"\nint main(){}\n".into() },
                SolutionSource { name: "list.h".into(), contents: "struct Node{int v;};\n".into() },
            ],
            Some("list.h".into()),
            SolutionSettings { observe: true },
        )
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lattice-sln-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writes_the_folder_layout_and_round_trips() {
        let root = temp("layout");
        let s = sample();
        write(&root, &s, None).unwrap();
        assert!(root.join("lattice.sln").is_file());
        assert!(root.join("src/main.cpp").is_file());
        assert!(root.join("src/list.h").is_file());
        assert!(root.join("target").is_dir());
        let (back, urr) = read(&root).unwrap();
        assert_eq!(back.files, s.files);
        assert_eq!(back.manifest.active_file.as_deref(), Some("list.h"));
        assert!(back.manifest.settings.observe);
        assert!(urr.is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removed_files_are_deleted_but_strangers_are_left_alone() {
        let root = temp("remove");
        write(&root, &sample(), None).unwrap();
        fs::write(root.join("src/notes.txt"), "keep me").unwrap();
        let mut s = sample();
        s.files.pop();
        s.manifest.files.pop();
        write(&root, &s, None).unwrap();
        assert!(!root.join("src/list.h").exists());
        assert!(root.join("src/notes.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn picks_up_sources_added_by_hand() {
        let root = temp("extra");
        write(&root, &sample(), None).unwrap();
        fs::write(root.join("src/util.cpp"), "int x;").unwrap();
        let (s, _) = read(&root).unwrap();
        assert_eq!(s.files.last().unwrap().name, "util.cpp");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_foreign_newer_and_unsafe_solutions() {
        let root = temp("bad");
        write(&root, &sample(), None).unwrap();
        let path = manifest_path(&root);
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("lattice-solution", "other")).unwrap();
        assert!(read(&root).is_err());
        fs::write(&path, text.replace(&format!("\"version\": {VERSION}"), "\"version\": 99")).unwrap();
        assert!(read(&root).unwrap_err().contains("newer"));
        fs::remove_dir_all(&root).unwrap();

        let mut s = sample();
        s.files[0].name = "../evil.cpp".into();
        assert!(validate("x", &s.files).is_err());
        let mut s = sample();
        s.files[1].name = "MAIN.cpp".into();
        assert!(validate("x", &s.files).unwrap_err().contains("Two files"));
    }

    #[test]
    fn hash_ignores_order_but_notices_edits() {
        let s = sample();
        let mut rev = s.files.clone();
        rev.reverse();
        assert_eq!(hash_sources(&s.files), hash_sources(&rev));
        rev[0].contents.push(' ');
        assert_ne!(hash_sources(&s.files), hash_sources(&rev));
    }

    #[test]
    fn root_of_accepts_the_folder_or_its_manifest() {
        let root = temp("root");
        write(&root, &sample(), None).unwrap();
        assert_eq!(root_of(&root), root);
        assert_eq!(root_of(&manifest_path(&root)), root);
        fs::remove_dir_all(&root).unwrap();
    }
}
