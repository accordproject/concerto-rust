//! Who owns each op or blocking member: the task that will add its dispatch
//! entry or Rust counterpart (PORTING.md 6.2), so that every `unsupported`
//! and every failing fixture names the task it waits on.
//!
//! An owner is looked up, in order:
//!
//! 1. in the seam ledger (`migration/ledger/SEAM_LEDGER.tsv` in the
//!    `concerto` checkout, the `planned_task` column), by `<Class>.<member>`
//!    (`constructor` spelled `new`, as ops spell it). The ledger keys the
//!    model-manager members by the class that defines them, so a
//!    `ModelManager.<member>` or `AstModelManager.<member>` op is also looked
//!    up as `BaseModelManager.<member>`. A module-level function (no class in
//!    the ledger) is keyed by the holder `lib/ops.js` calls it on
//!    (`MetaModel`, `DcsConverter`, `DateTimeUtil`);
//! 2. in the ledger by class, when every planned member of the class shares
//!    one task;
//! 3. in PORTING.md 6.2's op families: "P2-08 for the `ModelManager` and
//!    `ModelFile` ops, P2-12 for the `DecoratorManager` ops, P3-01 for
//!    `Serializer` and the `Factory` checks" (the instance classes
//!    `Resource`, `Typed`, … go with `Serializer`/`Factory`). Since P3-01
//!    was split, `Serializer` and `Factory` are P3-01b's and the instance
//!    classes stay with P3-01 (P3-01a);
//! 4. otherwise the owner is `unowned`, spelled out so that it shows in the
//!    report instead of a blank.
//!
//! Code the ledger keeps in TS (a row classified `TS` with planned task
//! `-`) is labelled `stays-ts` rather than given an op-family owner or
//! `unowned`: a member with such a row, and a class all of whose rows are
//! such rows (`ModelLoader`, `Factory`, `Resource`, `Typed`, `DcsConverter`,
//! ...). No porting task will add its dispatch entry; the fixture replays
//! through the WASM adapter, where TS runs it.
//!
//! Plan-owner overrides ([`PLAN_OWNER_OVERRIDES`]) are checked before
//! everything else. They record owner decisions that the ledger doesn't
//! express yet.
//!
//! # Finding the ledger (PORTING.md OD-7)
//!
//! The ledger is read from the checkout the corpus lives in
//! (`<fixtures>/../../ledger/SEAM_LEDGER.tsv`), or from `$CONCERTO_ORACLE_LEDGER`
//! directly when that is set (the TSV file itself, not a directory).
//!
//! **A missing or unreadable ledger fails the run.** Without it, owner
//! attribution silently degrades to steps 3 and 4 above: every `stays-ts`
//! member is misreported, most often as `unowned` (accordproject/concerto-rust#132:
//! extracting the canonical corpus tarball into a checkout without the
//! integration branch's `migration/ledger/` sent `unowned` from 530 to
//! 1,336 and made `stays-ts` disappear, while pass/fail/regression verdicts
//! and `baseline.tsv` stayed correct). [`Ledger::load`] panics rather than
//! fall back, naming the path it tried. Set `CONCERTO_ORACLE_NO_LEDGER=1` to
//! opt out explicitly and run with the degraded fallback anyway (a warning
//! is still printed).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The owner of something no task in the ledger or PORTING.md names.
pub const UNOWNED: &str = "unowned";

/// The owner of code the ledger keeps in TS.
pub const STAYS_TS: &str = "stays-ts";

/// Plan-owner decisions that take precedence over the ledger. A key is
/// either an exact op (`<Class>.<member>`) or a whole class; an exact op
/// wins over its class.
///
/// - `Factory`: its ledger rows are TS with no task (D7), but the plan owner
///   assigned its fixtures to P3-01, which owns the Factory model checks
///   (#32: "Factory model checks go to Rust"). P3-01 was then split
///   (accordproject/concerto-rust#56, rescoped 2026-09-25): the Factory and
///   Serializer oracle wiring moved to P3-01b (#124).
/// - `Serializer.new`: the constructor's ledger row is TS with no task
///   ("argument checks only"), but the plan owner decided it goes to Rust
///   (PLAN.md 3: the Serializer's per-field checks go to Rust; P4-10 only
///   adds the fast path, so it consumes rather than owns them), under
///   P3-01b since the split.
/// - `Serializer.toJSON` and `fromJSON`: the ledger says `P3-01+P4-10`;
///   P3-01's share of them is P3-01b's since the split (#124: "retarget
///   the `Serializer.fromJSON`/`toJSON` member owners where the ledger says
///   `P3-01+P4-10`"). P4-10 (the view) consumes the port.
/// - The `Resource`/`Identifiable` members that mutate the instance
///   (`setPropertyValue`, `addArrayValue`, `setIdentifier`) or serialize it
///   (`Resource.toJSON`, `getSerializer().toJSON(this)`): P3-01b too, by the
///   same split. The rest of those classes stay with P3-01 (P3-01a).
const PLAN_OWNER_OVERRIDES: &[(&str, &str)] = &[
    ("Factory", "P3-01b"),
    ("Serializer.new", "P3-01b"),
    ("Serializer.toJSON", "P3-01b"),
    ("Serializer.fromJSON", "P3-01b"),
    ("Resource.setPropertyValue", "P3-01b"),
    ("Resource.addArrayValue", "P3-01b"),
    ("Resource.toJSON", "P3-01b"),
    ("Identifiable.setIdentifier", "P3-01b"),
];

#[derive(Default)]
pub struct Ledger {
    /// `<Class>.<member>` to its planned task, or [`STAYS_TS`].
    members: HashMap<String, String>,
    /// `<Class>` to its members' owners: the planned tasks, and whether
    /// any row stays in TS.
    classes: HashMap<String, ClassOwners>,
}

#[derive(Default)]
struct ClassOwners {
    tasks: std::collections::BTreeSet<String>,
    stays_ts: bool,
}

/// The holder `lib/ops.js` (`staticHolder`) calls a module's functions on.
fn function_holder(file: &str) -> Option<&'static str> {
    match file {
        "src/introspect/metamodel.ts" => Some("MetaModel"),
        "src/dcsconverter.ts" => Some("DcsConverter"),
        "src/datetimeutil.ts" => Some("DateTimeUtil"),
        _ => None,
    }
}

/// PORTING.md 6.2's op-family owners, for a class the ledger does not
/// settle (not in it, or its members split across tasks).
fn family_owner(class: &str) -> Option<&'static str> {
    match class {
        "ModelManager" | "BaseModelManager" | "AstModelManager" | "ModelFile" | "ModelLoader"
        | "Introspector" => Some("P2-08"),
        "DecoratorManager" => Some("P2-12"),
        // P3-01 was split (accordproject/concerto-rust#56, rescoped
        // 2026-09-25): the Serializer and Factory wiring is P3-01b (#124).
        "Serializer" | "Factory" => Some("P3-01b"),
        "Resource" | "ValidatedResource" | "Typed" | "Identifiable" | "Relationship" => {
            Some("P3-01")
        }
        _ => None,
    }
}

/// How to get a ledger, or opt out, for every "no ledger" failure
/// (accordproject/concerto-rust#132).
const LEDGER_HELP: &str = "Extract the canonical oracle corpus (it includes migration/ledger/) \
     into a `concerto` checkout of claude/tender-pascal-ocwf9q, or copy migration/ledger/ from \
     that branch next to the corpus; point CONCERTO_ORACLE_LEDGER at the SEAM_LEDGER.tsv file \
     directly if it lives somewhere else; or set CONCERTO_ORACLE_NO_LEDGER=1 to run with the \
     degraded PORTING.md-op-family/unowned fallback instead.";

impl Ledger {
    /// Loads the ledger for `fixtures_dir`'s corpus (see the module doc),
    /// reading `$CONCERTO_ORACLE_LEDGER` and `$CONCERTO_ORACLE_NO_LEDGER`
    /// from the environment. Panics when the ledger cannot be found or read,
    /// unless the opt-out is set. See [`Self::load_with`] for the pure
    /// logic, which the harness's own tests drive directly instead of
    /// mutating process-wide env vars.
    pub fn load(fixtures_dir: &Path) -> Self {
        let configured = std::env::var("CONCERTO_ORACLE_LEDGER")
            .ok()
            .map(PathBuf::from);
        let opt_out = std::env::var("CONCERTO_ORACLE_NO_LEDGER").is_ok_and(|v| v == "1");
        Self::load_with(fixtures_dir, configured.as_deref(), opt_out)
    }

    /// [`Self::load`]'s logic with the env vars passed in directly:
    /// `configured` is `$CONCERTO_ORACLE_LEDGER` (the SEAM_LEDGER.tsv file
    /// itself, not a directory), and `opt_out` is
    /// `$CONCERTO_ORACLE_NO_LEDGER=1`.
    pub(crate) fn load_with(fixtures_dir: &Path, configured: Option<&Path>, opt_out: bool) -> Self {
        let (path, source) = Self::locate(fixtures_dir, configured);
        let parsed = path
            .as_deref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| Self::parse(&text));
        parsed.unwrap_or_else(|| Self::missing(path.as_deref(), source, opt_out))
    }

    /// Where the ledger should be, and why: `configured` if given
    /// (`$CONCERTO_ORACLE_LEDGER`), otherwise
    /// `<fixtures_dir>/../../ledger/SEAM_LEDGER.tsv` (the checkout the
    /// corpus lives in). `None` only when `fixtures_dir` has no grandparent
    /// to derive that default path from.
    fn locate(fixtures_dir: &Path, configured: Option<&Path>) -> (Option<PathBuf>, &'static str) {
        if let Some(path) = configured {
            return (Some(path.to_path_buf()), "$CONCERTO_ORACLE_LEDGER");
        }
        let path = fixtures_dir
            .parent()
            .and_then(Path::parent)
            .map(|migration| migration.join("ledger").join("SEAM_LEDGER.tsv"));
        (path, "<fixtures>/../../ledger/SEAM_LEDGER.tsv")
    }

    /// No usable ledger was found at `path` (or no path could even be
    /// derived, when `path` is `None`). Panics naming what was tried, unless
    /// `opt_out` is set, in which case it prints a warning and returns the
    /// degraded fallback ledger ([`Self::default`]: op-family owners and
    /// `unowned` only, no `stays-ts`).
    fn missing(path: Option<&Path>, source: &str, opt_out: bool) -> Self {
        let tried = match path {
            Some(p) => format!("{} ({source})", p.display()),
            None => format!(
                "nowhere ({source}: the fixtures directory has no grandparent to derive it from)"
            ),
        };
        if opt_out {
            eprintln!(
                "oracle harness: WARNING: no seam ledger at {tried}. CONCERTO_ORACLE_NO_LEDGER=1 \
                 is set, so owner attribution falls back to PORTING.md's op families and \
                 `unowned` (`stays-ts` will not appear in the report)."
            );
            return Self::default();
        }
        panic!(
            "oracle harness: no seam ledger found at {tried}, so owner attribution (`stays-ts` \
             vs an op-family owner vs `unowned`) would be silently wrong. {LEDGER_HELP}"
        );
    }

    /// Parses a `SEAM_LEDGER.tsv`'s text. `None` when the header is missing
    /// one of the columns this harness reads.
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let header = lines.next()?;
        let columns: Vec<&str> = header.split('\t').collect();
        let col = |name: &str| columns.iter().position(|c| *c == name);
        let (
            Some(file_col),
            Some(class_col),
            Some(member_col),
            Some(classification_col),
            Some(task_col),
        ) = (
            col("file"),
            col("class"),
            col("member"),
            col("classification"),
            col("planned_task"),
        )
        else {
            return None;
        };
        let mut ledger = Self::default();
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            let (Some(file), Some(class), Some(member), Some(task)) = (
                fields.get(file_col),
                fields.get(class_col),
                fields.get(member_col),
                fields.get(task_col),
            ) else {
                continue;
            };
            // A module-level function has no class in the ledger; ops name
            // it by the holder `lib/ops.js` calls it on.
            let class = if class.is_empty() {
                function_holder(file).unwrap_or_default()
            } else {
                class
            };
            if class.is_empty() || task.is_empty() {
                continue;
            }
            let stays_ts = *task == "-";
            if stays_ts && fields.get(classification_col) != Some(&"TS") {
                continue;
            }
            let member = if *member == "constructor" {
                "new"
            } else {
                member
            };
            let owner = if stays_ts { STAYS_TS } else { task };
            ledger
                .members
                .insert(format!("{class}.{member}"), owner.to_string());
            let owners = ledger.classes.entry(class.to_string()).or_default();
            if stays_ts {
                owners.stays_ts = true;
            } else {
                owners.tasks.insert((*task).to_string());
            }
        }
        Some(ledger)
    }

    /// The owner of an op or member (`<Class>.<member>`): a task such as
    /// `P2-08+P4-08`, or [`UNOWNED`]. A name with no class part (a
    /// pseudo-member such as `env.random`) is unowned unless PORTING names
    /// its family.
    pub fn owner(&self, op: &str) -> String {
        let (class, member) = op.split_once('.').unwrap_or((op, ""));
        if let Some((_, owner)) = PLAN_OWNER_OVERRIDES
            .iter()
            .find(|(key, _)| *key == op)
            .or_else(|| PLAN_OWNER_OVERRIDES.iter().find(|(key, _)| *key == class))
        {
            return (*owner).to_string();
        }
        let mut names = vec![op.to_string()];
        if matches!(class, "ModelManager" | "AstModelManager") {
            names.push(format!("BaseModelManager.{member}"));
        }
        names
            .iter()
            .find_map(|name| self.members.get(name).cloned())
            .or_else(|| {
                let owners = self.classes.get(class)?;
                match owners.tasks.len() {
                    // Every row of the class stays in TS.
                    0 if owners.stays_ts => Some(STAYS_TS.to_string()),
                    1 => owners.tasks.first().cloned(),
                    _ => None,
                }
            })
            .or_else(|| family_owner(class).map(str::to_string))
            .unwrap_or_else(|| UNOWNED.to_string())
    }
}
