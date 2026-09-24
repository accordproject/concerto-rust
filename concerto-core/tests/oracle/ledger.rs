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
//!    `Resource`, `Typed`, … go with `Serializer`/`Factory`);
//! 4. otherwise the owner is `unowned`, spelled out so that it shows in the
//!    report instead of a blank.
//!
//! The ledger is read from the checkout the corpus lives in
//! (`<fixtures>/../../ledger/`); without it only steps 3 and 4 apply.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// The owner of something no task in the ledger or PORTING.md names.
pub const UNOWNED: &str = "unowned";

#[derive(Default)]
pub struct Ledger {
    members: HashMap<String, String>,
    /// `<Class>` to the task all its planned members share, `None` when
    /// they name several.
    classes: HashMap<String, Option<String>>,
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

/// PORTING.md 6.2's op-family owners.
fn family_owner(class: &str) -> Option<&'static str> {
    match class {
        "ModelManager" | "BaseModelManager" | "AstModelManager" | "ModelFile" | "ModelLoader"
        | "Introspector" => Some("P2-08"),
        "DecoratorManager" => Some("P2-12"),
        "Serializer" | "Factory" | "Resource" | "ValidatedResource" | "Typed" | "Identifiable"
        | "Relationship" => Some("P3-01"),
        _ => None,
    }
}

impl Ledger {
    pub fn load(fixtures_dir: &Path) -> Self {
        let Some(path) = fixtures_dir
            .parent()
            .and_then(Path::parent)
            .map(|migration| migration.join("ledger").join("SEAM_LEDGER.tsv"))
        else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return Self::default();
        };
        let columns: Vec<&str> = header.split('\t').collect();
        let col = |name: &str| columns.iter().position(|c| *c == name);
        let (Some(file_col), Some(class_col), Some(member_col), Some(task_col)) = (
            col("file"),
            col("class"),
            col("member"),
            col("planned_task"),
        ) else {
            return Self::default();
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
            if class.is_empty() || task.is_empty() || *task == "-" {
                continue;
            }
            let member = if *member == "constructor" {
                "new"
            } else {
                member
            };
            ledger
                .members
                .insert(format!("{class}.{member}"), (*task).to_string());
            ledger
                .classes
                .entry((*class).to_string())
                .and_modify(|shared| {
                    if shared.as_deref() != Some(*task) {
                        *shared = None;
                    }
                })
                .or_insert_with(|| Some((*task).to_string()));
        }
        ledger
    }

    /// The owner of an op or member (`<Class>.<member>`): a task such as
    /// `P2-08+P4-08`, or [`UNOWNED`]. A name with no class part (a
    /// pseudo-member such as `env.random`) is unowned unless PORTING names
    /// its family.
    pub fn owner(&self, op: &str) -> String {
        let (class, member) = op.split_once('.').unwrap_or((op, ""));
        let mut names = vec![op.to_string()];
        if matches!(class, "ModelManager" | "AstModelManager") {
            names.push(format!("BaseModelManager.{member}"));
        }
        names
            .iter()
            .find_map(|name| self.members.get(name).cloned())
            .or_else(|| self.classes.get(class).cloned().flatten())
            .or_else(|| family_owner(class).map(str::to_string))
            .unwrap_or_else(|| UNOWNED.to_string())
    }
}
